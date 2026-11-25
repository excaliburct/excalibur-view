//! A job's crew, kept by its program (`GET` and `POST /projects/{id}/crew`).
//!
//! Fabwire decides who sees each of its jobs on the shop's and field's iPads
//! (its Decision 0412) and keeps the job's project people in step here. Its
//! sync key can't use `/projects/{id}/people`: that route is administrators'
//! alone and names people by View's own id, which a program never learns.
//! So the program gets this narrower route instead, by sign-in:
//!
//! - only Fabwire's own sync key, and only on a project it filed (one with a
//!   reference), refused "not found" otherwise, as every sync route is;
//! - a sign-in View has no person for is `404 no_such_person`;
//! - the last person is never taken off (`409 last_person`), checked and
//!   deleted in one statement so two removals at once can't both pass; and a
//!   Fabwire project left with nobody named anyway (its last person deleted)
//!   is the office's only, never the shop's or field's (`store::OPEN_WHEN_EMPTY`);
//! - every change is on the audit log as `project_person_added` and
//!   `project_person_removed`, naming the key.

use axum::extract::{Path, State}; // WHAT: the route's parts. WHY: which project, and the server
use axum::http::{HeaderMap, StatusCode}; // WHAT: headers and statuses. WHY: who is asking, and the refusals
use axum::Json; // WHAT: JSON in and out. WHY: as every route
use hub::Problem; // WHAT: a refusal's word and sentence. WHY: the program tells its refusals apart by the word
use rusqlite::{params, OptionalExtension}; // WHAT: the database. WHY: people are found by sign-in
use serde::{Deserialize, Serialize}; // WHAT: the body and the answer. WHY: JSON

use crate::api::{caller, may_reach_project, record, Answer, Denied, Shared}; // WHAT: the server's own gates and answers. WHY: the crew is held to the rules every sync route is
use crate::auth::{self, Who}; // WHAT: who is asking. WHY: only the program's key keeps a crew

/// The longest sign-in looked up.
const LONGEST_EMAIL: usize = 320; // WHAT: the longest an email address can be. WHY: anything longer is never somebody's sign-in, so it is refused before the database is asked

/// One person on a project, as the program reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CrewMember {
    pub email: String, // WHAT: their sign-in. WHY: how the program names people
    pub name: String,  // WHAT: their name. WHY: the program shows who it is
}

/// A person put on, or taken off, by sign-in.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)] // WHAT: a field not named here refused. WHY: a mistyped "on" is never read as its default
pub struct CrewChange {
    pub email: String, // WHAT: whose sign-in. WHY: the program knows people by it
    pub on: bool,      // WHAT: true puts them on, false takes them off. WHY: one route for both, as /people has
}

/// What one change did.
enum Changed {
    Already, // WHAT: it was already so. WHY: nothing written
    Done,    // WHAT: put on or taken off. WHY: audited
    Last,    // WHAT: refused: the last person on the project. WHY: 409 last_person
}

/// Refused unless the caller is Fabwire's own sync key on a project it filed.
fn the_program(server: &Shared, who: &Who, project: &str) -> Answer<()> {
    if !who.is_key(auth::purpose::SYNC) {
        return Err(Denied::new(
            StatusCode::FORBIDDEN,
            Problem::new(
                "program_only",
                "Only the office's program (Fabwire) keeps a job's crew here. An administrator uses the project's People.",
            ),
        )); // WHAT: a person or any other key, refused. WHY: people are put on by an administrator through /people; this route is the program's
    }
    if !crate::tablet::is_the_program(who) {
        return Err(Denied::new(
            StatusCode::FORBIDDEN,
            Problem::new(
                "program_only",
                "Only Fabwire's own key keeps a job's crew: a key for another program is refused.",
            ),
        )); // WHAT: a sync key made for another program, refused. WHY: who sees a job is the one program's to keep, as its tablet manifest is
    }
    may_reach_project(server, who, project) // WHAT: a project the program filed. WHY: a sync key never touches a job somebody made by hand; refused as "not found"
}

/// Who is on the project now, by sign-in, in the order they were put on.
fn crew_of(server: &Shared, project: &str) -> Answer<Vec<CrewMember>> {
    server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT people.email, people.name FROM project_people JOIN people ON people.id = project_people.person \
                 WHERE project_people.project = ?1 ORDER BY project_people.added, people.email",
            )?; // WHAT: each person on it, with their sign-in. WHY: the program compares sign-ins; the order is steady
            let rows = statement.query_map(params![project], |r| Ok(CrewMember { email: r.get(0)?, name: r.get(1)? }))?; // WHAT: read. WHY: answered
            Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?) // WHAT: all of them. WHY: one answer
        })
        .map_err(|e| Denied::broke("read who is on that project", e)) // WHAT: a database failure said plainly. WHY: never an empty list, which would read as "everybody"
}

