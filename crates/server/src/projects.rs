//! Jobs, as another program sees them.
//!
//! An estimating system that sends a bid's drawings here and reads the
//! takeoff back needs four things the viewer never asked for, and each of them
//! is here because the program this replaces got it wrong:
//!
//! - **The same bid is always the same project.** A project can carry a
//!   reference in the other program's terms (`fabwire:bid:2630`), and asking
//!   for a project with a reference that already has one answers with that
//!   one. A push retried after a dropped connection does not make a second job.
//! - **The same drawing is never stored twice.** An upload of bytes the project
//!   already holds answers with the set that holds them. Bluebeam's answer to a
//!   re-push was to delete the file and upload it again, which threw away every
//!   markup on it.
//! - **A drawing issued again replaces the one before, and the takeoff comes
//!   with it.** The new issue supersedes the old; the old is kept, markups and
//!   all. Asked to, the markups are brought forward onto the new issue — sheet
//!   for sheet, only where the sheet is the same size — each one marked
//!   *Carried forward — check* so nobody mistakes a quantity measured on the
//!   last revision for one measured on this.
//! - **A finished bid is put away, not destroyed.** Archiving takes a project
//!   off the list and changes nothing else. It can be brought back.
//!
//! And one thing for keeping up: a stamp that changes whenever anything that
//! would change the takeoff does, so a program can tell "nothing new" from
//! "pull again" without pulling.

use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Json;
use hub::{DrawingSet, NewProject, Project, Sheet};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::api::{
    caller, fresh_id, now, set_from_row, sheets_for, writer, Answer, Denied, Everything, Shared,
    SET_COLUMNS,
};
use crate::store::Store;

/// What a markup brought forward from the issue before says about itself,
/// in the status column every markup list already shows.
pub const CARRIED: &str = "Carried forward — check";

/// The longest reference a program may file a project under.
const LONGEST_REFERENCE: usize = 200;

/// The longest place or folder, all of it, and the most folders deep.
const LONGEST_PLACE: usize = 260;
const DEEPEST_PLACE: usize = 12;

const PROJECT_COLUMNS: &str = "p.id, p.number, p.name, p.created,
     (SELECT count(*) FROM sets WHERE sets.project = p.id AND sets.superseded_by IS NULL),
     p.reference, p.archived, p.place";

/// A place in the Studio's list, or a drawing's folder, as folders written
/// with `/` (a `\` is taken as one): each folder's name trimmed, none empty,
/// none `.` or `..`, nothing that is not printable. Empty is none: the
/// project's own, as before places. `what` names it in the refusal.
pub fn folders(text: &str, what: &str) -> Result<Option<String>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(['/', '\\']).map(str::trim).collect();
    if parts.iter().any(|p| p.is_empty() || *p == "." || *p == "..") {
        return Err(format!(
            "{what} is folders' names between slashes, none of them empty: \"{text}\" isn't."
        ));
    }
    if parts.iter().any(|p| p.chars().any(char::is_control)) {
        return Err(format!("{what} can't hold a control character."));
    }
    let joined = parts.join("/");
    if joined.chars().count() > LONGEST_PLACE || parts.len() > DEEPEST_PLACE {
        return Err(format!(
            "{what} can be at most {LONGEST_PLACE} characters and {DEEPEST_PLACE} folders deep."
        ));
    }
    Ok(Some(joined))
}

fn project_from_row(r: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: r.get(0)?,
        number: r.get(1)?,
        name: r.get(2)?,
        created: r.get(3)?,
        sets: r.get::<_, i64>(4)? as usize,
        reference: r.get(5)?,
        archived: r.get(6)?,
        place: r.get(7)?,
    })
}

pub(crate) fn read_project(db: &Connection, id: &str) -> rusqlite::Result<Option<Project>> {
    db.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects p WHERE p.id = ?1"),
        params![id],
        project_from_row,
    )
    .optional()
}

fn by_reference(db: &Connection, reference: &str) -> rusqlite::Result<Option<Project>> {
    db.query_row(
        &format!("SELECT {PROJECT_COLUMNS} FROM projects p WHERE p.reference = ?1"),
        params![reference],
        project_from_row,
    )
    .optional()
}

// ---- the routes -----------------------------------------------------------

