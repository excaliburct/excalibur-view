//! Who sees which folder of a project, the groups of people that decides it
//! for, and the files in those folders that are not drawings.
//!
//! A shop's job folder holds the contract and the prices beside the drawings
//! the shop and the field build from. The office sees all of it; the people
//! in the shop and in the field see the folders given to them and nothing
//! else. So each folder can say who sees it:
//!
//! - `office`: administrators and estimators only;
//! - `everyone`: everybody who may see the project, which is what every
//!   folder was before folders had rules, and what a folder with no rule
//!   anywhere above it still is;
//! - a list of groups (`["Shop", "Field"]`): the people in those groups, and
//!   the office always.
//!
//! A folder without its own rule takes its parent's, and a folder is never
//! seen by more people than the folder it is in. Administrators and
//! estimators see everything. A folder somebody may not see is answered
//! exactly as a folder that does not exist, everywhere its contents could
//! show: listings, downloads, markups, takeoffs, the assistant.
//!
//! The rules are kept on the server and checked on the server, on every
//! request, so a copy of the program that was changed, or a program using a
//! key, cannot see more than its person may.

use std::collections::HashMap; // WHAT: folder rules by folder. WHY: looked up once per folder of a listing.

use axum::body::Body; // WHAT: a response body. WHY: a file's bytes go back in one.
use axum::extract::{Multipart, Path, Query, State}; // WHAT: the request's parts. WHY: the routes below take them.
use axum::http::{header, HeaderMap, StatusCode}; // WHAT: headers and statuses. WHY: downloads say what they are.
use axum::response::{IntoResponse, Response}; // WHAT: a response built by hand. WHY: a file is not JSON.
use axum::Json; // WHAT: JSON in and out. WHY: every other route speaks it.
use hub::{FolderInfo, Group, ProjectFile, Role, See}; // WHAT: the wire types. WHY: the Studio and other programs read the same shapes.
use rusqlite::{params, Connection, OptionalExtension}; // WHAT: the database. WHY: rules, groups and files live in it.
use serde::Deserialize; // WHAT: reading request bodies. WHY: the small JSON bodies below.

use crate::api::{
    administrator, caller, fresh_id, may_reach_project, may_reach_set, now, read_set, record,
    writer, Answer, Denied, Everything, Server, Shared,
}; // WHAT: the API's own gates and helpers. WHY: one set of rules for every route.
use crate::auth::{self, Who}; // WHAT: who is asking. WHY: the rule depends on their role, their groups and their key.

/// The most folders one request may name.
const MOST_FOLDERS: usize = 10_000; // WHAT: ten thousand. WHY: more than any job's whole tree, few enough that one request can't tie the server up.

/// The folder as compared: each folder's name in lower case with its spacing
/// tidied, joined with `/`. The same rule `projects::same_folder` uses.
pub fn key_of(path: &str) -> String {
    path.split('/') // WHAT: each folder's name. WHY: compared one at a time.
        .map(|p| {
            p.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        }) // WHAT: tidied and lower case. WHY: a share's folders don't care about case.
        .collect::<Vec<_>>()
        .join("/") // WHAT: joined back. WHY: one string to look up.
}

/// A folder and every folder it is in, the outermost first:
/// `a`, `a/b`, `a/b/c`.
pub fn and_above(path: &str) -> Vec<String> {
    let parts: Vec<&str> = path.split('/').collect(); // WHAT: its folders. WHY: each prefix is a folder above it.
    (1..=parts.len()).map(|n| parts[..n].join("/")).collect() // WHAT: every prefix. WHY: the rules above apply to it.
}

/// Whether this person is the office: an administrator or an estimator, who
/// see every folder whatever its rule.
pub fn is_office(who: &Who) -> bool {
    who.role != Role::Viewer // WHAT: anybody but a viewer. WHY: the rule as defined: the office is administrators and estimators.
}

/// A rule as it is written in the database.
fn stored(see: &See) -> String {
    see.to_value().to_string() // WHAT: the JSON of the rule. WHY: read back by `from_stored`.
}

/// A rule read back. One nobody can read is the office only: the safe way
/// round, never everyone.
fn from_stored(text: &str) -> See {
    serde_json::from_str::<serde_json::Value>(text) // WHAT: the JSON. WHY: as `stored` wrote it.
        .ok()
        .and_then(|v| See::from_value(&v).ok()) // WHAT: read as a rule. WHY: the same reading as the API's.
        .unwrap_or(See::Office) // WHAT: unreadable is the office. WHY: a damaged rule must never open a folder.
}

/// The rules on one project's folders, by folder key.
fn rules_of(db: &Connection, project: &str) -> rusqlite::Result<HashMap<String, See>> {
    let mut statement =
        db.prepare("SELECT key, see FROM folders WHERE project = ?1 AND see IS NOT NULL")?; // WHAT: the folders that have their own rule. WHY: the rest take their parent's.
    let rows = statement.query_map(params![project], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?; // WHAT: each key and rule. WHY: for the map.
    let mut rules = HashMap::new(); // WHAT: the map. WHY: looked up per folder.
    for row in rows {
        let (key, see) = row?; // WHAT: one rule. WHY: a database error is passed up, never read as no rule.
        rules.insert(key, from_stored(&see)); // WHAT: kept by key. WHY: compared case and spacing aside.
    }
    Ok(rules)
}

/// What one person may see of one project's folders.
pub struct Sight {
    everything: bool, // WHAT: true for the office. WHY: the office sees every folder, and nothing need be read.
    groups: Vec<String>, // WHAT: the person's groups, lower case. WHY: a folder given to one of them is theirs to see.
    rules: HashMap<String, See>, // WHAT: the project's folder rules. WHY: each folder's own, by key.
}

impl Sight {
    /// Everything: the office's sight.
    pub fn everything() -> Sight {
        Sight {
            everything: true,
            groups: Vec::new(),
            rules: HashMap::new(),
        } // WHAT: no rules needed. WHY: none would take anything away.
    }

    /// Whether this is the office's sight.
    pub fn is_everything(&self) -> bool {
        self.everything // WHAT: the flag. WHY: a listing can skip filtering.
    }

    /// Who sees `folder`, its parents' rules counted. No folder is the
    /// project's own, which everybody on the project sees.
    pub fn rule(&self, folder: Option<&str>) -> See {
        let mut rule = See::Everyone; // WHAT: the default. WHY: a folder with no rule anywhere above it is everybody's, as every folder was before.
        if let Some(folder) = folder {
            for above in and_above(folder) {
                if let Some(own) = self.rules.get(&key_of(&above)) {
                    rule = own.within(&rule); // WHAT: this folder's own rule, inside the one above. WHY: never wider than its parent.
                }
            }
        }
        rule
    }

    /// Whether this person sees what is in `folder`.
    pub fn sees(&self, folder: Option<&str>) -> bool {
        if self.everything {
            return true; // WHAT: the office. WHY: sees every folder.
        }
        match self.rule(folder) {
            See::Everyone => true, // WHAT: everybody's. WHY: the default and the word.
            See::Office => false,  // WHAT: the office's only. WHY: this person is not the office.
            See::Groups(groups) => groups
                .iter()
                .any(|g| self.groups.contains(&g.to_lowercase())), // WHAT: one of their groups is named. WHY: given to that group.
        }
    }
}

/// What `who` may see of `project`'s folders, read fresh from the database.
pub fn sight_of(db: &Connection, project: &str, who: &Who) -> rusqlite::Result<Sight> {
    if is_office(who) {
        return Ok(Sight {
            everything: true,
            groups: Vec::new(),
            rules: rules_of(db, project)?,
        }); // WHAT: the office, with the project's rules. WHY: it sees everything, and a listing still shows it who else sees each folder.
    }
    let mut statement = db.prepare(
        "SELECT g.name FROM groups g JOIN group_people m ON m.grp = g.id WHERE m.person = ?1",
    )?; // WHAT: the person's groups. WHY: read on every request, so taking somebody out of a group takes effect at once.
    let groups = statement
        .query_map(params![who.id], |r| r.get::<_, String>(0))?
        .map(|g| g.map(|g| g.to_lowercase())) // WHAT: lower case. WHY: names compare whatever their case.
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Sight {
        everything: false,
        groups,
        rules: rules_of(db, project)?,
    }) // WHAT: their groups and the project's rules. WHY: all `sees` needs.
}