/// `GET /projects/{id}/crew`: who is on one of the program's projects, `[{"email", "name"}]`. Empty means everybody.
///
/// # Errors
///
/// Refused with the problem the request earned: not signed in, not the program's key, not found, or a server error when the database can't be read.
pub async fn who(State(server): State<Shared>, headers: HeaderMap, Path(project): Path<String>) -> Answer<Json<Vec<CrewMember>>> {
    let who = caller(&server, &headers)?; // WHAT: who is asking. WHY: only the program
    the_program(&server, &who, &project)?; // WHAT: the program, on its own project. WHY: as above
    crew_of(&server, &project).map(Json) // WHAT: who is on it. WHY: the program puts on and takes off from this
}

/// `POST /projects/{id}/crew` `{"email", "on"}`: a sign-in put on one of the program's projects, or taken off. Answers who is on it now.
///
/// # Errors
///
/// Refused with the problem the request earned: not the program's key, not found (the project, or a person for the sign-in),
/// a bad request, the last person taken off, or a server error when the database can't be read.
pub async fn change(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Json(body): Json<CrewChange>,
) -> Answer<Json<Vec<CrewMember>>> {
    let who = caller(&server, &headers)?; // WHAT: who is asking. WHY: only the program
    the_program(&server, &who, &project)?; // WHAT: the program, on its own project. WHY: checked before anything else is looked at
    let email = body.email.trim().to_lowercase(); // WHAT: the sign-in, spaces and capitals aside. WHY: sign-ins are compared that way at sign-in
    if email.is_empty() || email.len() > LONGEST_EMAIL || !email.contains('@') {
        return Err(Denied::wrong("Say whose sign-in: an email address.")); // WHAT: not a sign-in. WHY: refused before the database is asked
    }
    let person = server
        .store
        .with(|db| {
            Ok(db
                .query_row("SELECT id FROM people WHERE lower(email) = ?1", params![email], |r| r.get::<_, String>(0))
                .optional()?)
        }) // WHAT: the person with that sign-in. WHY: project_people keeps View's own ids
        .map_err(|e| Denied::broke("find that person", e))?
        .ok_or_else(|| {
            Denied::new(
                StatusCode::NOT_FOUND,
                Problem::new("no_such_person", format!("Nobody on this server signs in as {email}.")),
            )
        })?; // WHAT: 404 with its own word. WHY: the program says it on the job's page and goes on with the rest
    let outcome = server
        .store
        .with(|db| {
            let there: bool = db.query_row("SELECT EXISTS (SELECT 1 FROM project_people WHERE project = ?1 AND person = ?2)", params![project, person], |r| r.get(0))?; // WHAT: whether they are on it already, under the database's one lock. WHY: read and changed in the same turn, so two requests never both act on a stale count
            if body.on == there {
                return Ok(Changed::Already); // WHAT: already so. WHY: the program may ask again after a failed answer; nothing is written twice
            }
            if body.on {
                db.execute(
                    "INSERT INTO project_people (project, person, added, added_by) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(project, person) DO NOTHING",
                    params![project, person, crate::audit::now(), who.id],
                )?; // WHAT: on. WHY: they see the job
                return Ok(Changed::Done);
            }
            let gone = db.execute(
                "DELETE FROM project_people WHERE project = ?1 AND person = ?2 \
                 AND (SELECT count(*) FROM project_people WHERE project = ?1) > 1",
                params![project, person],
            )?; // WHAT: off, only while someone else stays, in one statement. WHY: the count and the delete can't be split by a second removal (two at once used to empty a two-person job, the review of 0.8.0)
            Ok(if gone == 0 { Changed::Last } else { Changed::Done }) // WHAT: nothing deleted means they were the last. WHY: they were on it a moment ago, under the same lock
        })
        .map_err(|e| Denied::broke("change who is on that project", e))?;
    match outcome {
        Changed::Already => return crew_of(&server, &project).map(Json), // WHAT: the crew as it is. WHY: a repeat writes no audit line
        Changed::Last => {
            return Err(Denied::new(
                StatusCode::CONFLICT,
                Problem::new(
                    "last_person",
                    "That is the last person on this project. The program can't take the last one off: a job always has someone named.",
                ),
            )) // WHAT: 409. WHY: a program narrows who sees a job; it never leaves one with nobody named
        }
        Changed::Done => {} // WHAT: changed. WHY: written on the audit log below
    }
    let key = who.key.as_ref().map(|k| k.name.clone()).unwrap_or_default(); // WHAT: the key's name. WHY: the audit line names which program did it
    record(
        &server,
        &crate::audit::Entry::new(if body.on { "project_person_added" } else { "project_person_removed" }, &who.email)
            .by(&who.id)
            .about(project.clone())
            .saying(format!("{email}, by the program's key \"{key}\"")),
    ); // WHAT: every change on the audit log, as /people writes it. WHY: somebody will one day ask when a job stopped showing for them
    crew_of(&server, &project).map(Json) // WHAT: who is on it now. WHY: the program keeps what it was told
}
