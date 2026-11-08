//! Sign-offs: signatures asked for on a drawing set, signed on the shop's
//! iPads.
//!
//! A program that keeps the shop's paperwork (Fabwire) puts a traveler or a
//! shipping ticket up as a drawing set, and asks for the sign-offs on it: one
//! line each, in its own words ("B12 · WELD", "Load 1 · Received by"), each
//! with the program's own reference so it knows which box the signature is
//! for. Whoever may see the set opens it in View, picks a line, signs it with
//! a finger, and the program reads the signatures back and fills its own form.
//!
//! The rules that keep it honest:
//!
//! - **Only a person signs.** A key (a program's or an assistant's) can ask
//!   for sign-offs and read them back, but never sign one: a signature that
//!   a program could make is worth nothing.
//! - **A signature is final.** Signed once, a line stays signed; signing it
//!   again is refused, and a signed line can't be withdrawn. Who signed (their
//!   account and the name they wrote), when, and the strokes are kept.
//! - **Seeing the set is signing it.** Whoever may see the set (its project
//!   and its folder) may sign its lines: the shop people are viewers, and the
//!   folder rules say which shop sees which job's papers.
//! - **Every signature goes on the audit log.**

use axum::body::Body; // WHAT: a request's body, read only after the gate. WHY: a stranger's upload is never read.
use axum::extract::{Path, Query, State}; // WHAT: the route's parts. WHY: which set or line, and since when.
use axum::http::{header, HeaderMap, StatusCode}; // WHAT: headers and status codes. WHY: who is asking, and the picture's type.
use axum::response::{IntoResponse, Response}; // WHAT: an answer. WHY: the picture is bytes, not JSON.
use axum::Json; // WHAT: JSON answers. WHY: every other answer is JSON.
use rusqlite::{params, OptionalExtension}; // WHAT: bound parameters, and a row that may not be there. WHY: no SQL is ever built from text.
use serde::{Deserialize, Serialize}; // WHAT: the wire's shapes. WHY: requests in, answers out.

use crate::api::{
    caller, json_once_let_in, may_reach_set, record, sharing, turned_away, writer, Answer, Denied,
    Shared,
}; // WHAT: the server's own gates and answers. WHY: sign-offs are held to exactly the rules every other route is.

/// The most lines one request may ask for (a big job's traveler is a few hundred marks times six stations).
pub const MOST_ASKED: usize = 5000; // WHAT: the limit. WHY: room for any traveler, none for a flood.
/// How big a request may be, read only after the gate.
const ASK_BODY: usize = 4 * 1024 * 1024; // WHAT: 4 MiB. WHY: 5,000 short lines fit with room; a signature fits many times over.
/// The signing pad's size, in its own units: every stroke point is inside it.
pub const PAD: (f64, f64) = (400.0, 140.0); // WHAT: 400 by 140. WHY: the pad Fabwire's proposals already use; the picture comes out this size.
/// The most strokes, and points in all of them, one signature may have.
const MOST_STROKES: usize = 64; // WHAT: strokes. WHY: a long signature is a dozen; this is far past any.
const MOST_POINTS: usize = 8000; // WHAT: points. WHY: a finger sends a few hundred a second; a signature is a few seconds.
/// The most lines one page of changes gives back.
const CHANGES_PAGE: usize = 500; // WHAT: a page. WHY: a program catching up asks again with the last one's number.

/// One line asked for: what it says, how lines are grouped, and the asker's reference.
#[derive(Deserialize)]
pub struct Ask {
    pub label: String, // WHAT: what the line says ("B12 · WELD"). WHY: the person signing must see what they sign for.
    #[serde(default)]
    pub group: String, // WHAT: a heading lines go under ("Stair 1"). WHY: a long traveler reads by page.
    #[serde(rename = "ref")]
    pub reference: String, // WHAT: the asker's own name for the box. WHY: how the signature finds its way back; never shown.
    #[serde(default)]
    pub page: Option<u32>, // WHAT: the page it's on, if the asker knows. WHY: View can open there.
}

