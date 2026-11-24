//! The tablet manifest, kept, served and enforced (docs/TABLET-MANIFEST.md).
//!
//! Fabwire decides per job what the shop and the field see on their tablets
//! and sends it here as the job's manifest (`PUT /projects/{id}/tablet`).
//! Only a program's sync key may set it. From then on a tablet account (a
//! viewer) on that job is served only what the manifest lets its audience
//! reach, on top of the folder rules: the manifest can narrow what a person
//! sees, never widen it, because every item must still pass its folder.
//!
//! The narrowing itself lives in [`crate::folders::Sight`]: every route that
//! lists or reads a set or a file already goes through a person's sight, so
//! an item the manifest leaves out is "not found" everywhere at once.

use std::collections::HashSet; // WHAT: the ids a tablet account reaches. WHY: looked up once per item.

use axum::body::Body; // WHAT: the request's body. WHY: read only once the caller is known to be the program.
use axum::extract::{Path, State}; // WHAT: the route's parts. WHY: which project.
use axum::http::{HeaderMap, StatusCode}; // WHAT: headers and statuses. WHY: who is asking, and the refusals.
use axum::response::{IntoResponse, Response}; // WHAT: an answer. WHY: the PUT reads its body by hand.
use axum::Json; // WHAT: JSON answers. WHY: as every route.
use hub::tablet::{self, Audience, BoxLine, Manifest, TabletJob, TabletSaved, TabletView}; // WHAT: the manifest's shapes and rules. WHY: one definition with the app.
use hub::Problem; // WHAT: a refusal's word and sentence. WHY: the program-only refusal has its own word
use rusqlite::{params, Connection, OptionalExtension}; // WHAT: the database. WHY: manifests and their items live in it.
use sha2::Digest as _; // WHAT: SHA-256. WHY: the same manifest again is recognised by its digest.

use crate::api::{caller, may_reach_project, record, turned_away, Answer, Denied, Shared}; // WHAT: the server's own gates and answers. WHY: the manifest is held to the rules every route is.
use crate::auth::{self, Who}; // WHAT: who is asking. WHY: only the program sets it; a viewer is narrowed by it.

/// The most a manifest may be.
pub const TABLET_BODY: usize = 8 * 1024 * 1024; // WHAT: 8 MiB. WHY: five thousand sheets and a week of loads fit with room; read only after the gate.

/// What a key's name begins with when Fabwire made it (`Fabwire V2`), lower case.
const PROGRAM: &str = "fabwire"; // WHAT: the program's name. WHY: the one program whose key may set a manifest, named as Fabwire makes its key

/// What a tablet account may reach of one job: the sets and files its manifest lets its audience have.
#[derive(Clone, Debug, Default)]
pub struct Only {
    pub sets: HashSet<String>, // WHAT: the sets. WHY: everything else is not there for them.
    pub files: HashSet<String>, // WHAT: the files. WHY: likewise.
}

/// Whether a folder is, or is inside, `` `OUT OF USE (OFFICE ONLY) ``: where
/// Fabwire moves a superseded or unreleased sheet. Refused to tablet accounts
/// whatever any manifest says.
#[must_use] // WHAT: the answer is the point. WHY: dropping it is a mistake
pub fn out_of_use(folder: Option<&str>) -> bool {
    folder.is_some_and(|f| {
        f.split('/').any(|part| {
            part.trim()
                .trim_start_matches('`')
                .trim()
                .to_lowercase()
                .starts_with("out of use")
        }) // WHAT: any folder on its path named so, case and the backtick aside. WHY: at any depth, however it is written
    })
}