/// The same, through the server's store, refused as a server error when the
/// database can't be read: never read as "may see".
pub fn sight(server: &Server, project: &str, who: &Who) -> Answer<Sight> {
    if is_office(who) {
        return Ok(Sight::everything()); // WHAT: the office. WHY: no database read for the common case.
    }
    server
        .store
        .with(|db| Ok(sight_of(db, project, who)?)) // WHAT: read fresh. WHY: a change of group or rule counts at once.
        .map_err(|e| Denied::broke("check who sees that folder", e)) // WHAT: a failure said plainly. WHY: refused, never waved through.
}

/// The gate for anything kept in a folder: the project's own gate, then the
/// folder's. Refused as `what` not found, exactly as one that isn't there.
pub fn may_see_folder(
    server: &Server,
    who: &Who,
    project: &str,
    folder: Option<&str>,
    what: &str,
) -> Answer<()> {
    if sight(server, project, who)?.sees(folder) {
        Ok(()) // WHAT: theirs to see. WHY: the folder's rule lets them.
    } else {
        Err(Denied::missing(what)) // WHAT: not found. WHY: nobody learns a folder, or what is in it, exists by being refused it.
    }
}

/// How many current drawing sets of a project this sight sees: the count a
/// project shows, so a hidden folder's sets aren't counted for somebody who
/// can't see them.
pub fn current_sets_seen(db: &Connection, project: &str, sight: &Sight) -> rusqlite::Result<usize> {
    let mut statement =
        db.prepare("SELECT folder FROM sets WHERE project = ?1 AND superseded_by IS NULL")?; // WHAT: each current set's folder. WHY: counted by who sees it.
    let folders = statement
        .query_map(params![project], |r| r.get::<_, Option<String>>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(folders.iter().filter(|f| sight.sees(f.as_deref())).count()) // WHAT: only the ones they see. WHY: a count is part of what a folder holds.
}

/// Makes `path` and every folder above it where they aren't already written
/// down. A program's sync key makes a folder that has no rule above it the
/// office's, so a mirrored job is safe until somebody says who sees what;
/// anybody else's new folder takes its parent's rule, as before. Answers how
/// many were made.
fn make_folder(
    db: &Connection,
    project: &str,
    path: &str,
    who: &Who,
    by_program: bool,
) -> rusqlite::Result<usize> {
    let mut made = 0; // WHAT: how many were new. WHY: said in the answer.
    let mut ruled_above = false; // WHAT: whether a folder above has its own rule. WHY: then a new one takes it, rather than the office's.
    for above in and_above(path) {
        let key = key_of(&above); // WHAT: as compared. WHY: one row per folder whatever its case.
        let there: Option<Option<String>> = db
            .query_row(
                "SELECT see FROM folders WHERE project = ?1 AND key = ?2",
                params![project, key],
                |r| r.get(0),
            )
            .optional()?; // WHAT: the folder's row, and its rule. WHY: made only where missing.
        match there {
            Some(see) => ruled_above |= see.is_some(), // WHAT: there already. WHY: never changed by being named again.
            None => {
                let office = by_program && !ruled_above; // WHAT: a program's new folder with no rule above it. WHY: safe by default for a mirror of the whole job.
                let see = office.then(|| stored(&See::Office)); // WHAT: the office's, or none to take the parent's. WHY: as just said.
                db.execute(
                    "INSERT INTO folders (project, key, path, see, set_in, set_by, set_at, made, made_by)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    params![
                        project,
                        key,
                        above,
                        see,
                        office.then_some("program"),
                        office.then(|| who.name.clone()),
                        office.then(now),
                        now(),
                        who.name
                    ],
                )?; // WHAT: the folder written down. WHY: so an empty one shows, and so it can carry a rule.
                ruled_above |= office; // WHAT: this one now has a rule. WHY: the folders under it take it.
                made += 1; // WHAT: counted. WHY: said in the answer.
            }
        }
    }
    Ok(made)
}