/// A sign-off line as the wire gives it.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SignOff {
    pub id: String,    // WHAT: its id. WHY: what signing names.
    pub set: String,   // WHAT: the drawing set it's on. WHY: the asker matches it to its document.
    pub label: String, // WHAT: what it says. WHY: shown to the signer.
    pub group: String, // WHAT: its heading. WHY: shown above its lines.
    #[serde(rename = "ref")]
    pub reference: String, // WHAT: the asker's reference. WHY: read back with the signature.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<u32>, // WHAT: its page, if known. WHY: View opens there.
    pub status: &'static str, // WHAT: "open", "signed" or "withdrawn". WHY: one word says where it stands.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signer: Option<String>, // WHAT: the name written when signing. WHY: printed on the form.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed_by: Option<String>, // WHAT: the account that signed. WHY: a name is typed; the account is who it really was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signed: Option<String>, // WHAT: when (RFC 3339, UTC). WHY: printed on the form as the date.
    pub seq: i64, // WHAT: its place in the order of changes. WHY: a program asks for what changed after the last one it saw.
}

/// A signature as the signer's pad sends it.
#[derive(Deserialize)]
pub struct Signing {
    pub name: String, // WHAT: the name written under the signature. WHY: printed on the form; the account is kept beside it.
    pub strokes: Vec<Vec<[f64; 2]>>, // WHAT: each stroke's points, in the pad's units. WHY: the signature itself, drawn again as a picture.
}

/// What a page of changes holds.
#[derive(Serialize)]
pub struct Changes {
    pub signoffs: Vec<SignOff>, // WHAT: lines changed after `since`, oldest change first. WHY: applied in order.
    pub latest: i64, // WHAT: the last change's number, to ask from next time. WHY: nothing is missed and nothing read twice.
}

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    pub since: i64, // WHAT: the last change already seen (0: from the start). WHY: a program catching up.
}

/// The columns a sign-off is read with, in `row`'s order.
const COLUMNS: &str =
    "id, set_id, label, grp, ref, page, signer, signed_by, signed, withdrawn, seq"; // WHAT: one list. WHY: every query reads a line the same way.

fn row(r: &rusqlite::Row) -> rusqlite::Result<SignOff> {
    let signed: Option<String> = r.get(8)?; // WHAT: when it was signed, if it was. WHY: decides its status.
    let withdrawn: bool = r.get::<_, i64>(9)? != 0; // WHAT: whether the asker took it back. WHY: likewise.
    Ok(SignOff {
        id: r.get(0)?,
        set: r.get(1)?,
        label: r.get(2)?,
        group: r.get(3)?,
        reference: r.get(4)?,
        page: r.get::<_, Option<i64>>(5)?.map(|p| p as u32), // WHAT: the page, checked under 10,000 when asked. WHY: fits.
        status: match (&signed, withdrawn) {
            (Some(_), _) => "signed", // WHAT: signed wins. WHY: a signature is never undone.
            (None, true) => "withdrawn", // WHAT: taken back. WHY: no longer to be signed.
            (None, false) => "open",  // WHAT: waiting. WHY: shown to sign.
        },
        signer: r.get(6)?,
        signed_by: r.get(7)?,
        signed,
        seq: r.get(10)?,
    })
}

/// The next number in the order of changes. Taken inside the store's lock, so two are never the same.
fn next_seq(db: &rusqlite::Connection) -> rusqlite::Result<i64> {
    db.query_row("SELECT COALESCE(MAX(seq), 0) + 1 FROM signoffs", [], |r| {
        r.get(0)
    }) // WHAT: one past the highest. WHY: the store holds one connection behind a lock, so this is atomic.
}

/// Text a person reads: not empty, not too long, no control characters.
fn plain(what: &str, text: &str, most: usize) -> Answer<String> {
    let text = text.trim(); // WHAT: without spaces at the ends. WHY: " WELD " and "WELD" are the same line.
    if text.is_empty() || text.chars().count() > most || text.chars().any(char::is_control) {
        return Err(Denied::wrong(format!(
            "A sign-off's {what} must be 1 to {most} characters, with no control characters."
        ))); // WHAT: refused with the rule. WHY: said plainly, nothing saved.
    }
    Ok(text.to_string())
}