/// What a person in `groups` may reach of `project`, or `None` when the job
/// has no manifest (the folder rules alone decide, as before 0.8.0). Only
/// for tablet accounts: the caller never asks this for the office.
///
/// # Errors
///
/// Fails when the database can't be read; the caller refuses, never lets through.
pub fn only_for(
    db: &Connection,
    project: &str,
    groups: &[String],
) -> rusqlite::Result<Option<Only>> {
    let has: Option<i64> = db
        .query_row(
            "SELECT 1 FROM tablet_manifests WHERE project = ?1",
            params![project],
            |r| r.get(0),
        )
        .optional()?; // WHAT: whether the job has a manifest. WHY: none means no narrowing
    if has.is_none() {
        return Ok(None); // WHAT: no manifest. WHY: unchanged from before
    }
    let audience = Audience::of(groups); // WHAT: Shop, Field, both or neither. WHY: decides which items
    let mut statement = db.prepare("SELECT kind, id FROM tablet_items WHERE project = ?1 AND ((shop = 1 AND ?2) OR (field = 1 AND ?3))")?; // WHAT: the items for their audience. WHY: worked out when the manifest was set
    let rows = statement.query_map(params![project, audience.shop, audience.field], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?; // WHAT: each item. WHY: sorted into sets and files below
    let mut only = Only::default(); // WHAT: what they reach. WHY: filled below
    for row in rows {
        let (kind, id) = row?; // WHAT: one item. WHY: a database error is passed up, never read as "nothing"
        if kind == "set" {
            only.sets.insert(id); // WHAT: a set. WHY: reachable
        } else {
            only.files.insert(id); // WHAT: a file. WHY: reachable
        }
    }
    Ok(Some(only))
}

/// `PUT /projects/{id}/tablet`: the job's manifest, from Fabwire's sync key only.
pub async fn put(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    body: Body,
) -> Response {
    let gate = caller(&server, &headers).and_then(|who| {
        if !who.is_key(auth::purpose::SYNC) {
            return Err(Denied::new(
                StatusCode::FORBIDDEN,
                Problem::new(
                    "program_only",
                    "Only the office's program (Fabwire) sets what a job's tablets show.",
                ),
            )); // WHAT: a person or any other key, refused. WHY: only Fabwire's program key sets it (docs/TABLET-MANIFEST.md §2)
        }
        if !who.key.as_ref().is_some_and(|k| k.name.trim().to_lowercase().starts_with(PROGRAM)) {
            return Err(Denied::new(
                StatusCode::FORBIDDEN,
                Problem::new(
                    "program_only",
                    "Only Fabwire's own key sets what a job's tablets show: a key for another program is refused.",
                ),
            )); // WHAT: a sync key made for another program, refused. WHY: any administrator can make a sync key, but only the key Fabwire makes (named for it) may set a manifest
        }
        may_reach_project(&server, &who, &project).map(|()| who) // WHAT: a project the program keeps. WHY: a sync key never touches a job somebody made by hand
    });
    let who = match gate {
        Ok(who) => who, // WHAT: the program, on its own project. WHY: its body is read next
        Err(denied) => return turned_away(body, denied).await, // WHAT: refused, the body thrown away unread past a little. WHY: a stranger's body is never read
    };
    let bytes = match crate::api::body_once_let_in(body, TABLET_BODY).await {
        Ok(bytes) => bytes,                           // WHAT: the body. WHY: read below
        Err(denied) => return denied.into_response(), // WHAT: too big. WHY: said plainly
    };
    let mut manifest: Manifest = match serde_json::from_slice(&bytes) {
        Ok(m) => m, // WHAT: read. WHY: checked below
        Err(e) => {
            return Denied::wrong(format!(
                "unknown_field: That is not a tablet manifest (docs/TABLET-MANIFEST.md): {e}"
            ))
            .into_response()
        } // WHAT: refused with the contract's code. WHY: an unknown or mistyped field is never half-read
    };
    once(&mut manifest.sheets); // WHAT: a sheet listed twice kept once. WHY: the same sheet twice is one sheet on a tablet, and a flood of repeats is never stored
    once(&mut manifest.documents); // WHAT: a pinned document listed twice kept once. WHY: likewise
    once(&mut manifest.loads); // WHAT: a load listed twice kept once. WHY: likewise
    set(&server, &who, &project, &manifest)
        .map(Json)
        .into_response() // WHAT: checked and kept. WHY: apart, so it reads as one function
}

/// `items` with every exact repeat of an earlier item taken out, order kept.
fn once<T: serde::Serialize>(items: &mut Vec<T>) {
    let mut seen = HashSet::new(); // WHAT: each item as text, as met. WHY: an item met before is a repeat
    items.retain(|item| serde_json::to_string(item).map_or(true, |text| seen.insert(text))); // WHAT: only the first of each kept. WHY: an item that can't be written as text is kept, never dropped quietly
}

/// Checks a manifest against the project and keeps it when it changed.
fn set(server: &Shared, who: &Who, project: &str, manifest: &Manifest) -> Answer<TabletSaved> {
    tablet::validate(manifest).map_err(|e| Denied::wrong(e.to_string()))?; // WHAT: the contract's rules. WHY: "code: sentence", as both sides test
    let canonical =
        serde_json::to_string(manifest).map_err(|e| Denied::broke("write that manifest", e))?; // WHAT: the server's own form of it. WHY: the same manifest is the same bytes, whatever spacing it came in
    let digest = crate::store::hex(&sha2::Sha256::digest(canonical.as_bytes())); // WHAT: its SHA-256. WHY: a push that changes nothing is known at once
    let (shop, field) = (
        tablet::reaches(
            manifest,
            Audience {
                shop: true,
                field: false,
            },
        ),
        tablet::reaches(
            manifest,
            Audience {
                shop: false,
                field: true,
            },
        ),
    ); // WHAT: what each audience reaches. WHY: kept as items, read on every request
    let now = crate::api::now(); // WHAT: the time, once. WHY: kept as when it changed
    let saved = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: all or nothing. WHY: a manifest and its items are never seen half-changed
            for id in shop.0.iter().chain(&field.0) {
                ours(&tx, project, "set", id)?; // WHAT: each set is this job's, current and in use. WHY: a manifest can't point at another job's drawings
            }
            for id in shop.1.iter().chain(&field.1) {
                ours(&tx, project, "file", id)?; // WHAT: each file likewise. WHY: likewise
            }
            let kept: Option<(String, i64)> = tx.query_row("SELECT digest, version FROM tablet_manifests WHERE project = ?1", params![project], |r| Ok((r.get(0)?, r.get(1)?))).optional()?; // WHAT: the one kept, if any. WHY: compared
            if let Some((was, version)) = &kept {
                if *was == digest {
                    return Ok(TabletSaved { project: project.to_string(), version: *version, changed: false }); // WHAT: the same again. WHY: nothing written, no audit line; Fabwire may push every minute
                }
            }
            let version = kept.map_or(1, |(_, v)| v + 1); // WHAT: one more change. WHY: counted
            tx.execute(
                "INSERT INTO tablet_manifests (project, body, digest, version, updated, updated_by) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(project) DO UPDATE SET body = excluded.body, digest = excluded.digest, version = excluded.version, updated = excluded.updated, updated_by = excluded.updated_by",
                params![project, canonical, digest, version, now, who.email],
            )?; // WHAT: kept. WHY: served to the tablets from now on
            tx.execute("DELETE FROM tablet_items WHERE project = ?1", params![project])?; // WHAT: the old items gone. WHY: replaced whole, never merged
            let mut add = tx.prepare("INSERT INTO tablet_items (project, kind, id, shop, field) VALUES (?1, ?2, ?3, ?4, ?5)")?; // WHAT: one item. WHY: below
            for (kind, all, s, f) in [("set", shop.0.union(&field.0).collect::<Vec<_>>(), &shop.0, &field.0), ("file", shop.1.union(&field.1).collect::<Vec<_>>(), &shop.1, &field.1)] {
                for id in all {
                    add.execute(params![project, kind, id, s.contains(id), f.contains(id)])?; // WHAT: the item, and which audiences reach it. WHY: one row each
                }
            }
            drop(add); // WHAT: the statement let go. WHY: before the transaction ends
            tx.commit()?; // WHAT: done. WHY: all at once
            Ok(TabletSaved { project: project.to_string(), version, changed: true })
        })
        .map_err(|e| match e.downcast::<Refused>() {
            Ok(refused) => Denied::wrong(refused.0), // WHAT: an id that isn't this job's or is out of use. WHY: said with its code
            Err(e) => Denied::broke("keep that manifest", e), // WHAT: the database failed. WHY: said plainly
        })?;
    if saved.changed {
        let words = format!(
            "version {}: {} sheets, {} documents, {} loads; shop sees {}; field sees {}",
            saved.version,
            manifest.sheets.len(),
            manifest.documents.len(),
            manifest.loads.len(),
            sections(
                manifest,
                Audience {
                    shop: true,
                    field: false
                }
            ),
            sections(
                manifest,
                Audience {
                    shop: false,
                    field: true
                }
            )
        ); // WHAT: what the manifest now holds, in a line. WHY: somebody will ask when a sheet appeared on the field's tablets
        record(
            server,
            &crate::audit::Entry::new(crate::audit::action::TABLET_MANIFEST, &who.email)
                .by(&who.id)
                .about(format!("{} {}", manifest.job.number, project))
                .saying(words),
        ); // WHAT: every change on the audit log. WHY: who changed what a tablet shows, and when
    }
    Ok(saved)
}