/// Every folder of a project this sight sees: those written down, those a
/// current set or file is in, and every folder above them, each once.
pub fn listing(db: &Connection, project: &str, sight: &Sight) -> rusqlite::Result<Vec<FolderInfo>> {
    struct Row {
        path: String,           // WHAT: as first written. WHY: shown so.
        own: Option<See>,       // WHAT: its own rule. WHY: shown, and for the Studio.
        set_in: Option<String>, // WHAT: program or view. WHY: shown.
        set_by: Option<String>, // WHAT: who set it. WHY: shown.
        set_at: Option<String>, // WHAT: when. WHY: shown.
    }
    let mut found: Vec<(String, Row)> = Vec::new(); // WHAT: each folder by key, in the order found. WHY: one entry per folder.
    let mut statement = db
        .prepare("SELECT key, path, see, set_in, set_by, set_at FROM folders WHERE project = ?1")?; // WHAT: the folders written down. WHY: their spelling and rules come first.
    let rows = statement.query_map(params![project], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Row {
                path: r.get(1)?,
                own: r.get::<_, Option<String>>(2)?.map(|s| from_stored(&s)),
                set_in: r.get(3)?,
                set_by: r.get(4)?,
                set_at: r.get(5)?,
            },
        ))
    })?;
    for row in rows {
        found.push(row?); // WHAT: kept. WHY: listed.
    }
    let mut counts: HashMap<String, (usize, usize)> = HashMap::new(); // WHAT: sets and files in each folder, by key. WHY: shown beside it.
    let mut held: Vec<String> = Vec::new(); // WHAT: the folders things are in. WHY: they are folders whether or not written down.
    let mut statement = db.prepare(
        "SELECT folder, 0 FROM sets WHERE project = ?1 AND superseded_by IS NULL AND folder IS NOT NULL
         UNION ALL SELECT folder, 1 FROM files WHERE project = ?1 AND replaced_by IS NULL AND folder IS NOT NULL",
    )?; // WHAT: each current set's and file's folder. WHY: counted, and listed even when not written down.
    let rows = statement.query_map(params![project], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
    })?;
    for row in rows {
        let (folder, file) = row?; // WHAT: one thing's folder, and whether it is a file. WHY: counted in the right column.
        let count = counts.entry(key_of(&folder)).or_default(); // WHAT: its folder's counts. WHY: added to.
        if file == 1 {
            count.1 += 1
        } else {
            count.0 += 1
        } // WHAT: one more file or set. WHY: shown.
        held.push(folder); // WHAT: kept. WHY: listed with the folders above it.
    }
    let mut every: Vec<String> = found.iter().map(|(_, r)| r.path.clone()).collect(); // WHAT: the written-down folders' paths. WHY: and the folders above them.
    every.extend(held); // WHAT: and the folders things are in. WHY: likewise.
    let mut listed: Vec<FolderInfo> = Vec::new(); // WHAT: the answer. WHY: built below.
    let mut keys: Vec<String> = Vec::new(); // WHAT: the keys already listed. WHY: each folder once.
    for path in every {
        for above in and_above(&path) {
            let key = key_of(&above); // WHAT: as compared. WHY: once each.
            if keys.contains(&key) {
                continue; // WHAT: listed already. WHY: once each.
            }
            keys.push(key.clone()); // WHAT: noted. WHY: once each.
            if !sight.sees(Some(&above)) {
                continue; // WHAT: a folder this person can't see. WHY: left out as if it weren't there; its name is never shown.
            }
            let row = found.iter().find(|(k, _)| *k == key).map(|(_, r)| r); // WHAT: its row, if written down. WHY: its spelling and rule.
            let (sets, files) = counts.get(&key).copied().unwrap_or_default(); // WHAT: what is in it. WHY: shown.
            listed.push(FolderInfo {
                folder: row.map(|r| r.path.clone()).unwrap_or(above.clone()), // WHAT: the spelling first written. WHY: as the program sent it.
                see: sight.rule(Some(&above)), // WHAT: who sees it, the parents counted. WHY: what actually decides.
                own: row.and_then(|r| r.own.clone()), // WHAT: its own rule, if any. WHY: the Studio shows where a rule was set.
                set_in: row.and_then(|r| r.set_in.clone()), // WHAT: where it was last set. WHY: shown.
                set_by: row.and_then(|r| r.set_by.clone()), // WHAT: who. WHY: shown.
                set_at: row.and_then(|r| r.set_at.clone()), // WHAT: when. WHY: shown.
                sets,                                       // WHAT: its current sets. WHY: shown.
                files,                                      // WHAT: its current files. WHY: shown.
            });
        }
    }
    listed.sort_by_key(|f| key_of(&f.folder)); // WHAT: in order, a folder before what is in it. WHY: reads as a tree.
    Ok(listed)
}

// ---- groups --------------------------------------------------------------

fn group_by_id(db: &Connection, id: &str) -> rusqlite::Result<Option<Group>> {
    let found = db
        .query_row(
            "SELECT id, name, made, made_by FROM groups WHERE id = ?1",
            params![id],
            |r| {
                Ok(Group {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    people: Vec::new(),
                    made: r.get(2)?,
                    made_by: r.get(3)?,
                })
            },
        )
        .optional()?; // WHAT: the group, if there is one. WHY: its people are read next.
    let Some(mut group) = found else {
        return Ok(None);
    }; // WHAT: no such group. WHY: answered as not found.
    let mut statement =
        db.prepare("SELECT person FROM group_people WHERE grp = ?1 ORDER BY added")?; // WHAT: who is in it. WHY: shown.
    group.people = statement
        .query_map(params![id], |r| r.get(0))?
        .collect::<rusqlite::Result<Vec<String>>>()?; // WHAT: their ids. WHY: the Studio ticks them.
    Ok(Some(group))
}

fn every_group(db: &Connection) -> rusqlite::Result<Vec<Group>> {
    let mut statement = db.prepare("SELECT id FROM groups ORDER BY name COLLATE NOCASE")?; // WHAT: every group, by name. WHY: listed in order.
    let ids = statement
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut groups = Vec::new(); // WHAT: the answer. WHY: each with its people.
    for id in ids {
        if let Some(group) = group_by_id(db, &id)? {
            groups.push(group); // WHAT: kept. WHY: listed.
        }
    }
    Ok(groups)
}

/// The group of this name, made empty if there isn't one: whether it was made.
fn group_named(db: &Connection, name: &str, by: &str) -> rusqlite::Result<(String, bool)> {
    if let Some(id) = db
        .query_row(
            "SELECT id FROM groups WHERE name = ?1 COLLATE NOCASE",
            params![name],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        return Ok((id, false)); // WHAT: there already. WHY: Shop and shop are one group.
    }
    let id = fresh_id("grp"); // WHAT: a new id. WHY: the group's.
    db.execute(
        "INSERT INTO groups (id, name, made, made_by) VALUES (?1, ?2, ?3, ?4)",
        params![id, name, now(), by],
    )?; // WHAT: made, nobody in it. WHY: an administrator puts people in it later.
    Ok((id, true))
}

/// `GET /groups`: every group and who is in it. Administrators only.
pub async fn groups(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<Group>>> {
    administrator(&server, &headers)?; // WHAT: an administrator signed in. WHY: who is in which group decides who sees which folder.
    server
        .store
        .with(|db| Ok(every_group(db)?))
        .map(Json)
        .map_err(|e| Denied::broke("list the groups", e)) // WHAT: every group. WHY: the Studio's list.
}

#[derive(Deserialize)]
pub struct NewGroup {
    pub name: String, // WHAT: what to call it. WHY: a folder names it.
}

/// The name a group is given, tidied, or why it can't be.
fn group_name(name: &str) -> Answer<String> {
    match See::from_value(&serde_json::json!([name])) {
        Ok(See::Groups(names)) => Ok(names[0].clone()), // WHAT: the tidied name. WHY: the same rule a folder's list of groups is read with.
        Ok(_) | Err(_) => Err(Denied::wrong(format!(
            "A group's name is 1 to {} characters of ordinary text.",
            hub::LONGEST_GROUP_NAME
        ))), // WHAT: refused. WHY: an empty or odd name.
    }
}

/// `POST /groups` `{"name": "Shop"}`: a group, made if it isn't there, and
/// the one there if it is. Administrators only.
pub async fn make_group(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewGroup>,
) -> Answer<Json<Group>> {
    let who = administrator(&server, &headers)?; // WHAT: an administrator signed in. WHY: as above.
    let name = group_name(&body.name)?; // WHAT: the name, tidied. WHY: refused if it can't be one.
    let (group, made) = server
        .store
        .with(|db| {
            let (id, made) = group_named(db, &name, &who.name)?; // WHAT: found or made. WHY: asking twice makes one group.
            Ok((group_by_id(db, &id)?, made))
        })
        .map_err(|e| Denied::broke("make that group", e))?;
    let group = group.ok_or_else(|| Denied::missing("group"))?; // WHAT: the group. WHY: it was just found or made.
    if made {
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::MADE_GROUP, &who.email)
                .by(&who.id)
                .about(name),
        ); // WHAT: written down. WHY: groups decide who sees folders.
    }
    Ok(Json(group))
}