/// `POST /sets/:id/signoffs`: lines asked for on a set. Asking for a
/// reference that is already there (open or signed) changes nothing, so a
/// program can ask again after a restart without making doubles.
pub async fn ask(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(set): Path<String>,
    body: Body,
) -> Response {
    let gate =
        writer(&server, &headers).and_then(|who| may_reach_set(&server, &who, &set).map(|_| who)); // WHAT: someone who may change drawings, on this set. WHY: asked before the body is read.
    let who = match gate {
        Ok(who) => who, // WHAT: let in. WHY: their list is read next.
        Err(denied) => return turned_away(body, denied).await, // WHAT: refused. WHY: a stranger's body is read no further than a little.
    };
    let asked: Vec<Ask> = match json_once_let_in(body, ASK_BODY).await {
        Ok(asked) => asked, // WHAT: the lines. WHY: checked below.
        Err(denied) => return denied.into_response(), // WHAT: too big or not JSON. WHY: said plainly.
    };
    asked_for(&server, &who, &set, asked).into_response() // WHAT: saved. WHY: kept apart so it reads as one function.
}

fn asked_for(
    server: &Shared,
    who: &crate::auth::Who,
    set: &str,
    asked: Vec<Ask>,
) -> Answer<Json<Vec<SignOff>>> {
    if asked.len() > MOST_ASKED {
        return Err(Denied::wrong(format!(
            "At most {MOST_ASKED} sign-offs at once."
        ))); // WHAT: refused. WHY: past any real form.
    }
    let mut ready = Vec::with_capacity(asked.len()); // WHAT: every line, checked. WHY: all are checked before any is written.
    for one in &asked {
        let page = match one.page {
            Some(p) if p >= 10_000 => {
                return Err(Denied::wrong(
                    "A sign-off's page is past any drawing set's.",
                ))
            } // WHAT: refused. WHY: no set has that many pages.
            p => p, // WHAT: as given. WHY: fine.
        };
        ready.push((
            plain("label", &one.label, 200)?, // WHAT: what it says. WHY: shown to the signer.
            if one.group.trim().is_empty() {
                String::new()
            } else {
                plain("group", &one.group, 200)?
            }, // WHAT: its heading, which may be none. WHY: optional.
            plain("ref", &one.reference, 200)?, // WHAT: the asker's reference. WHY: how it comes back.
            page,
        ));
    }
    let now = crate::api::now(); // WHAT: the time, once. WHY: every line asked together has the same time.
    let lines = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?; // WHAT: all or nothing. WHY: a list half saved would be asked for twice.
            for (label, group, reference, page) in &ready {
                let there: Option<String> = tx
                    .query_row(
                        "SELECT id FROM signoffs WHERE set_id = ?1 AND ref = ?2 AND withdrawn = 0",
                        params![set, reference],
                        |r| r.get(0),
                    )
                    .optional()?; // WHAT: the same reference already asked for. WHY: asking again changes nothing.
                if there.is_some() {
                    continue; // WHAT: left as it is. WHY: open stays open, signed stays signed.
                }
                let seq = next_seq(&tx)?; // WHAT: its change number. WHY: the asker sees it in its changes.
                let id = format!("so_{:016x}{:016x}", rand::random::<u64>(), rand::random::<u64>()); // WHAT: a random id. WHY: nobody can guess another line's id.
                tx.execute(
                    "INSERT INTO signoffs (id, set_id, label, grp, ref, page, asked_by, asked, withdrawn, seq)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
                    params![id, set, label, group, reference, page.map(|p| p as i64), who.id, now, seq],
                )?; // WHAT: the line. WHY: open until signed.
            }
            tx.commit()?; // WHAT: kept. WHY: all at once.
            on_set(db, set)
        })
        .map_err(|e| Denied::broke("ask for those sign-offs", e))?;
    record(
        server,
        crate::audit::Entry::new(crate::audit::action::ASKED_SIGNOFFS, &who.email)
            .by(&who.id)
            .about(set.to_string())
            .saying(format!("{} line(s)", ready.len())),
    ); // WHAT: on the log. WHY: who asked for signatures, and on what.
    Ok(Json(lines))
}

/// A set's lines, in the order they were asked for.
fn on_set(db: &rusqlite::Connection, set: &str) -> anyhow::Result<Vec<SignOff>> {
    let mut q = db.prepare(&format!(
        "SELECT {COLUMNS} FROM signoffs WHERE set_id = ?1 ORDER BY rowid"
    ))?; // WHAT: every line on the set. WHY: the order they were asked in is the form's order.
    let lines = q
        .query_map(params![set], row)?
        .collect::<rusqlite::Result<Vec<_>>>()?; // WHAT: read. WHY: all of them.
    Ok(lines)
}