/// The sections an audience gets, in words, for the audit line.
fn sections(m: &Manifest, audience: Audience) -> String {
    let on: Vec<&str> = tablet::SECTIONS
        .iter()
        .copied()
        .filter(|s| audience.sees_section(m, s))
        .collect(); // WHAT: each section on. WHY: listed
    on.join(", ") // WHAT: one line. WHY: read in the log
}

/// An id a manifest named that it may not: said with the contract's code.
#[derive(Debug)]
struct Refused(String);

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0) // WHAT: the sentence. WHY: as kept
    }
}

impl std::error::Error for Refused {}

/// Whether `id` is a current set (or file) of `project`, not in an out-of-use folder.
fn ours(db: &Connection, project: &str, kind: &str, id: &str) -> anyhow::Result<()> {
    let sql = if kind == "set" {
        "SELECT project, folder, superseded_by IS NOT NULL FROM sets WHERE id = ?1"
    } else {
        "SELECT project, folder, replaced_by IS NOT NULL FROM files WHERE id = ?1"
    }; // WHAT: its project, folder and whether a newer one replaced it. WHY: all three decide
    let found: Option<(String, Option<String>, bool)> = db
        .query_row(sql, params![id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .optional()?; // WHAT: the item, if there is one. WHY: below
    match found {
        Some((p, _, _)) if p != project => {
            Err(Refused(format!("not_in_project: {kind} {id} is not this job's.")).into())
        } // WHAT: another job's. WHY: a manifest can't reach past its own job
        None => Err(Refused(format!(
            "not_in_project: there is no {kind} {id} on this server."
        ))
        .into()), // WHAT: not there. WHY: likewise
        Some((_, folder, replaced)) if replaced || out_of_use(folder.as_deref()) => Err(Refused(
            format!("out_of_use: {kind} {id} has been replaced or is out of use."),
        )
        .into()), // WHAT: superseded, or put out of use. WHY: never a tablet's
        Some(_) => Ok(()), // WHAT: this job's and current. WHY: fine
    }
}

/// The manifest kept for a project, with its version and when it changed.
fn kept(server: &Shared, project: &str) -> Answer<Option<(Manifest, i64, String)>> {
    let row: Option<(String, i64, String)> = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT body, version, updated FROM tablet_manifests WHERE project = ?1",
                    params![project],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("read that job's tablet manifest", e))?; // WHAT: the row. WHY: read below
    let Some((body, version, updated)) = row else {
        return Ok(None);
    }; // WHAT: none kept. WHY: said by the caller
    let manifest = serde_json::from_str(&body)
        .map_err(|e| Denied::broke("read that job's tablet manifest", e))?; // WHAT: read back. WHY: it was written by this server, so a failure is a fault
    Ok(Some((manifest, version, updated)))
}

/// The person's groups, lower case.
pub(crate) fn groups_of(server: &Shared, who: &Who) -> Answer<Vec<String>> {
    server
        .store
        .with(|db| {
            let mut statement = db.prepare("SELECT g.name FROM groups g JOIN group_people m ON m.grp = g.id WHERE m.person = ?1")?; // WHAT: their groups. WHY: Shop and Field decide the audience
            let names = statement
                .query_map(params![who.id], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?; // WHAT: the names. WHY: compared case aside
            Ok(names)
        })
        .map_err(|e| Denied::broke("read your groups", e))
}

/// A manifest cut to what `who` may reach: its sections, sheets, documents,
/// loads and summaries, each item through the same sight every route uses.
fn as_seen(
    server: &Shared,
    who: &Who,
    project: &str,
    mut m: Manifest,
) -> Answer<(Manifest, Audience)> {
    let office = crate::folders::is_office(who); // WHAT: an administrator or estimator. WHY: the office sees the whole manifest, to check it
    let audience = if office {
        Audience {
            shop: true,
            field: true,
        }
    } else {
        Audience::of(&groups_of(server, who)?)
    }; // WHAT: the tablet's audience. WHY: decides the sections
    let sight = crate::folders::sight(server, project, who)?; // WHAT: what they may see, the manifest's narrowing included. WHY: one rule with every route
    let seen_set = |id: &str| -> Answer<bool> { set_seen(server, &sight, id) }; // WHAT: whether a set is theirs. WHY: below, for every set the manifest names
    if !office {
        let shop_on = |s: &String| audience.shop && tablet::SECTIONS.contains(&s.as_str()); // WHAT: kept when they're the shop. WHY: another audience's sections aren't theirs
        m.sections.shop.retain(shop_on); // WHAT: the shop's sections, for the shop. WHY: as above
        let field_on = |s: &String| audience.field && tablet::SECTIONS.contains(&s.as_str()); // WHAT: likewise for the field. WHY: as above
        m.sections.field.retain(field_on); // WHAT: the field's. WHY: as above
    }
    let mut sheets = Vec::with_capacity(m.sheets.len()); // WHAT: the sheets they reach. WHY: built below
    for sheet in std::mem::take(&mut m.sheets) {
        if seen_set(&sheet.set)? {
            sheets.push(sheet); // WHAT: kept. WHY: theirs
        }
    }
    m.sheets = sheets; // WHAT: only those. WHY: a tablet never lists what it can't open
    let shipping = audience.sees_section(&m, "shipping"); // WHAT: whether they get Shipping. WHY: the loads are only for them then
    let mut loads = Vec::with_capacity(m.loads.len()); // WHAT: the loads they reach. WHY: built below
    for mut load in std::mem::take(&mut m.loads) {
        if shipping && seen_set(&load.set)? {
            lines_onto(server, &mut load)?; // WHAT: each box's live line. WHY: signed a moment ago shows signed
            loads.push(load); // WHAT: kept. WHY: theirs
        }
    }
    m.loads = loads; // WHAT: only those. WHY: as sheets
    let mut documents = Vec::with_capacity(m.documents.len()); // WHAT: the pins they reach. WHY: built below
    for doc in std::mem::take(&mut m.documents) {
        let theirs = office || audience.sees_document(&m, &doc); // WHAT: pinned for them with Documents on. WHY: the office sees every pin
        let reachable = if doc.kind == "set" {
            seen_set(&doc.id)?
        } else {
            file_seen(server, &sight, &doc.id)?
        }; // WHAT: through the sight. WHY: the folder rules still decide
        if theirs && reachable {
            documents.push(doc); // WHAT: kept. WHY: theirs
        }
    }
    m.documents = documents; // WHAT: only those. WHY: as sheets
    if let Some(t) = m.traveler.take() {
        m.traveler = if audience.sees_section(&m, "traveler") && seen_set(&t.set)? {
            Some(t)
        } else {
            None
        }; // WHAT: kept when theirs. WHY: as sheets
    }
    if let Some(l) = m.loading.take() {
        m.loading = if audience.sees_section(&m, "loading") && seen_set(&l.set)? {
            Some(l)
        } else {
            None
        }; // WHAT: kept when theirs. WHY: as sheets
    }
    let sections_on = |s: &str| office || audience.sees_section(&m, s); // WHAT: whether a section is theirs. WHY: a to-do for another section isn't theirs
    let todo: Vec<_> = m
        .todo
        .iter()
        .filter(|t| sections_on(&t.section))
        .cloned()
        .collect(); // WHAT: their to-dos. WHY: as above
    m.todo = todo; // WHAT: only those. WHY: as above
    Ok((m, audience))
}

/// Whether a set passes this sight (folder rules and the manifest), and is current.
fn set_seen(server: &Shared, sight: &crate::folders::Sight, id: &str) -> Answer<bool> {
    let row: Option<(Option<String>, bool)> = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT folder, superseded_by IS NOT NULL FROM sets WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that drawing set", e))?; // WHAT: its folder and whether it was replaced. WHY: both decide
    Ok(row.is_some_and(|(folder, superseded)| sight.sees_set(folder.as_deref(), id, superseded)))
    // WHAT: seen or not. WHY: one rule
}

/// Whether a file passes this sight.
fn file_seen(server: &Shared, sight: &crate::folders::Sight, id: &str) -> Answer<bool> {
    let row: Option<(Option<String>, bool)> = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT folder, replaced_by IS NOT NULL FROM files WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that file", e))?; // WHAT: its folder and whether it was replaced. WHY: both decide
    Ok(row.is_some_and(|(folder, replaced)| sight.sees_file(folder.as_deref(), id, replaced)))
    // WHAT: seen or not. WHY: one rule
}

/// Each box's sign-off line as the server has it now, by its reference on the ticket's set.
fn lines_onto(server: &Shared, load: &mut tablet::Load) -> Answer<()> {
    for b in &mut load.boxes {
        if b.reference.is_empty() {
            continue; // WHAT: filled in the office. WHY: no line to sign
        }
        let line: Option<(String, Option<String>, Option<String>, bool)> = server
            .store
            .with(|db| Ok(db.query_row("SELECT id, signer, signed, withdrawn != 0 FROM signoffs WHERE set_id = ?1 AND ref = ?2", params![load.set, b.reference], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?))
            .map_err(|e| Denied::broke("read that ticket's sign-offs", e))?; // WHAT: the line. WHY: its state now
        b.line = line.map(|(id, signer, signed, withdrawn)| BoxLine {
            id,
            status: match (&signed, withdrawn) {
                (Some(_), _) => "signed", // WHAT: signed wins. WHY: a signature is never undone
                (None, true) => "withdrawn", // WHAT: taken back. WHY: not to sign
                (None, false) => "open",  // WHAT: waiting. WHY: to sign
            }
            .to_string(),
            signer: signer.unwrap_or_default(),
            signed: signed.unwrap_or_default(),
        }); // WHAT: the live line, or none when Fabwire hasn't asked it yet. WHY: the tablet signs only a line that is there
    }
    Ok(())
}

/// `GET /projects/{id}/tablet`: the manifest as this caller's tablet shows it.
///
/// # Errors
///
/// Not found when there is no manifest or the project is not theirs; a server error when the database fails.
pub async fn one(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
) -> Answer<Json<TabletView>> {
    let who = caller(&server, &headers)?; // WHAT: a person, or Fabwire's key. WHY: reading
    may_reach_project(&server, &who, &project)?; // WHAT: the project's gate. WHY: a job they aren't on isn't there
    let (manifest, version, updated) =
        kept(&server, &project)?.ok_or_else(|| Denied::missing("tablet manifest"))?; // WHAT: the one kept. WHY: none is not found
    if who.key.is_some() {
        return Ok(Json(TabletView {
            project,
            version,
            updated,
            shop: false,
            field: false,
            manifest,
        })); // WHAT: a key reads it as kept. WHY: Fabwire checks what it sent
    }
    let (manifest, audience) = as_seen(&server, &who, &project, manifest)?; // WHAT: cut to them. WHY: they are served only what they reach
    Ok(Json(TabletView {
        project,
        version,
        updated,
        shop: audience.shop,
        field: audience.field,
        manifest,
    }))
}

/// `GET /tablet`: the caller's jobs for the tablet, each with its card words.
///
/// # Errors
///
/// Refused for a key; a server error when the database fails.
pub async fn jobs(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<Vec<TabletJob>>> {
    let who = caller(&server, &headers)?; // WHAT: a person. WHY: their tablet
    if who.key.is_some() {
        return Err(Denied::not_with_this_key("list a person's tablet jobs")); // WHAT: a program has no tablet. WHY: it reads each manifest it set instead
    }
    let projects: Vec<String> = server
        .store
        .with(|db| {
            let mut statement = db.prepare("SELECT t.project FROM tablet_manifests t JOIN projects p ON p.id = t.project WHERE p.archived IS NULL ORDER BY p.number")?; // WHAT: every job with a manifest, not put away. WHY: a finished job drops off
            let rows = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?; // WHAT: their ids. WHY: each checked below
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the tablet jobs", e))?;
    let mut jobs = Vec::new(); // WHAT: their cards. WHY: built below
    for project in projects {
        if may_reach_project(&server, &who, &project).is_err() {
            continue; // WHAT: not on it. WHY: only the jobs assigned to them
        }
        let Some((manifest, _, updated)) = kept(&server, &project)? else {
            continue;
        }; // WHAT: its manifest. WHY: gone since the list was read
        if tablet::is_empty(&manifest) {
            continue; // WHAT: an empty manifest. WHY: Fabwire pushes one when the job leaves active, so its tablets drop it
        }
        let (m, audience) = as_seen(&server, &who, &project, manifest)?; // WHAT: cut to them. WHY: the card says only what's theirs
        if !crate::folders::is_office(&who) && !audience.shop && !audience.field {
            continue; // WHAT: neither shop nor field. WHY: nothing of it is theirs
        }
        jobs.push(TabletJob {
            project,
            number: m.job.number,
            name: m.job.name,
            town: m.job.town,
            shop: audience.shop,
            field: audience.field,
            todo: m.todo.into_iter().map(|t| t.words).collect(),
            updated,
        }); // WHAT: the card. WHY: the job list
    }
    Ok(Json(jobs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn out_of_use_is_found_at_any_depth_case_and_backtick_aside() {
        assert!(out_of_use(Some(
            "10 SHOP DRAWINGS/`OUT OF USE (OFFICE ONLY)"
        ))); // WHAT: as Fabwire names it. WHY: the usual case
        assert!(out_of_use(Some("`out of use (office only)/S-101"))); // WHAT: inside it, lower case. WHY: at any depth
        assert!(!out_of_use(Some("10 SHOP DRAWINGS/10.4 FOR FABRICATION"))); // WHAT: an ordinary folder. WHY: not refused
        assert!(!out_of_use(None)); // WHAT: the project's own. WHY: not refused
    }
}