/// `GET /projects` — the jobs, archived ones left off unless `?all=1`.
pub async fn list(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(everything): Query<Everything>,
) -> Answer<Json<Vec<Project>>> {
    let who = caller(&server, &headers)?;
    let everybodys = who.role == hub::Role::Admin;
    let list = server
        .store
        .with(|db| {
            // A project with nobody named on it is everybody's, which is how
            // every server worked before this and how most shops want it.
            // Once one name goes on, it is for the people named on it.
            // Administrators see the lot: somebody has to be able to find a
            // job that the only person on it left the company over.
            let mut statement = db.prepare(&format!(
                "SELECT {PROJECT_COLUMNS} FROM projects p
                 WHERE (?1 OR p.archived IS NULL)
                   AND (?2
                        OR NOT EXISTS (SELECT 1 FROM project_people m WHERE m.project = p.id)
                        OR EXISTS (SELECT 1 FROM project_people m
                                   WHERE m.project = p.id AND m.person = ?3))
                 ORDER BY p.number"
            ))?;
            let mut rows = statement
                .query_map(params![everything.all, everybodys, who.id], project_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            if !crate::folders::is_office(&who) {
                for project in &mut rows {
                    let sight = crate::folders::sight_of(db, &project.id, &who)?; // WHAT: what this person sees of the project. WHY: its count of sets counts only those.
                    project.sets = crate::folders::current_sets_seen(db, &project.id, &sight)?; // WHAT: the sets they can open. WHY: a hidden folder's sets are not even counted for them.
                }
            }
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the projects", e))?;
    Ok(Json(list))
}

/// `POST /projects` — a new job, or, when it carries a reference that already
/// has one, that one: brought back if it had been archived, and renamed if the
/// other program has renamed it.
pub async fn create(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewProject>,
) -> Answer<Json<Project>> {
    let who = writer(&server, &headers)?; // WHAT: who is asking, if they may change drawings. WHY: kept, so the project found by its reference is checked against them below.
    let number = body.number.trim().to_string();
    let name = body.name.trim().to_string();
    if number.is_empty() {
        return Err(Denied::wrong("A project needs a job number."));
    }
    let reference = body
        .reference
        .as_deref()
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_string);
    if reference.is_none() && who.is_key(crate::auth::purpose::SYNC) {
        // WHAT: a sync key files every project under its own reference. WHY: a project without one is a hand-made job, which a sync key can neither make nor reach.
        return Err(Denied::wrong("A program keeping its projects in step files each one under its own reference."));
    }
    if reference.as_ref().is_some_and(|r| r.chars().count() > LONGEST_REFERENCE) {
        return Err(Denied::wrong(format!(
            "A project's reference can be at most {LONGEST_REFERENCE} characters."
        )));
    }
    if reference.is_some() && who.role != hub::Role::Admin && !who.is_key(crate::auth::purpose::SYNC) {
        // WHAT: filing under a program's reference is for an administrator (signed in, or a key they made) or a sync key, before anything is looked up. WHY: those reach every program's project; anybody else would learn from the answer whether a reference (`fabwire:bid:2630`) exists.
        return Err(Denied::forbidden("file a project under another program's reference"));
    }
    // Asked for with a place, it goes (or moves) there; without one, a new
    // project is with the others and one already there stays where it is.
    let place = match body.place.as_deref() {
        None => None,
        Some(text) => Some(folders(text, "A project's place").map_err(Denied::wrong)?),
    };
    if let Some(reference) = &reference {
        let there = server
            .store
            .with(|db| Ok(by_reference(db, reference)?)) // WHAT: the project already filed under this reference, if any. WHY: renaming or moving it is reaching it.
            .map_err(|e| Denied::broke("find that project", e))?;
        if let Some(there) = there {
            // WHAT: the same project gate as every other route. WHY: asking by a reference must not reach a job its id would be refused.
            crate::api::may_reach_project(&server, &who, &there.id)?;
        }
    }
    let project = server
        .store
        .with(|db| {
            if let Some(reference) = &reference {
                if let Some(there) = by_reference(db, reference)? {
                    db.execute(
                        "UPDATE projects SET archived = NULL, number = ?1,
                                name = CASE WHEN ?2 = '' THEN name ELSE ?2 END,
                                place = CASE WHEN ?4 THEN ?5 ELSE place END
                         WHERE id = ?3",
                        params![number, name, there.id, place.is_some(), place.clone().flatten()],
                    )?;
                    return Ok(read_project(db, &there.id)?.unwrap_or(there));
                }
            }
            let id = fresh_id("prj");
            db.execute(
                "INSERT INTO projects (id, number, name, created, reference, place)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, number, name, now(), reference, place.flatten()],
            )?;
            read_project(db, &id)?.ok_or_else(|| anyhow::anyhow!("the project just made is not there"))
        })
        .map_err(|e| Denied::broke("create that project", e))?;
    Ok(Json(project))
}

/// `GET /projects/{id}`.
pub async fn one(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<Project>> {
    let who = caller(&server, &headers)?;
    // Checked here as well as in the listing, and with the same rule. A
    // project somebody cannot see in a list but can open by guessing its id
    // is not access control, it is a tidier list -- and the answer is the
    // same "not found" either way, so nobody learns a project exists by
    // being refused it.
    let mine = server
        .store
        .may_see(&id, &who.id, who.role == hub::Role::Admin)
        .map_err(|e| Denied::broke("read that project", e))?;
    if !mine {
        return Err(Denied::missing("project"));
    }
    server
        .store
        .with(|db| {
            let Some(mut project) = read_project(db, &id)? else { return Ok(None) }; // WHAT: the project. WHY: not found otherwise.
            if !crate::folders::is_office(&who) {
                let sight = crate::folders::sight_of(db, &id, &who)?; // WHAT: what this person sees of it. WHY: the count of sets is theirs.
                project.sets = crate::folders::current_sets_seen(db, &id, &sight)?; // WHAT: only the sets they can open. WHY: as the list says.
            }
            Ok(Some(project))
        })
        .map_err(|e| Denied::broke("read that project", e))?
        .map(Json)
        .ok_or_else(|| Denied::missing("project"))
}

/// `GET /projects/:id/people` — who is on it. Empty means everybody.
pub async fn who_is_on(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<Vec<String>>> {
    let who = caller(&server, &headers)?;
    let mine = server
        .store
        .may_see(&id, &who.id, who.role == hub::Role::Admin)
        .map_err(|e| Denied::broke("read that project", e))?;
    if !mine {
        return Err(Denied::missing("project"));
    }
    server
        .store
        .people_on(&id)
        .map(Json)
        .map_err(|e| Denied::broke("read who is on that project", e))
}

#[derive(Deserialize)]
pub struct WhoOnProject {
    pub person: String,
    /// True puts them on, false takes them off.
    #[serde(default = "yes")]
    pub on: bool,
}

/// `POST /projects/:id/people` — put somebody on a job, or take them off.
///
/// Administrators only. Deciding who sees which job is the sort of thing that
/// should not be doable by whoever happens to be on the job already.
pub async fn change_who_is_on(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<WhoOnProject>,
) -> Answer<Json<Vec<String>>> {
    let who = crate::api::administrator(&server, &headers)?;
    let person = body.person.trim().to_string();
    if person.is_empty() {
        return Err(Denied::wrong("Say who."));
    }
    if body.on {
        server
            .store
            .put_on_project(&id, &person, &who.id)
            .map_err(|e| Denied::broke("put somebody on that project", e))?;
    } else {
        server
            .store
            .take_off_project(&id, &person)
            .map_err(|e| Denied::broke("take somebody off that project", e))?;
    }
    let now = server
        .store
        .people_on(&id)
        .map_err(|e| Denied::broke("read who is on that project", e))?;
    // Worth a line, because this decides who can see a job and somebody will
    // one day ask when a project stopped appearing for them.
    let _ = server.store.audit(
        &crate::audit::Entry::new(
            if body.on { "project_person_added" } else { "project_person_removed" },
            &who.email,
        )
        .by(&who.id)
        .about(id.clone())
        .saying(person.clone()),
    );
    if now.is_empty() {
        tracing::info!("{id} has nobody named on it now, so it is everybody's again");
    }
    Ok(Json(now))
}

#[derive(Deserialize)]
pub struct Placing {
    /// Folders written with `/`; empty or null puts it back with the others.
    #[serde(default)]
    pub place: Option<String>,
}

/// `POST /projects/{id}/place` — moves it in the Studio's list: a bid won
/// goes to `Active Jobs`, a lost one to `Lost Bids/2026`. Its drawings,
/// markups and takeoff are as they were.
pub async fn place(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Placing>,
) -> Answer<Json<Project>> {
    let who = writer(&server, &headers)?;
    crate::api::may_reach_project(&server, &who, &id)?;
    let place = folders(body.place.as_deref().unwrap_or(""), "A project's place")
        .map_err(Denied::wrong)?;
    server
        .store
        .with(|db| {
            let changed =
                db.execute("UPDATE projects SET place = ?1 WHERE id = ?2", params![place, id])?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(read_project(db, &id)?)
        })
        .map_err(|e| Denied::broke("move that project", e))?
        .map(Json)
        .ok_or_else(|| Denied::missing("project"))
}

#[derive(Deserialize)]
pub struct Archiving {
    #[serde(default = "yes")]
    pub archived: bool,
}

fn yes() -> bool {
    true
}

/// `POST /projects/{id}/archive` — `{"archived": false}` brings it back.
pub async fn archive(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Archiving>,
) -> Answer<Json<Project>> {
    let who = writer(&server, &headers)?;
    crate::api::may_reach_project(&server, &who, &id)?;
    server
        .store
        .with(|db| {
            let when = body.archived.then(now);
            let changed = db.execute(
                "UPDATE projects SET archived = ?1 WHERE id = ?2",
                params![when, id],
            )?;
            if changed == 0 {
                return Ok(None);
            }
            Ok(read_project(db, &id)?)
        })
        .map_err(|e| Denied::broke("archive that project", e))?
        .map(Json)
        .ok_or_else(|| Denied::missing("project"))
}

/// What has changed on a project, without reading it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Stamp {
    pub project: String,
    /// Changes whenever anything that would change the takeoff does: a markup
    /// drawn, moved or deleted, a drawing added or issued again.
    pub stamp: String,
    /// Drawing sets that count — not the ones issued again since.
    pub sets: usize,
    pub superseded: usize,
    /// Markups on the sets that count.
    pub markups: usize,
    /// Of those, how many were brought forward from an earlier issue.
    pub carried_forward: usize,
    pub archived: Option<String>,
}

/// `GET /projects/{id}/stamp`.
pub async fn stamp(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<Stamp>> {
    let who = caller(&server, &headers)?;
    crate::api::may_reach_project(&server, &who, &id)?;
    let sight = crate::folders::sight(&server, &id, &who)?; // WHAT: which folders this caller sees. WHY: a change in a hidden folder is not a change for them.
    server
        .store
        .with(|db| stamp_seen(db, &id, &sight))
        .map_err(|e| Denied::broke("read that project", e))?
        .map(Json)
        .ok_or_else(|| Denied::missing("project"))
}

/// The stamp of what `sight` sees of a project: a set in a folder hidden from
/// it changes nothing and counts for nothing.
pub(crate) fn stamp_seen(db: &Connection, id: &str, sight: &crate::folders::Sight) -> anyhow::Result<Option<Stamp>> {
    let Some(project) = read_project(db, id)? else {
        return Ok(None);
    };
    let mut statement = db.prepare(
        "SELECT id, revision, superseded_by IS NOT NULL, folder FROM sets WHERE project = ?1 ORDER BY id",
    )?; // WHAT: each set with its folder. WHY: the folder decides whether it counts here.
    let sets: Vec<(String, i64, bool)> = statement
        .query_map(params![id], |r| Ok(((r.get(0)?, r.get(1)?, r.get(2)?), r.get::<_, Option<String>>(3)?)))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|(_, folder)| sight.sees(folder.as_deref())) // WHAT: only the sets this sight sees. WHY: a hidden folder must not show in a count or a change.
        .map(|(set, _)| set)
        .collect();
    let current: Vec<&(String, i64, bool)> = sets.iter().filter(|s| !s.2).collect();
    let (mut markups, mut carried) = (0i64, 0i64); // WHAT: the markups on the current sets seen. WHY: added up set by set.
    for (set, _, _) in &current {
        let (here, brought): (i64, i64) = db.query_row(
            "SELECT count(*), count(carried_from) FROM markups WHERE set_id = ?1 AND removed = 0",
            params![set],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?; // WHAT: this set's markups, and how many were carried forward. WHY: the same counts as before, over only what is seen.
        markups += here; // WHAT: added. WHY: the project's total.
        carried += brought; // WHAT: added. WHY: likewise.
    }
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    for (set, revision, _) in &current {
        hasher.update(format!("{set}:{revision};").as_bytes());
    }
    let stamp = crate::store::hex(&hasher.finalize()[..8]);
    Ok(Some(Stamp {
        project: project.id,
        stamp,
        sets: current.len(),
        superseded: sets.len() - current.len(),
        markups: markups as usize,
        carried_forward: carried as usize,
        archived: project.archived,
    }))
}

// ---- drawings arriving ----------------------------------------------------

/// A set in this project holding exactly these bytes, if there is one. The
/// current one is preferred to one it replaced.
pub(crate) fn same_drawing(
    db: &Connection,
    project: &str,
    digest: &str,
) -> rusqlite::Result<Option<DrawingSet>> {
    db.query_row(
        &format!(
            "SELECT {SET_COLUMNS} FROM sets WHERE project = ?1 AND digest = ?2
             ORDER BY superseded_by IS NOT NULL, uploaded DESC LIMIT 1"
        ),
        params![project, digest],
        set_from_row,
    )
    .optional()
}

/// Whether two folders are the same folder: case and spacing aside, as a
/// share's folders are; none is only none.
pub fn same_folder(a: Option<&str>, b: Option<&str>) -> bool {
    fn plain(f: &str) -> Vec<String> {
        f.split('/')
            .map(|p| p.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
            .collect()
    }
    match (a, b) {
        (None, None) => true,
        (Some(a), Some(b)) => plain(a) == plain(b),
        _ => false,
    }
}

/// How two names for a drawing are compared: case, spacing and a trailing
/// `.pdf` do not make it a different drawing.
pub fn same_name(a: &str, b: &str) -> bool {
    fn plain(n: &str) -> String {
        let n = n.trim().to_lowercase();
        let n = n.strip_suffix(".pdf").unwrap_or(&n).trim().to_string();
        n.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    plain(a) == plain(b)
}

/// The current set this project already has under the same name in the same
/// folder: the issue a new upload of that drawing replaces. A drawing of the
/// same name in another folder (`10.1 FOR APPROVAL` and `10.4 FOR
/// FABRICATION`) is another drawing. A drawing sent from a folder with none
/// there replaces the one of its name sent before folders were, so a takeoff
/// made on that one comes with it.
pub(crate) fn issued_before(
    db: &Connection,
    project: &str,
    name: &str,
    folder: Option<&str>,
) -> rusqlite::Result<Option<DrawingSet>> {
    let mut statement = db.prepare(&format!(
        "SELECT {SET_COLUMNS} FROM sets WHERE project = ?1 AND superseded_by IS NULL
         ORDER BY uploaded DESC"
    ))?;
    let current = statement
        .query_map(params![project], set_from_row)?
        .collect::<Result<Vec<_>, _>>()?;
    let found = current
        .iter()
        .find(|s| same_name(&s.name, name) && same_folder(s.folder.as_deref(), folder))
        .cloned();
    if found.is_some() {
        return Ok(found); // WHAT: the same drawing in the same folder. WHY: that one it replaces.
    }
    let Some(folder) = folder else {
        return Ok(None); // WHAT: sent outside every folder, and none there. WHY: nothing to replace.
    };
    if !crate::folders::everybodys(db, project, folder)? {
        return Ok(None); // WHAT: a folder not everybody sees. WHY: the old issue outside every folder is everybody's, so pointing it at this one would tell a field hand a newer drawing is hidden from them.
    }
    Ok(current
        .iter()
        .find(|s| s.folder.is_none() && same_name(&s.name, name))
        .cloned()) // WHAT: the same drawing sent before folders were. WHY: an office that starts using folders keeps its issues in a line.
}

/// Whether a sheet is the same sheet of paper as another: the same size, the
/// same way up. A markup is only brought across between two of those, because
/// anywhere else its coordinates are somewhere else on the drawing.
fn same_paper(a: &Sheet, b: &Sheet) -> bool {
    (a.width - b.width).abs() <= 1.0 && (a.height - b.height).abs() <= 1.0 && a.rotation == b.rotation
}

/// Brings the markups on `from` across to `to`, the issue that replaced it.
///
/// Sheet for sheet, and only when the two issues have the same number of
/// sheets: with a sheet added or taken out there is no telling which is
/// which, and a takeoff put on the wrong sheet is worse than one that has to
/// be carried across by hand. A sheet whose size changed keeps its markups
/// where they were. Everything brought across says so in its status.
///
/// Returns how many came across, and, when some did not, why.
pub(crate) fn carry_forward(
    db: &Connection,
    from: &str,
    to: &str,
    to_sheets: &[Sheet],
) -> anyhow::Result<(usize, Option<String>)> {
    let from_sheets = sheets_for(db, from)?;
    let mut statement = db.prepare(
        "SELECT id, page, subject, kind, caption, author, colour, dictionary
         FROM markups WHERE set_id = ?1 AND removed = 0 ORDER BY revision, id",
    )?;
    type Old = (String, i64, String, String, String, String, String, Vec<u8>);
    let old: Vec<Old> = statement
        .query_map(params![from], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if old.is_empty() {
        return Ok((0, None));
    }
    if from_sheets.len() != to_sheets.len() {
        return Ok((
            0,
            Some(format!(
                "The new issue has {} sheets and the one before had {}, so which sheet became \
                 which cannot be told for certain. None of the {} markups were brought forward: \
                 open both issues in Excalibur View to carry the takeoff across by hand.",
                to_sheets.len(),
                from_sheets.len(),
                old.len()
            )),
        ));
    }
    let revision = Store::bump(db, to)?;
    let when = now();
    let mut carried = 0usize;
    let mut left: Vec<usize> = Vec::new();
    for (id, page, subject, kind, caption, author, colour, dictionary) in old {
        let page_index = page as usize;
        let paper_matches = match (from_sheets.get(page_index), to_sheets.get(page_index)) {
            (Some(a), Some(b)) => same_paper(a, b),
            _ => false,
        };
        let markup = paper_matches
            .then(|| crate::quantities::read_markup(&dictionary))
            .flatten();
        let Some(mut markup) = markup else {
            left.push(page_index);
            continue;
        };
        markup.set("BSIStatus", pdf::Object::text(CARRIED));
        let bytes = crate::quantities::write_markup(&markup);
        db.execute(
            "INSERT INTO markups (id, set_id, page, subject, kind, caption, author, colour,
                                  created, dictionary, revision, removed, carried_from)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0, ?12)",
            params![
                fresh_id("mk"),
                to,
                page,
                subject,
                kind,
                caption,
                author,
                colour,
                when,
                bytes,
                revision as i64,
                id
            ],
        )?;
        carried += 1;
    }
    let note = if left.is_empty() {
        None
    } else {
        let mut pages: Vec<usize> = left.clone();
        pages.sort_unstable();
        pages.dedup();
        let named: Vec<String> = pages.iter().map(|p| format!("{}", p + 1)).collect();
        Some(format!(
            "{} markup(s) stayed on the earlier issue because the sheet changed size or could not \
             be read (page {}). Carry those across by hand.",
            left.len(),
            named.join(", ")
        ))
    };
    Ok((carried, note))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_drawing_is_the_same_drawing_whatever_case_or_extension_it_arrives_in() {
        assert!(same_name("S-101 Framing Plan.pdf", "s-101  framing plan"));
        assert!(same_name(" S-101.PDF", "S-101"));
        assert!(!same_name("S-101", "S-102"));
        assert!(!same_name("S-101 Rev 2", "S-101"));
    }

    #[test]
    fn a_sheet_that_changed_size_is_not_the_same_paper() {
        let sheet = |w: f64, h: f64, r: i32| Sheet {
            page: 0,
            number: String::new(),
            title: String::new(),
            width: w,
            height: h,
            rotation: r,
            scale: None,
        };
        assert!(same_paper(&sheet(2592.0, 1728.0, 0), &sheet(2592.4, 1727.8, 0)));
        assert!(!same_paper(&sheet(2592.0, 1728.0, 0), &sheet(3024.0, 2160.0, 0)));
        assert!(!same_paper(&sheet(2592.0, 1728.0, 0), &sheet(2592.0, 1728.0, 90)));
    }
}