/// `GET /sets/:id/signoffs`: a set's lines, for anyone who may see it.
pub async fn of_set(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(set): Path<String>,
) -> Answer<Json<Vec<SignOff>>> {
    let who = caller(&server, &headers)?; // WHAT: anyone signed in. WHY: the shop's viewers sign.
    may_reach_set(&server, &who, &set)?; // WHAT: the set's project and folder rules. WHY: a set they may not see has no lines for them.
    server
        .store
        .with(|db| on_set(db, &set))
        .map(Json)
        .map_err(|e| Denied::broke("read those sign-offs", e))
}

/// The set a line is on, if there is such a line.
fn set_of(server: &Shared, id: &str) -> Answer<String> {
    server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT set_id FROM signoffs WHERE id = ?1",
                    params![id],
                    |r| r.get(0),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that sign-off", e))?
        .ok_or_else(|| Denied::missing("sign-off")) // WHAT: no such line. WHY: refused like a line they may not see.
}

/// `POST /signoffs/:id/sign`: a person signs a line.
pub async fn sign(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: Body,
) -> Response {
    let gate = caller(&server, &headers).and_then(|who| {
        if who.key.is_some() {
            return Err(Denied::not_with_this_key(
                "sign a sign-off: only a person signs",
            )); // WHAT: no key signs. WHY: a signature a program can make is worth nothing.
        }
        sharing(&server)?; // WHAT: the office's sharing is on. WHY: signing is new work, held to the license like any.
        let set = set_of(&server, &id)?; // WHAT: the set it's on. WHY: its rules decide.
        may_reach_set(&server, &who, &set)?; // WHAT: may they see that set. WHY: seeing the set is signing it; a line elsewhere is not there for them.
        Ok(who)
    }); // WHAT: every gate before the body. WHY: a stranger's strokes are never read.
    let who = match gate {
        Ok(who) => who, // WHAT: let in. WHY: their signature is read next.
        Err(denied) => return turned_away(body, denied).await, // WHAT: refused. WHY: as every gated route.
    };
    let signing: Signing = match json_once_let_in(body, ASK_BODY).await {
        Ok(s) => s,                                   // WHAT: the signature. WHY: checked below.
        Err(denied) => return denied.into_response(), // WHAT: too big or not JSON. WHY: said plainly.
    };
    signed(&server, &who, &id, signing).into_response() // WHAT: kept. WHY: apart, so it reads as one function.
}

/// Whether strokes are a signature this keeps: inside the pad, finite, and not absurdly many.
pub fn strokes_ok(strokes: &[Vec<[f64; 2]>]) -> bool {
    let points: usize = strokes.iter().map(Vec::len).sum(); // WHAT: every point. WHY: counted once.
    !strokes.is_empty() // WHAT: something drawn. WHY: an empty signature isn't one.
        && strokes.len() <= MOST_STROKES // WHAT: not too many strokes. WHY: past any signature.
        && (2..=MOST_POINTS).contains(&points) // WHAT: at least a mark, not a flood. WHY: likewise.
        && strokes.iter().flatten().all(|[x, y]| {
            x.is_finite() && y.is_finite() && (0.0..=PAD.0).contains(x) && (0.0..=PAD.1).contains(y)
        }) // WHAT: every point a real number on the pad. WHY: the picture is drawn from them.
}