#[derive(Deserialize)]
pub struct GroupMember {
    pub person: String, // WHAT: the person's id. WHY: who goes in or comes out.
    #[serde(default = "yes")]
    pub on: bool, // WHAT: true puts them in, false takes them out. WHY: one route for both, as a project's people.
}

fn yes() -> bool {
    true // WHAT: in, unless said otherwise. WHY: the ordinary thing to ask.
}

/// `POST /groups/{id}/people` `{"person": "usr_…", "on": true}`: somebody put
/// in a group or taken out. Administrators only. Counts on their next request.
pub async fn change_group(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<GroupMember>,
) -> Answer<Json<Group>> {
    let who = administrator(&server, &headers)?; // WHAT: an administrator signed in. WHY: as above.
    let person = body.person.trim().to_string(); // WHAT: whose. WHY: an id with spaces round it is the same id.
    let group = server
        .store
        .with(|db| {
            if group_by_id(db, &id)?.is_none() {
                return Ok(Err("group")); // WHAT: no such group. WHY: not found.
            }
            let known: bool = db.query_row("SELECT count(*) FROM people WHERE id = ?1", params![person], |r| r.get::<_, i64>(0))? > 0; // WHAT: whether the person exists. WHY: a group of nobody's ids is no use.
            if !known {
                return Ok(Err("person")); // WHAT: no such person. WHY: not found.
            }
            if body.on {
                db.execute(
                    "INSERT INTO group_people (grp, person, added, added_by) VALUES (?1, ?2, ?3, ?4) ON CONFLICT DO NOTHING",
                    params![id, person, now(), who.id],
                )?; // WHAT: put in. WHY: twice is not an error.
            } else {
                db.execute("DELETE FROM group_people WHERE grp = ?1 AND person = ?2", params![id, person])?; // WHAT: taken out. WHY: they stop seeing its folders at once.
            }
            Ok(Ok(group_by_id(db, &id)?))
        })
        .map_err(|e| Denied::broke("change who is in that group", e))?;
    let group = group
        .map_err(Denied::missing)?
        .ok_or_else(|| Denied::missing("group"))?; // WHAT: the group as it is now. WHY: the answer.
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::CHANGED_GROUP, &who.email)
            .by(&who.id)
            .about(group.name.clone())
            .saying(format!(
                "{} {person}",
                if body.on { "put in" } else { "taken out" }
            )),
    ); // WHAT: written down. WHY: somebody will ask when a person started or stopped seeing a folder.
    Ok(Json(group))
}

// ---- folders -------------------------------------------------------------

/// `GET /projects/{id}/folders`: every folder of the project the caller may
/// see, the empty ones too, each with who sees it.
pub async fn folders_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Answer<Json<Vec<FolderInfo>>> {
    let who = caller(&server, &headers)?; // WHAT: anybody signed in, or a key. WHY: reading is for whoever may see the project.
    may_reach_project(&server, &who, &project)?; // WHAT: the project's gate. WHY: the same as every route under a project.
    server
        .store
        .with(|db| Ok(listing(db, &project, &sight_of(db, &project, &who)?)?)) // WHAT: what they may see of its folders, with every rule. WHY: hidden folders are left off, and each shown folder says who sees it.
        .map(Json)
        .map_err(|e| Denied::broke("list that project's folders", e))
}

/// One folder asked for: its path, and who should see it, if said.
struct Asked {
    path: String,     // WHAT: the folder, checked. WHY: as a set's folder is.
    see: Option<See>, // WHAT: who sees it, when said. WHY: left out, an existing folder's rule is left as it is.
}

/// Reads `{"folders": [...]}`: each a folder's name, or `{"folder", "see"}`.
fn asked(body: &serde_json::Value) -> Answer<Vec<Asked>> {
    let Some(list) = body.get("folders").and_then(|f| f.as_array()) else {
        return Err(Denied::wrong(
            "Send {\"folders\": [...]}: each a folder, or {\"folder\": ..., \"see\": ...}.",
        )); // WHAT: no list. WHY: said plainly.
    };
    if list.len() > MOST_FOLDERS {
        return Err(Denied::wrong(format!(
            "At most {MOST_FOLDERS} folders at a time."
        ))); // WHAT: too many. WHY: one request can't tie the server up.
    }
    let mut out = Vec::with_capacity(list.len()); // WHAT: the folders read. WHY: the answer.
    for item in list {
        let (text, see) = match item {
            serde_json::Value::String(text) => (text.as_str(), None), // WHAT: a folder's name alone. WHY: what a program that only makes folders sends.
            serde_json::Value::Object(fields) => {
                let Some(text) = fields.get("folder").and_then(|f| f.as_str()) else {
                    return Err(Denied::wrong("Each folder needs its \"folder\"."));
                    // WHAT: no folder named. WHY: refused.
                };
                let see = match fields.get("see") {
                    None | Some(serde_json::Value::Null) => None, // WHAT: no rule said. WHY: left as it is.
                    Some(value) => Some(See::from_value(value).map_err(Denied::wrong)?), // WHAT: the rule. WHY: refused if it can't be read.
                };
                (text, see)
            }
            _ => {
                return Err(Denied::wrong(
                    "Each folder is its name, or {\"folder\": ..., \"see\": ...}.",
                ))
            } // WHAT: anything else. WHY: refused.
        };
        let Some(path) = crate::projects::folders(text, "A folder").map_err(Denied::wrong)? else {
            return Err(Denied::wrong("A folder needs a name.")); // WHAT: an empty folder name. WHY: the project's own is not a folder to make.
        };
        out.push(Asked { path, see }); // WHAT: kept. WHY: made below.
    }
    Ok(out)
}

/// `POST /projects/{id}/folders`: makes the folders that aren't there (empty
/// ones too), and sets who sees each one that says. Never takes a folder away.
/// For an administrator, and for a sync key on its own projects.
pub async fn make_folders(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Answer<Json<serde_json::Value>> {
    let who = caller(&server, &headers)?; // WHAT: who is asking. WHY: checked just below.
    let by_program = who.is_key(auth::purpose::SYNC); // WHAT: a program keeping its projects in step. WHY: its new folders are the office's until it says.
    if !by_program {
        administrator(&server, &headers)?; // WHAT: otherwise an administrator signed in. WHY: deciding who sees a folder is theirs.
    }
    crate::api::sharing(&server)?; // WHAT: new work can be shared. WHY: the same rule as every write.
    may_reach_project(&server, &who, &project)?; // WHAT: the project's gate; a sync key's own projects only. WHY: as every route under a project.
    let asked = asked(&body)?; // WHAT: the folders asked for. WHY: all checked before anything is written.
    let set_in = if by_program { "program" } else { "view" }; // WHAT: where the rule was set. WHY: a rule changed in the Studio is kept until the program sets that folder again.
    let (made, changed, groups_made, changes) = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: one step. WHY: half a tree of folders is never visible.
            let db: &Connection = &tx;
            let mut made = 0usize; // WHAT: folders made. WHY: said in the answer.
            let mut changed = 0usize; // WHAT: rules changed. WHY: said in the answer.
            let mut groups_made: Vec<String> = Vec::new(); // WHAT: groups made empty. WHY: the answer says so, for an administrator to fill.
            let mut changes: Vec<String> = Vec::new(); // WHAT: the folders whose rule changed. WHY: for the audit line.
            for one in &asked {
                made += make_folder(db, &project, &one.path, &who, by_program)?; // WHAT: the folder and those above it. WHY: an empty folder shows.
                let Some(see) = &one.see else { continue }; // WHAT: no rule said. WHY: left as it is.
                if let See::Groups(names) = see {
                    for name in names {
                        if group_named(db, name, &who.name)?.1 {
                            groups_made.push(name.clone()); // WHAT: a group nobody had made. WHY: made empty, and said.
                        }
                    }
                }
                let key = key_of(&one.path); // WHAT: as compared. WHY: the row to change.
                let was: Option<String> = db.query_row(
                    "SELECT see FROM folders WHERE project = ?1 AND key = ?2",
                    params![project, key],
                    |r| r.get(0),
                )?; // WHAT: its rule now. WHY: only a real change is counted and written down.
                if was.as_deref().map(from_stored).as_ref() != Some(see) {
                    db.execute(
                        "UPDATE folders SET see = ?1, set_in = ?2, set_by = ?3, set_at = ?4 WHERE project = ?5 AND key = ?6",
                        params![stored(see), set_in, who.name, now(), project, key],
                    )?; // WHAT: the new rule, and who set it where. WHY: shown in the Studio, and kept until changed.
                    changed += 1; // WHAT: counted. WHY: said.
                    changes.push(format!("{}: {}", one.path, see.words())); // WHAT: what changed. WHY: the audit line.
                }
            }
            tx.commit()?; // WHAT: all of it at once. WHY: as above.
            Ok((made, changed, groups_made, changes))
        })
        .map_err(|e| Denied::broke("make those folders", e))?;
    let (listed, rules) = server
        .store
        .with(|db| {
            Ok((
                listing(db, &project, &sight_of(db, &project, &who)?)?,
                rules_of(db, &project)?,
            ))
        }) // WHAT: the whole tree, with its rules. WHY: only the office or the project's program gets here.
        .map_err(|e| Denied::broke("list that project's folders", e))?;
    let narrowed = Sight {
        everything: false,
        groups: Vec::new(),
        rules,
    }; // WHAT: the rules, for working out each parent's. WHY: to say when a folder was asked to be wider than its parent.
    let mut notes: Vec<String> = Vec::new(); // WHAT: what the caller should know. WHY: said, rather than silently changed.
    for one in &asked {
        let (Some(see), Some((parent, _))) = (&one.see, one.path.rsplit_once('/')) else {
            continue;
        }; // WHAT: a rule on a folder inside another. WHY: only those have a parent to be wider than.
        let above = narrowed.rule(Some(parent)); // WHAT: who sees the folder it is in. WHY: it can't be seen by more.
        if see.wider_than(&above) {
            notes.push(format!(
                "\"{}\" is seen by {} at most: a folder is never seen by more people than the folder it is in (\"{parent}\": {}).",
                one.path,
                see.within(&above).words().to_lowercase(),
                above.words().to_lowercase()
            )); // WHAT: said plainly. WHY: the rule is kept as asked, but what decides is narrower.
        }
    }
    if !groups_made.is_empty() {
        notes.push(format!(
            "Made {} empty: an administrator puts people in them in Studio, Office, People.",
            groups_made.join(", ")
        )); // WHAT: the groups made. WHY: nobody sees those folders until somebody is put in them.
    }
    if changed > 0 {
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::CHANGED_FOLDER, &who.email)
                .by(&who.id)
                .about(project.clone())
                .saying(
                    changes
                        .iter()
                        .take(20)
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("; "),
                ),
        ); // WHAT: one line for the request. WHY: who opened which folder to whom, without a line per folder of a whole job.
    }
    Ok(Json(serde_json::json!({
        "project": project,
        "holds": listed.len(),
        "made": made,
        "changed": changed,
        "groups_made": groups_made,
        "notes": notes,
        "folders": listed,
    }))) // WHAT: what was done, and the project's folders now. WHY: the program and the Studio see the result in one answer.
}

// ---- files that aren't drawings -------------------------------------------

const FILE_COLUMNS: &str = "id, project, folder, name, digest, bytes, modified, uploaded, uploaded_by, replaces, replaced_by"; // WHAT: a file's columns. WHY: in the order `file_from_row` reads them.

fn file_from_row(r: &rusqlite::Row) -> rusqlite::Result<ProjectFile> {
    Ok(ProjectFile {
        id: r.get(0)?,
        project: r.get(1)?,
        folder: r.get(2)?,
        name: r.get(3)?,
        digest: r.get(4)?,
        bytes: r.get::<_, i64>(5)? as u64,
        modified: r.get(6)?,
        uploaded: r.get(7)?,
        uploaded_by: r.get(8)?,
        replaces: r.get(9)?,
        replaced_by: r.get(10)?,
        already_here: false,
    }) // WHAT: one file, as the wire has it. WHY: one place reads a row.
}

fn read_file(db: &Connection, id: &str) -> rusqlite::Result<Option<ProjectFile>> {
    db.query_row(
        &format!("SELECT {FILE_COLUMNS} FROM files WHERE id = ?1"),
        params![id],
        file_from_row,
    )
    .optional() // WHAT: a file by id. WHY: every file route starts here.
}

/// The gate for a file: its project's, then its folder's. A file nobody can
/// find and one somebody may not see are answered the same.
fn may_reach_file(server: &Server, who: &Who, id: &str) -> Answer<ProjectFile> {
    let file = server
        .store
        .with(|db| Ok(read_file(db, id)?))
        .map_err(|e| Denied::broke("find that file", e))?
        .ok_or_else(|| Denied::missing("file"))?; // WHAT: the file, if there is one. WHY: not found otherwise.
    may_reach_project(server, who, &file.project).map_err(|_| Denied::missing("file"))?; // WHAT: the project's gate. WHY: refused in the file's words, so nothing says the file exists.
    may_see_folder(server, who, &file.project, file.folder.as_deref(), "file")?; // WHAT: the folder's gate. WHY: a file in a folder they can't see is not there for them.
    Ok(file)
}