fn signed(
    server: &Shared,
    who: &crate::auth::Who,
    id: &str,
    signing: Signing,
) -> Answer<Json<SignOff>> {
    let name = plain("signer's name", &signing.name, 80)?; // WHAT: the name written. WHY: printed on the form.
    if !strokes_ok(&signing.strokes) {
        return Err(Denied::wrong(
            "That signature couldn't be kept: draw it on the pad (it needs at least one stroke).",
        )); // WHAT: refused. WHY: nothing worth keeping, or something that isn't a pad's.
    }
    let strokes = serde_json::to_string(&signing.strokes)
        .map_err(|e| Denied::broke("keep that signature", e))?; // WHAT: the strokes as text. WHY: kept with the line.
    let now = crate::api::now(); // WHAT: when. WHY: printed as the date.
    let line = server
        .store
        .with(|db| {
            let seq = next_seq(db)?; // WHAT: its change number. WHY: the asker sees the signature.
            let changed = db.execute(
                "UPDATE signoffs SET signer = ?1, signed_by = ?2, signed = ?3, strokes = ?4, seq = ?5
                 WHERE id = ?6 AND signed IS NULL AND withdrawn = 0",
                params![name, who.id, now, strokes, seq, id],
            )?; // WHAT: signed, only if still open. WHY: a signature is final; two people at once can't both sign it.
            let line = db
                .query_row(&format!("SELECT {COLUMNS} FROM signoffs WHERE id = ?1"), params![id], row)
                .optional()?; // WHAT: the line as it is now. WHY: the answer, or the reason it wasn't signed.
            Ok((changed, line))
        })
        .map_err(|e| Denied::broke("keep that signature", e))?;
    match line {
        (1, Some(line)) => {
            record(
                server,
                crate::audit::Entry::new(crate::audit::action::SIGNED_OFF, &who.email)
                    .by(&who.id)
                    .about(format!("{} · {}", line.set, line.label))
                    .saying(format!("signed as {name}")),
            ); // WHAT: on the log. WHY: every signature can be found later.
            Ok(Json(line))
        }
        (_, Some(line)) if line.status == "signed" => Err(Denied::taken(format!(
            "That's already signed, by {}.",
            line.signer.unwrap_or_default()
        ))), // WHAT: refused: signed before. WHY: a signature is final.
        (_, Some(_)) => Err(Denied::taken(
            "That sign-off was withdrawn; it isn't asked for any more.",
        )), // WHAT: refused: taken back. WHY: nothing to sign.
        (_, None) => Err(Denied::missing("sign-off")), // WHAT: gone. WHY: as any missing thing.
    }
}