/// A file's name as it was sent, without any folders in front of it and
/// without anything that isn't printable.
fn file_name(sent: &str) -> Answer<String> {
    let name = sent.rsplit(['/', '\\']).next().unwrap_or("").trim(); // WHAT: the last part only. WHY: a name never says where to write.
    if name.is_empty() || name == "." || name == ".." {
        return Err(Denied::wrong("A file needs a name.")); // WHAT: no name. WHY: a file is listed by it.
    }
    if name.chars().any(char::is_control) || name.chars().count() > 255 {
        return Err(Denied::wrong(
            "A file's name is at most 255 characters of ordinary text.",
        )); // WHAT: a name no file window would show. WHY: refused rather than mangled.
    }
    Ok(name.to_string())
}

/// When a file was saved, as RFC 3339, or why that isn't a time.
fn saved_at(sent: &str) -> Answer<Option<String>> {
    let sent = sent.trim(); // WHAT: without spaces round it. WHY: a form field may carry a newline.
    if sent.is_empty() {
        return Ok(None); // WHAT: not said. WHY: allowed; the Studio shows when it arrived instead.
    }
    let rfc = &time::format_description::well_known::Rfc3339; // WHAT: ISO 8601 as the web writes it. WHY: what programs send.
    let at = time::OffsetDateTime::parse(sent, rfc)
        .map_err(|_| Denied::wrong(format!("\"{sent}\" isn't a time: send when the file was saved as ISO 8601, 2026-10-28T09:30:00Z.")))?; // WHAT: read. WHY: refused if it can't be.
    Ok(at.to_offset(time::UtcOffset::UTC).format(rfc).ok()) // WHAT: written back in UTC. WHY: one way of writing every time here.
}

/// `POST /files`, multipart: `project`, `folder`, `modified` and `file`. Any
/// kind of file, up to the server's largest upload. The same bytes already in
/// the project answer with that file (`already_here`); the same name in the
/// same folder is the file saved again, and replaces it in place, the one
/// before kept as its history (`replaces`).
pub async fn upload_file(
    State(server): State<Shared>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Answer<Json<ProjectFile>> {
    let who = writer(&server, &headers)?; // WHAT: somebody who may change drawings. WHY: putting a file in a project is a change.
    let (mut project, mut folder, mut modified, mut sent_name) =
        (String::new(), String::new(), String::new(), String::new()); // WHAT: the form's fields. WHY: read below.
    let mut bytes: Vec<u8> = Vec::new(); // WHAT: the file. WHY: read below.
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| Denied::wrong(format!("That upload was malformed: {e}")))?
    {
        match field.name().unwrap_or("") {
            "project" => project = field.text().await.unwrap_or_default(), // WHAT: its project. WHY: the file goes in it.
            "folder" => folder = field.text().await.unwrap_or_default(), // WHAT: its folder. WHY: listed there.
            "modified" => modified = field.text().await.unwrap_or_default(), // WHAT: when it was saved. WHY: shown.
            "name" => sent_name = field.text().await.unwrap_or_default(), // WHAT: a name given outright. WHY: used when the file part carries none.
            "file" => {
                if sent_name.is_empty() {
                    sent_name = field.file_name().unwrap_or("").to_string(); // WHAT: the file's own name. WHY: how a file is listed.
                }
                bytes = field
                    .bytes()
                    .await
                    .map_err(|e| Denied::wrong(format!("That file did not arrive whole: {e}")))?
                    .to_vec(); // WHAT: its bytes. WHY: stored.
            }
            _ => {} // WHAT: anything else. WHY: ignored, as /sets does.
        }
    }
    if bytes.is_empty() {
        return Err(Denied::wrong("No file came with that upload.")); // WHAT: nothing to keep. WHY: an empty file is not worth a line in a folder.
    }
    let name = file_name(&sent_name)?; // WHAT: its name, checked. WHY: listed by it.
    let modified = saved_at(&modified)?; // WHAT: when it was saved, checked. WHY: shown.
    let folder = crate::projects::folders(&folder, "A file's folder").map_err(Denied::wrong)?; // WHAT: the folder, checked as a set's is. WHY: one rule for folders.
    let known = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT 1 FROM projects WHERE id = ?1",
                    params![project],
                    |_| Ok(()),
                )
                .optional()?
                .is_some())
        })
        .map_err(|e| Denied::broke("check that project", e))?; // WHAT: whether the project is there. WHY: not found otherwise.
    if !known {
        return Err(Denied::missing("project")); // WHAT: no such project. WHY: as /sets answers.
    }
    may_reach_project(&server, &who, &project)?; // WHAT: the project's gate. WHY: as every route under a project.
    may_see_folder(&server, &who, &project, folder.as_deref(), "project")?; // WHAT: the folder's gate. WHY: nobody puts a file where they couldn't see it.
    let digest = server
        .store
        .put_blob(&bytes)
        .map_err(|e| Denied::broke("store that file", e))?; // WHAT: the bytes kept once, by their digest. WHY: the same file twice costs one copy.
    let by_program = who.is_key(auth::purpose::SYNC); // WHAT: a program's sync key. WHY: its new folders are the office's.
    let file = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: one step. WHY: a file half-replaced is never visible.
            let db: &Connection = &tx;
            let there = db
                .query_row(
                    &format!("SELECT {FILE_COLUMNS} FROM files WHERE project = ?1 AND digest = ?2 AND replaced_by IS NULL LIMIT 1"),
                    params![project, digest],
                    file_from_row,
                )
                .optional()?; // WHAT: these bytes already in the project. WHY: that one is the answer.
            if let Some(mut there) = there {
                there.already_here = true; // WHAT: said. WHY: nothing new was made.
                return Ok(there);
            }
            if let Some(folder) = &folder {
                make_folder(db, &project, folder, &who, by_program)?; // WHAT: its folder written down. WHY: a program's new folder is the office's until it says.
            }
            let mut statement = db.prepare(&format!("SELECT {FILE_COLUMNS} FROM files WHERE project = ?1 AND replaced_by IS NULL"))?; // WHAT: the current files. WHY: the one of this name in this folder is replaced.
            let current = statement.query_map(params![project], file_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement); // WHAT: the query put away. WHY: it holds the transaction, which is committed below.
            let before = current.into_iter().find(|f| {
                f.name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase() == name.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
                    && crate::projects::same_folder(f.folder.as_deref(), folder.as_deref())
            }); // WHAT: the same name in the same folder, case and spacing aside. WHY: that is the file saved again.
            let file = ProjectFile {
                id: fresh_id("file"),
                project: project.clone(),
                folder: before.as_ref().and_then(|b| b.folder.clone()).or(folder.clone()), // WHAT: the folder as first spelled. WHY: a folder doesn't show twice for its case.
                name: name.clone(),
                bytes: bytes.len() as u64,
                modified: modified.clone(),
                digest: digest.clone(),
                uploaded: now(),
                uploaded_by: who.name.clone(),
                replaces: before.as_ref().map(|b| b.id.clone()), // WHAT: the one it replaces. WHY: kept as its history.
                replaced_by: None,
                already_here: false,
            }; // WHAT: the new file. WHY: written next.
            db.execute(
                "INSERT INTO files (id, project, folder, name, digest, bytes, modified, uploaded, uploaded_by, replaces)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![file.id, file.project, file.folder, file.name, file.digest, file.bytes as i64, file.modified, file.uploaded, file.uploaded_by, file.replaces],
            )?; // WHAT: kept. WHY: listed in its folder.
            if let Some(before) = &before {
                db.execute("UPDATE files SET replaced_by = ?1 WHERE id = ?2", params![file.id, before.id])?; // WHAT: the old one marked. WHY: kept, but no longer the current one.
            }
            tx.commit()?; // WHAT: all at once. WHY: as above.
            Ok(file)
        })
        .map_err(|e| Denied::broke("keep that file", e))?;
    if !file.already_here {
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::UPLOADED_FILE, &who.email)
                .by(&who.id)
                .about(file.name.clone())
                .saying(file.folder.clone().unwrap_or_default()),
        ); // WHAT: written down. WHY: the same line a drawing set gets.
    }
    Ok(Json(file))
}

/// `GET /projects/{id}/files`: the project's current files the caller may
/// see; `?all=1` adds the ones replaced since.
pub async fn files_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(everything): Query<Everything>,
) -> Answer<Json<Vec<ProjectFile>>> {
    let who = caller(&server, &headers)?; // WHAT: anybody signed in, or a key. WHY: reading.
    may_reach_project(&server, &who, &project)?; // WHAT: the project's gate. WHY: as every route under a project.
    let sight = sight(&server, &project, &who)?; // WHAT: what they may see. WHY: a hidden folder's files are left off.
    let list = server
        .store
        .with(|db| {
            let mut statement = db.prepare(&format!(
                "SELECT {FILE_COLUMNS} FROM files WHERE project = ?1 AND (?2 OR replaced_by IS NULL) ORDER BY folder, name COLLATE NOCASE, uploaded"
            ))?; // WHAT: its files, the current ones unless asked for all. WHY: as sets are listed.
            let rows = statement.query_map(params![project, everything.all], file_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list that project's files", e))?;
    Ok(Json(
        list.into_iter()
            .filter(|f| sight.sees(f.folder.as_deref()))
            .collect(),
    )) // WHAT: only what they may see. WHY: a hidden folder's files are not there for them.
}

/// `GET /files/{id}/file`: the file's bytes, as they were sent.
pub async fn file_bytes(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Response> {
    let who = caller(&server, &headers)?; // WHAT: anybody signed in, or a key. WHY: reading.
    let file = may_reach_file(&server, &who, &id)?; // WHAT: the file, if theirs to see. WHY: not found otherwise.
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::DOWNLOADED_FILE, &who.email)
            .by(&who.id)
            .about(file.name.clone()),
    ); // WHAT: written down. WHY: reaching a file is the line an auditor wants.
    if let Some(have) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    {
        if have.trim_matches('"') == file.digest {
            return Ok(StatusCode::NOT_MODIFIED.into_response()); // WHAT: they have these bytes. WHY: nothing need move.
        }
    }
    let bytes = server
        .store
        .get_blob(&file.digest)
        .map_err(|e| Denied::broke("read that file", e))?; // WHAT: the bytes. WHY: sent back.
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()), // WHAT: plain bytes. WHY: the kind is told by its name; the server never guesses.
            (header::ETAG, format!("\"{}\"", file.digest)), // WHAT: its digest. WHY: a copy already held need not come again.
            (
                header::CONTENT_DISPOSITION,
                format!(
                    "attachment; filename=\"{}\"",
                    crate::api::safe_filename(&file.name)
                ),
            ), // WHAT: to be saved, under its name. WHY: never shown in a browser as if it were a page.
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()), // WHAT: don't guess its kind. WHY: a file somebody put up must never run as a page.
        ],
        Body::from(bytes),
    )
        .into_response())
}

#[derive(Deserialize)]
pub struct Moving {
    #[serde(default)]
    pub folder: Option<String>, // WHAT: where to. WHY: empty or null is the project's own.
}

/// `POST /files/{id}/folder` `{"folder": "…"}`: a file moved to another
/// folder of its project, with the files it replaced.
pub async fn move_file(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Moving>,
) -> Answer<Json<ProjectFile>> {
    let who = writer(&server, &headers)?; // WHAT: somebody who may change drawings. WHY: a move is a change.
    let file = may_reach_file(&server, &who, &id)?; // WHAT: the file, if theirs to see. WHY: not found otherwise.
    let folder = crate::projects::folders(body.folder.as_deref().unwrap_or(""), "A file's folder")
        .map_err(Denied::wrong)?; // WHAT: the new folder, checked. WHY: as a set's is.
    may_see_folder(&server, &who, &file.project, folder.as_deref(), "file")?; // WHAT: they may see where it goes. WHY: nobody moves a file somewhere they couldn't see.
    let by_program = who.is_key(auth::purpose::SYNC); // WHAT: a program's sync key. WHY: its new folders are the office's.
    let moved = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: one step. WHY: a file and its history move together.
            let db: &Connection = &tx;
            if let Some(folder) = &folder {
                make_folder(db, &file.project, folder, &who, by_program)?; // WHAT: the folder written down. WHY: as a file put there would.
            }
            let mut chain = vec![file.id.clone()]; // WHAT: this file and those it replaced. WHY: its history goes with it.
            let mut at = file.replaces.clone(); // WHAT: the one before. WHY: followed back.
            while let Some(id) = at.take().filter(|id| !chain.contains(id)) {
                at = db
                    .query_row(
                        "SELECT replaces FROM files WHERE id = ?1",
                        params![id],
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten(); // WHAT: the one before that. WHY: followed back to the first.
                chain.push(id); // WHAT: kept. WHY: moved.
            }
            for id in &chain {
                db.execute(
                    "UPDATE files SET folder = ?1 WHERE id = ?2",
                    params![folder, id],
                )?; // WHAT: moved. WHY: its history stays with it.
            }
            let moved = read_file(db, &file.id)?; // WHAT: the file as it is now. WHY: read inside the step, before it is committed.
            tx.commit()?; // WHAT: all at once. WHY: as above.
            Ok(moved)
        })
        .map_err(|e| Denied::broke("move that file", e))?
        .ok_or_else(|| Denied::missing("file"))?;
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::MOVED, &who.email)
            .by(&who.id)
            .about(moved.name.clone())
            .saying(moved.folder.clone().unwrap_or_default()),
    ); // WHAT: written down. WHY: a move can change who sees it.
    Ok(Json(moved))
}