/// `POST /signoffs/:id/withdraw`: the asker takes back a line not yet signed.
pub async fn withdraw(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<SignOff>> {
    let who = writer(&server, &headers)?; // WHAT: someone who may change drawings. WHY: as asking.
    let set = set_of(&server, &id)?; // WHAT: its set. WHY: its rules decide.
    may_reach_set(&server, &who, &set)?; // WHAT: on a set they may reach. WHY: as asking.
    let line = server
        .store
        .with(|db| {
            let seq = next_seq(db)?; // WHAT: its change number. WHY: the change is seen.
            db.execute(
                "UPDATE signoffs SET withdrawn = 1, seq = ?1 WHERE id = ?2 AND signed IS NULL AND withdrawn = 0",
                params![seq, id],
            )?; // WHAT: taken back, only if not signed. WHY: a signature stays.
            Ok(db.query_row(&format!("SELECT {COLUMNS} FROM signoffs WHERE id = ?1"), params![id], row).optional()?)
        })
        .map_err(|e| Denied::broke("withdraw that sign-off", e))?
        .ok_or_else(|| Denied::missing("sign-off"))?;
    if line.status == "signed" {
        return Err(Denied::taken(
            "That's signed already; a signature can't be withdrawn.",
        )); // WHAT: refused. WHY: final.
    }
    Ok(Json(line))
}

/// `GET /signoffs?since=N`: every line changed after `since`, on sets the caller may reach.
pub async fn changes(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(since): Query<Since>,
) -> Answer<Json<Changes>> {
    let who = writer(&server, &headers)?; // WHAT: the asker (a program's key, or the office). WHY: the shop reads lines through their sets.
    let lines = server
        .store
        .with(|db| {
            let mut q = db.prepare(&format!(
                "SELECT {COLUMNS} FROM signoffs WHERE seq > ?1 ORDER BY seq LIMIT ?2"
            ))?; // WHAT: changes after `since`, oldest first. WHY: applied in order.
            let lines = q
                .query_map(params![since.since.max(0), (CHANGES_PAGE * 4) as i64], row)?
                .collect::<rusqlite::Result<Vec<_>>>()?; // WHAT: read. WHY: a page, with room for lines this caller may not reach.
            Ok(lines)
        })
        .map_err(|e| Denied::broke("read the sign-offs", e))?;
    let read_to = lines.last().map(|l| l.seq); // WHAT: the last change read. WHY: next time starts after it, reachable or not.
    let mut reach: std::collections::HashMap<String, bool> = Default::default(); // WHAT: each set's answer, asked once. WHY: a traveler's hundreds of lines share one set.
    let mut out = Vec::new(); // WHAT: the lines the caller may see. WHY: given back.
    let mut latest = since.since.max(0); // WHAT: how far this page reaches. WHY: given back.
    for line in lines {
        if out.len() >= CHANGES_PAGE {
            break; // WHAT: a page is full. WHY: the rest come next time, from `latest`.
        }
        let may = *reach
            .entry(line.set.clone())
            .or_insert_with(|| may_reach_set(&server, &who, &line.set).is_ok()); // WHAT: may they see its set. WHY: a line on a set they can't see isn't there for them.
        latest = line.seq; // WHAT: read up to here. WHY: unseen lines are passed, never read again.
        if may {
            out.push(line); // WHAT: theirs. WHY: given back.
        }
    }
    if out.len() < CHANGES_PAGE {
        latest = read_to.unwrap_or(latest).max(latest); // WHAT: everything read. WHY: a page cut short still moves on past lines they can't see.
    }
    Ok(Json(Changes {
        signoffs: out,
        latest,
    }))
}

/// `GET /signoffs/:id/signature.png`: a signed line's signature as a picture (400 by 140, black on white).
pub async fn picture(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let reached = caller(&server, &headers).and_then(|who| {
        let set = set_of(&server, &id)?; // WHAT: its set. WHY: its rules decide.
        may_reach_set(&server, &who, &set) // WHAT: may they see it. WHY: as any of the set's contents.
    });
    if let Err(denied) = reached {
        return denied.into_response(); // WHAT: refused. WHY: as above.
    }
    let strokes: Option<String> = match server.store.with(|db| {
        Ok(db
            .query_row(
                "SELECT strokes FROM signoffs WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()?)
    }) {
        Ok(found) => found.flatten(), // WHAT: the strokes, if signed. WHY: drawn below.
        Err(e) => return Denied::broke("read that signature", e).into_response(), // WHAT: a database failure. WHY: said.
    };
    let Some(strokes) = strokes.and_then(|s| serde_json::from_str::<Vec<Vec<[f64; 2]>>>(&s).ok())
    else {
        return Denied::missing("signature").into_response(); // WHAT: not signed yet. WHY: nothing to draw.
    };
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "no-store"),
        ], // WHAT: a PNG, never cached. WHY: a signature isn't left in caches.
        drawn(&strokes),
    )
        .into_response()
}

// ---- the picture: strokes drawn on the pad, as a PNG ------------------------

/// The signature drawn, as a PNG: 400 by 140, one grey byte a pixel.
pub fn drawn(strokes: &[Vec<[f64; 2]>]) -> Vec<u8> {
    let (w, h) = (PAD.0 as usize, PAD.1 as usize); // WHAT: the pad's size in pixels. WHY: one unit, one pixel.
    let mut ink = vec![255u8; w * h]; // WHAT: white. WHY: black ink goes on it.
    let mut dot = |x: f64, y: f64| {
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                let (px, py) = (x.round() as i32 + dx, y.round() as i32 + dy); // WHAT: a 3-pixel pen. WHY: a finger's line, readable printed small.
                if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                    ink[py as usize * w + px as usize] = 0; // WHAT: inked. WHY: inside the pad only.
                }
            }
        }
    };
    for stroke in strokes {
        for pair in stroke.windows(2) {
            let ([x0, y0], [x1, y1]) = (pair[0], pair[1]); // WHAT: one segment. WHY: drawn as dots close together.
            let steps = ((x1 - x0).abs().max((y1 - y0).abs()).ceil() as usize).clamp(1, 1000); // WHAT: a dot a pixel. WHY: no gaps; a segment is at most the pad's size.
            for k in 0..=steps {
                let t = k as f64 / steps as f64; // WHAT: along the segment. WHY: evenly.
                dot(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
            }
        }
        if let [only] = stroke.as_slice() {
            dot(only[0], only[1]); // WHAT: a single tap. WHY: a dot, as on paper.
        }
    }
    png_gray(w, h, &ink)
}