/// `POST /sets/{id}/folder` `{"folder": "…"}`: a drawing set moved to another
/// folder of its project, with every issue of it before and after. Its
/// markups are untouched.
pub async fn move_set(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Moving>,
) -> Answer<Json<hub::DrawingSet>> {
    let who = writer(&server, &headers)?; // WHAT: somebody who may change drawings. WHY: a move is a change.
    may_reach_set(&server, &who, &id)?; // WHAT: the set's gate, its folder's included. WHY: not found otherwise.
    let set = server
        .store
        .with(|db| Ok(read_set(db, &id)?))
        .map_err(|e| Denied::broke("read that drawing set", e))?
        .ok_or_else(|| Denied::missing("drawing set"))?; // WHAT: the set. WHY: its project and history.
    let folder =
        crate::projects::folders(body.folder.as_deref().unwrap_or(""), "A drawing's folder")
            .map_err(Denied::wrong)?; // WHAT: the new folder, checked. WHY: one rule for folders.
    may_see_folder(
        &server,
        &who,
        &set.project,
        folder.as_deref(),
        "drawing set",
    )?; // WHAT: they may see where it goes. WHY: as for a file.
    let by_program = who.is_key(auth::purpose::SYNC); // WHAT: a program's sync key. WHY: its new folders are the office's.
    let moved = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: one step. WHY: a set and its issues move together.
            let db: &Connection = &tx;
            if let Some(folder) = &folder {
                make_folder(db, &set.project, folder, &who, by_program)?; // WHAT: the folder written down. WHY: as a file put there would.
            }
            let mut chain = vec![set.id.clone()]; // WHAT: this issue. WHY: and those before and after it.
            for column in ["supersedes", "superseded_by"] {
                let mut at = read_set(db, &set.id)?.and_then(|s| {
                    if column == "supersedes" {
                        s.supersedes
                    } else {
                        s.superseded_by
                    }
                }); // WHAT: the next issue that way. WHY: followed to the end.
                while let Some(next) = at.take().filter(|n| !chain.contains(n)) {
                    at = db
                        .query_row(
                            &format!("SELECT {column} FROM sets WHERE id = ?1"),
                            params![next],
                            |r| r.get::<_, Option<String>>(0),
                        )
                        .optional()?
                        .flatten(); // WHAT: the one after that. WHY: every issue of the drawing.
                    chain.push(next); // WHAT: kept. WHY: moved.
                }
            }
            for id in &chain {
                db.execute(
                    "UPDATE sets SET folder = ?1 WHERE id = ?2",
                    params![folder, id],
                )?; // WHAT: moved; markups are on the set and stay. WHY: the drawing keeps its takeoff wherever it is filed.
            }
            let moved = read_set(db, &set.id)?; // WHAT: the set as it is now. WHY: read inside the step, before it is committed.
            tx.commit()?; // WHAT: all at once. WHY: as above.
            Ok(moved)
        })
        .map_err(|e| Denied::broke("move that drawing set", e))?
        .ok_or_else(|| Denied::missing("drawing set"))?;
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::MOVED, &who.email)
            .by(&who.id)
            .about(moved.name.clone())
            .saying(moved.folder.clone().unwrap_or_default()),
    ); // WHAT: written down. WHY: a move can change who sees it.
    Ok(Json(moved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewer_in(groups: &[&str], rules: &[(&str, See)]) -> Sight {
        Sight {
            everything: false,
            groups: groups.iter().map(|g| g.to_lowercase()).collect(),
            rules: rules.iter().map(|(k, s)| (key_of(k), s.clone())).collect(),
        } // WHAT: a field hand's sight of one project. WHY: the rule tested without a server.
    }

    #[test]
    fn a_field_hand_sees_the_field_folders_and_not_the_offices() {
        let rules = [
            ("01 CONTRACT", See::Office),
            (
                "10 SHOP DRAWINGS",
                See::Groups(vec!["Field".into(), "Shop".into()]),
            ),
            ("10 SHOP DRAWINGS/10.9 PRICES", See::Office),
        ]; // WHAT: a job's rules. WHY: the shape the shop asked for.
        let field = viewer_in(&["field"], &rules); // WHAT: somebody in Field. WHY: the person the rules are for.
        assert!(!field.sees(Some("01 CONTRACT"))); // WHAT: the contract. WHY: the office's.
        assert!(!field.sees(Some("01 Contract/01.1 BID"))); // WHAT: inside it, any case. WHY: a child takes its parent's rule.
        assert!(field.sees(Some("10 shop drawings/10.4 FOR FABRICATION"))); // WHAT: the shop drawings. WHY: given to Field.
        assert!(!field.sees(Some("10 SHOP DRAWINGS/10.9 PRICES"))); // WHAT: a narrower child. WHY: its own rule.
        assert!(field.sees(Some("07 CONTRACT DRAWINGS"))); // WHAT: a folder with no rule. WHY: everybody's, as before.
        assert!(field.sees(None)); // WHAT: the project's own. WHY: everybody's.
        assert!(!viewer_in(&[], &rules).sees(Some("10 SHOP DRAWINGS"))); // WHAT: a viewer in no group. WHY: only Shop and Field.
        assert!(Sight::everything().sees(Some("01 CONTRACT"))); // WHAT: the office. WHY: sees everything.
    }

    #[test]
    fn a_child_given_everyone_inside_an_office_folder_is_still_the_offices() {
        let field = viewer_in(
            &["field"],
            &[
                ("01 CONTRACT", See::Office),
                ("01 CONTRACT/PUBLIC", See::Everyone),
            ],
        ); // WHAT: a wide child in a narrow parent. WHY: the rule a child can't break.
        assert!(!field.sees(Some("01 CONTRACT/PUBLIC"))); // WHAT: still hidden. WHY: never wider than its parent.
        assert_eq!(field.rule(Some("01 CONTRACT/PUBLIC")), See::Office); // WHAT: what decides. WHY: shown so in the Studio.
    }

    #[test]
    fn a_folder_is_one_folder_whatever_its_case_or_spacing() {
        assert_eq!(
            key_of("10  Shop Drawings/10.4 FOR FAB"),
            key_of("10 SHOP DRAWINGS/10.4  for fab")
        ); // WHAT: the same folder. WHY: as a share has it.
        assert_eq!(and_above("a/b/c"), vec!["a", "a/b", "a/b/c"]); // WHAT: every folder it is in. WHY: their rules apply to it.
    }

    #[test]
    fn a_rule_nobody_can_read_is_the_offices() {
        assert_eq!(from_stored("not json"), See::Office); // WHAT: damaged. WHY: never opens a folder.
        assert_eq!(from_stored(&stored(&See::Everyone)), See::Everyone); // WHAT: written and read back. WHY: the same rule.
    }
}