/// A greyscale picture as a PNG, with its data stored rather than packed (a
/// signature is 56 KB that way, and the encoder is a page of code with no library).
pub fn png_gray(w: usize, h: usize, pixels: &[u8]) -> Vec<u8> {
    let mut raw = Vec::with_capacity((w + 1) * h); // WHAT: the rows, each led by its filter byte. WHY: PNG's layout.
    for row in pixels.chunks(w) {
        raw.push(0); // WHAT: filter "none". WHY: the simplest.
        raw.extend_from_slice(row);
    }
    let mut zlib = vec![0x78, 0x01]; // WHAT: zlib's header, no compression level claimed. WHY: the data is stored.
    for (k, block) in raw.chunks(65_535).enumerate() {
        let last = (k + 1) * 65_535 >= raw.len(); // WHAT: whether this is the last block. WHY: deflate marks it.
        zlib.push(last as u8); // WHAT: a stored block, last or not. WHY: deflate's simplest block.
        let n = block.len() as u16; // WHAT: its length. WHY: stored blocks say it, and its complement.
        zlib.extend_from_slice(&n.to_le_bytes());
        zlib.extend_from_slice(&(!n).to_le_bytes());
        zlib.extend_from_slice(block);
    }
    zlib.extend_from_slice(&adler32(&raw).to_be_bytes()); // WHAT: zlib's checksum. WHY: readers check it.
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]; // WHAT: PNG's signature. WHY: every PNG starts so.
    let mut header = Vec::with_capacity(13); // WHAT: the IHDR chunk's data. WHY: size and kind.
    header.extend_from_slice(&(w as u32).to_be_bytes());
    header.extend_from_slice(&(h as u32).to_be_bytes());
    header.extend_from_slice(&[8, 0, 0, 0, 0]); // WHAT: 8-bit grey, deflate, adaptive filters, no interlace. WHY: the plainest PNG.
    chunk(&mut out, b"IHDR", &header);
    chunk(&mut out, b"IDAT", &zlib);
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes()); // WHAT: its length. WHY: PNG's chunk layout.
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_of = kind.to_vec(); // WHAT: the kind and data. WHY: the CRC covers both.
    crc_of.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_of).to_be_bytes());
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32; // WHAT: the standard start. WHY: PNG's CRC is the usual one.
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            }; // WHAT: the reflected polynomial. WHY: as PNG defines it.
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32); // WHAT: the standard start. WHY: zlib's checksum.
    for &x in data {
        a = (a + x as u32) % 65_521;
        b = (b + a) % 65_521;
    }
    (b << 16) | a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_signature_must_be_drawn_on_the_pad() {
        assert!(strokes_ok(&[vec![[10.0, 10.0], [100.0, 50.0]]]));
        assert!(!strokes_ok(&[]), "nothing drawn");
        assert!(
            !strokes_ok(&[vec![[10.0, 10.0]]]),
            "a single point isn't a signature"
        );
        assert!(
            !strokes_ok(&[vec![[10.0, 10.0], [500.0, 50.0]]]),
            "off the pad"
        );
        assert!(
            !strokes_ok(&[vec![[f64::NAN, 10.0], [1.0, 1.0]]]),
            "not a number"
        );
        assert!(
            !strokes_ok(&vec![vec![[1.0, 1.0], [2.0, 2.0]]; MOST_STROKES + 1]),
            "too many strokes"
        );
    }

    #[test]
    fn the_picture_is_a_png_with_the_ink_on_it() {
        let png = drawn(&[vec![[20.0, 70.0], [380.0, 70.0]]]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 400);
        assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 140);
        // The stored rows are in the data: row 70 has black from 20 to 380, row 0 is white.
        let idat = png.windows(4).position(|w| w == b"IDAT").unwrap() + 4;
        let raw = png[idat + 2..].to_vec();
        // Skip the first stored block's 5-byte header to reach the rows.
        let rows = &raw[5..];
        let row = |y: usize| &rows[y * 401 + 1..y * 401 + 401];
        assert!(row(0).iter().all(|p| *p == 255));
        assert_eq!(row(70)[200], 0);
        assert_eq!(row(70)[5], 255);
    }

    #[test]
    fn the_checksums_are_the_standard_ones() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
    }
}
