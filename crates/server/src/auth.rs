//! Who is asking.
//!
//! Passwords are hashed with Argon2id and never stored or logged in any other
//! form. Tokens are random, opaque and stored hashed as well, so a copy of the
//! database is not a set of live sessions.

use anyhow::{anyhow, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use hub::Role;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};

use crate::store::hex;

/// How long a sign-in lasts **without being used**. Every request pushes it
/// out again (see `keep_alive`), so a machine somebody works on never asks
/// for a password twice, while a laptop left in a jobsite trailer goes quiet
/// and then goes dead. Idle time is the thing worth measuring: a fixed span
/// makes the person who uses Excalibur every day sign in as often as the one
/// who opened it once.
pub const SESSION_DAYS: i64 = 30;

/// A session is only written forward when its expiry has slipped more than
/// this far, so ordinary use costs one small write a day rather than one per
/// request.
const SLIDE_AFTER_HOURS: i64 = 24;

pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| anyhow!("hashing that password: {e}"))
}

pub fn password_matches(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// A new session token: what the client keeps, and what the server stores.
///
/// They are not the same. The server keeps only the hash, so somebody who
/// reads the database cannot then act as anybody in it.
pub fn new_token() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    let token = hex(&bytes);
    let stored = hex(&Sha256::digest(token.as_bytes()));
    (token, stored)
}

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token.as_bytes()))
}

#[derive(Clone, Debug)]
pub struct Who {
    pub id: String,
    pub name: String,
    pub email: String,
    pub role: Role,
    /// The key this request came with, when it came with one rather than a
    /// signed-in session; None for a person signed in with a password.
    pub key: Option<KeyUse>, // WHAT: which key is acting, if any. WHY: a key never carries the power its maker's session would, so every check has to know.
}

/// A key acting for the person who made it: what it is, so the routes can hold
/// it to what it was made for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyUse {
    pub id: String, // WHAT: the key's id. WHY: so /me can say which key this is, and the person can find it in the list to take back.
    pub name: String, // WHAT: what the key was called when it was made. WHY: the name a person recognises it by.
    pub purpose: String, // WHAT: assistant, integration or sync. WHY: decides which routes the key may reach at all.
    pub expires: Option<String>, // WHAT: when it stops working, if it was given an end. WHY: shown back so the program can warn before it runs out.
}

impl Who {
    /// Whether this request came with a key of `purpose`.
    pub fn is_key(&self, purpose: &str) -> bool {
        self.key.as_ref().is_some_and(|k| k.purpose == purpose) // WHAT: true only for a key made for that purpose. WHY: a signed-in person is never mistaken for a key.
    }
}

impl Who {
    pub fn as_user(&self) -> hub::User {
        hub::User {
            id: self.id.clone(),
            name: self.name.clone(),
            email: self.email.clone(),
            role: self.role,
        }
    }
}

pub fn role_from(text: &str) -> Role {
    match text {
        "admin" => Role::Admin,
        "viewer" => Role::Viewer,
        _ => Role::Estimator,
    }
}

pub fn role_name(role: Role) -> &'static str {
    role.word()
}

/// Finds who a token belongs to, if it is still good — and, since the token
/// was just used, keeps it alive a while longer.
pub fn whose(db: &Connection, token: &str, now: &str) -> Result<Option<Who>> {
    let hash = token_hash(token);
    let found = db
        .query_row(
            "SELECT people.id, people.name, people.email, people.role, sessions.expires
             FROM sessions JOIN people ON people.id = sessions.person
             WHERE sessions.token = ?1 AND sessions.expires > ?2",
            params![hash, now],
            |r| {
                Ok((
                    Who {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        email: r.get(2)?,
                        role: role_from(&r.get::<_, String>(3)?),
                        key: None, // WHAT: no key: this is a password sign-in. WHY: a session is the person themselves.
                    },
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()?;
    let Some((who, expires)) = found else {
        return Ok(None);
    };
    keep_alive(db, &hash, &expires, now)?;
    Ok(Some(who))
}

/// Pushes a session's expiry back out to the full span, now that it has been
/// used. Does nothing until it has slipped by `SLIDE_AFTER_HOURS`, and never
/// brings an expiry *forward* — a session is only ever lengthened here.
///
/// A failure to write is not a failure to authenticate: the person is who
/// they say they are either way, and a read-only moment (a locked database, a
/// disk that has filled) should not sign the office out.
fn keep_alive(db: &Connection, hash: &str, expires: &str, now: &str) -> Result<()> {
    let (Some(now_at), Some(expires_at)) = (parse_time(now), parse_time(expires)) else {
        return Ok(());
    };
    let wanted = now_at + time::Duration::days(SESSION_DAYS);
    if wanted - expires_at < time::Duration::hours(SLIDE_AFTER_HOURS) {
        return Ok(());
    }
    let Ok(text) = wanted.format(&time::format_description::well_known::Rfc3339) else {
        return Ok(());
    };
    if let Err(e) = db.execute(
        "UPDATE sessions SET expires = ?1 WHERE token = ?2 AND expires < ?1",
        params![text, hash],
    ) {
        tracing::warn!("a session could not be kept alive: {e}");
    }
    Ok(())
}

fn parse_time(text: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(text, &time::format_description::well_known::Rfc3339).ok()
}

/// What a key is for.
pub mod purpose {
    /// An assistant: the MCP endpoint and nothing else, and that read-only.
    pub const ASSISTANT: &str = "assistant";
    /// Another system — FabWire — using the API as the person who made it.
    /// It never carries an administrator's power, even when an administrator
    /// made it: running the server takes a signed-in administrator.
    pub const INTEGRATION: &str = "integration";
    /// A program that keeps projects in step with its own records: it files
    /// projects under its own reference, puts drawings up, moves and archives
    /// those projects and reads their markups back — that list of routes and
    /// nothing else (`crate::api::SYNC_ROUTES`), and only on projects filed
    /// under a reference. Made by an administrator, never with their power.
    pub const SYNC: &str = "sync"; // WHAT: the narrow key purpose. WHY: a program that only syncs should not hold a key that can run the server.
}

/// The longest end a key can be given, in days: ten years. Longer is "never",
/// which is what leaving it out already means.
pub const LONGEST_KEY_DAYS: u32 = 3650; // WHAT: the cap on a key's lifetime. WHY: a number nobody would type on purpose is refused rather than stored.

/// What every key starts with, so one can never be mistaken for a session.
pub const KEY_PREFIX: &str = "hvk_";

/// A new key, and the digest that is kept of it.
pub fn fresh_key() -> (String, String) {
    let mut bytes = [0u8; 24];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    let key = format!("{KEY_PREFIX}{}", hex(&bytes));
    let digest = token_hash(&key);
    (key, digest)
}

/// Whose key this is, when it is one of `purposes` and has not run out.
///
/// A `sync` key acts as its maker with at most an estimator's role: it may
/// change what it was made to change and never what an administrator could.
pub fn whose_key(db: &Connection, key: &str, purposes: &[&str], now: &str) -> Result<Option<Who>> {
    let digest = token_hash(key);
    let found: Option<(Who, String, String)> = db
        .query_row(
            "SELECT people.id, people.name, people.email, people.role, keys.purpose, keys.id, keys.name, keys.expires
             FROM keys JOIN people ON people.id = keys.person
             WHERE keys.digest = ?1",
            params![digest],
            |r| {
                let purpose: String = r.get(4)?; // WHAT: what the key was made for. WHY: kept on the Who so every route can see it.
                let id: String = r.get(5)?; // WHAT: the key's own id. WHY: for last_used below and for /me.
                Ok((
                    Who {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        email: r.get(2)?,
                        role: role_from(&r.get::<_, String>(3)?),
                        key: Some(KeyUse {
                            id: id.clone(), // WHAT: the key's id. WHY: so the answer can say which key acted.
                            name: r.get(6)?, // WHAT: the key's name. WHY: shown back on /me.
                            purpose: purpose.clone(), // WHAT: the purpose. WHY: the routes hold the key to it.
                            expires: r.get(7)?, // WHAT: its end, if it has one. WHY: checked just below and shown on /me.
                        }), // WHAT: the request came with this key. WHY: a key never acts with its maker's full session power.
                    },
                    purpose,
                    id,
                ))
            },
        )
        .optional()?;
    let Some((mut who, purpose, id)) = found else {
        return Ok(None);
    };
    if !purposes.contains(&purpose.as_str()) {
        return Ok(None);
    }
    let ends = who.key.as_ref().and_then(|k| k.expires.clone()); // WHAT: the key's end, if it was given one. WHY: a key past its end is nobody.
    if let Some(ends) = ends {
        // WHAT: an end that cannot be read, or has come, refuses the key. WHY: the safe way round: an unreadable date never means "forever".
        match (parse_time(&ends), parse_time(now)) {
            (Some(ends_at), Some(now_at)) if now_at < ends_at => {} // WHAT: still inside its time. WHY: the key works until the instant it ends.
            _ => return Ok(None), // WHAT: run out, or a date nobody can read. WHY: refused exactly like a key that does not exist.
        }
    }
    if purpose == purpose::SYNC && who.role == Role::Admin {
        who.role = Role::Estimator; // WHAT: a sync key made by an administrator acts as an estimator. WHY: it may write drawings, never run the server or see every job.
    }
    db.execute("UPDATE keys SET last_used = ?1 WHERE id = ?2", params![now, id])?;
    Ok(Some(who))
}

/// What a key was made for, and the email of whoever made it, without using
/// it: nothing is written and nobody is signed in by asking. None when there
/// is no such key.
pub fn purpose_of(db: &Connection, key: &str) -> Result<Option<(String, String)>> {
    Ok(db
        .query_row(
            "SELECT keys.purpose, people.email FROM keys JOIN people ON people.id = keys.person WHERE keys.digest = ?1", // WHAT: looked up by digest, as every key is. WHY: the key itself is never stored.
            params![token_hash(key)],
            |r| Ok((r.get(0)?, r.get(1)?)), // WHAT: the purpose and the maker's email. WHY: the purpose decides; the email goes on the audit line.
        )
        .optional()?) // WHAT: None for a key that is not there. WHY: the route then refuses it as nobody, as before.
}

/// The moment `days` from `now`, written the way every other time here is.
pub fn days_from(now: &str, days: u32) -> Option<String> {
    let start = parse_time(now)?; // WHAT: now, read back as a time. WHY: the end is counted from it.
    let end = start + time::Duration::days(i64::from(days)); // WHAT: that many whole days later. WHY: "30 days" means the same instant thirty days on.
    end.format(&time::format_description::well_known::Rfc3339).ok() // WHAT: written as RFC 3339. WHY: the same shape as made, last_used and every session's expiry.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The word the desktop program sends for a role, and the word this
    /// server reads, have to be the same word.
    ///
    /// Nothing else would notice if they were not. `role_from` does not
    /// refuse a word it does not know -- it hands out an ordinary seat -- so
    /// a mismatch is silent: "make her an administrator" makes her an
    /// estimator, the screen says estimator, and the day somebody has to be
    /// removed there is nobody who can do it. This is the only place the two
    /// halves are checked against each other.
    #[test]
    fn a_role_survives_the_trip_to_a_server_and_back() {
        for role in [Role::Viewer, Role::Estimator, Role::Admin] {
            assert_eq!(
                role_from(role.word()),
                role,
                "{:?} did not come back as itself through the word {:?}",
                role,
                role.word()
            );
        }
    }

    /// And the words themselves are pinned, because they are on the wire:
    /// a server that started calling an administrator something else would
    /// be refusing every older copy of the program in the shop.
    #[test]
    fn the_words_on_the_wire_are_the_ones_every_seat_already_sends() {
        assert_eq!(Role::Viewer.word(), "viewer");
        assert_eq!(Role::Estimator.word(), "estimator");
        assert_eq!(Role::Admin.word(), "admin");
    }

    /// A database with one person and one session, expiring when told.
    fn a_server_with_a_session(expires: &str) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE people (id TEXT PRIMARY KEY, name TEXT, email TEXT, role TEXT, created TEXT);
             CREATE TABLE sessions (token TEXT PRIMARY KEY, person TEXT, issued TEXT, expires TEXT);
             INSERT INTO people VALUES ('p1', 'Sam', 'sam@example.com', 'admin', '2026-01-01T00:00:00Z');",
        )
        .unwrap();
        db.execute(
            "INSERT INTO sessions VALUES (?1, 'p1', '2026-01-01T00:00:00Z', ?2)",
            params![token_hash("tok"), expires],
        )
        .unwrap();
        db
    }

    fn expiry_of(db: &Connection) -> String {
        db.query_row("SELECT expires FROM sessions", [], |r| r.get(0)).unwrap()
    }

    #[test]
    fn using_a_session_keeps_it_alive() {
        // Signed in a fortnight ago, used today: the expiry moves out to a
        // full span from today, so somebody who works every day is never
        // asked to sign in again.
        let db = a_server_with_a_session("2026-03-01T00:00:00Z");
        let who = whose(&db, "tok", "2026-02-20T09:00:00Z").unwrap();
        assert_eq!(who.unwrap().email, "sam@example.com");
        assert!(expiry_of(&db).starts_with("2026-03-22"), "{}", expiry_of(&db));
    }

    #[test]
    fn a_session_left_alone_runs_out() {
        // The same laptop, opened five weeks later. Nothing pushed the expiry
        // out in the meantime, so it is over - and being over, it is not
        // quietly renewed by the attempt.
        let db = a_server_with_a_session("2026-03-01T00:00:00Z");
        assert!(whose(&db, "tok", "2026-04-07T09:00:00Z").unwrap().is_none());
        assert_eq!(expiry_of(&db), "2026-03-01T00:00:00Z");
    }

    #[test]
    fn a_session_is_not_written_on_every_request() {
        // Used twice in a morning: the first use is what moves it, and the
        // second leaves the row alone.
        let db = a_server_with_a_session("2026-03-01T00:00:00Z");
        whose(&db, "tok", "2026-02-20T09:00:00Z").unwrap();
        let after_first = expiry_of(&db);
        whose(&db, "tok", "2026-02-20T09:05:00Z").unwrap();
        assert_eq!(expiry_of(&db), after_first, "a second look changed nothing");
    }

    #[test]
    fn keeping_a_session_alive_never_shortens_it() {
        // A long-lived session - one an administrator extended by hand, say -
        // is not cut back to the standard span by being used.
        let far = "2027-01-01T00:00:00Z";
        let db = a_server_with_a_session(far);
        whose(&db, "tok", "2026-02-20T09:00:00Z").unwrap();
        assert_eq!(expiry_of(&db), far);
    }

    #[test]
    fn somebody_elses_token_is_still_nobody() {
        let db = a_server_with_a_session("2026-03-01T00:00:00Z");
        assert!(whose(&db, "not-the-token", "2026-02-20T09:00:00Z").unwrap().is_none());
        assert_eq!(expiry_of(&db), "2026-03-01T00:00:00Z");
    }

    #[test]
    fn a_password_is_never_stored_as_itself() {
        let stored = hash_password("correct horse battery staple").unwrap();
        assert!(!stored.contains("correct horse"));
        assert!(stored.starts_with("$argon2id$"));
        assert!(password_matches("correct horse battery staple", &stored));
        assert!(!password_matches("Correct Horse Battery Staple", &stored));
        assert!(!password_matches("", &stored));
    }

    #[test]
    fn the_same_password_twice_does_not_hash_the_same_way() {
        let a = hash_password("shop2026").unwrap();
        let b = hash_password("shop2026").unwrap();
        assert_ne!(a, b, "each one gets its own salt");
        assert!(password_matches("shop2026", &a));
        assert!(password_matches("shop2026", &b));
    }

    #[test]
    fn a_rubbish_stored_hash_lets_nobody_in() {
        assert!(!password_matches("anything", ""));
        assert!(!password_matches("anything", "not a hash"));
        assert!(!password_matches("", ""));
    }

    #[test]
    fn what_the_client_holds_is_not_what_the_server_stores() {
        let (token, stored) = new_token();
        assert_ne!(token, stored);
        assert_eq!(token_hash(&token), stored);
        assert_eq!(token.len(), 64);
    }

    #[test]
    fn two_tokens_are_never_the_same() {
        let (a, _) = new_token();
        let (b, _) = new_token();
        assert_ne!(a, b);
    }

    #[test]
    fn roles_survive_the_round_trip_through_the_database() {
        for role in [Role::Admin, Role::Estimator, Role::Viewer] {
            assert_eq!(role_from(role_name(role)), role);
        }
        // Anything unrecognised is the ordinary seat, never the powerful one.
        assert_eq!(role_from("wizard"), Role::Estimator);
        assert_eq!(role_from(""), Role::Estimator);
    }
}

// ---- something a person can read off a screen -----------------------------

/// Words a fabricator already says, because the whole point of this string is
/// that somebody reads it aloud across an office and the other person types it
/// correctly the first time. No symbols, no case, nothing that looks like
/// something else in a sans-serif font.
const WORDS: &[&str] = &[
    "anchor", "beam", "brace", "camber", "channel", "clevis", "column", "coping", "deck",
    "flange", "gusset", "girder", "grout", "haunch", "joist", "kicker", "lintel", "moment",
    "plate", "purlin", "rebar", "shim", "splice", "stud", "truss", "washer", "weld", "web",
];

/// A secret in that shape. Five words and four hex digits is about a hundred
/// thousand million million guesses; it is not a password hash's worth of
/// strength and is not meant to be, because it is the thing that decides who
/// may *ask* for an account rather than the thing that protects one.
pub fn readable_secret() -> String {
    let mut rng = rand::thread_rng();
    let mut out = Vec::new();
    for _ in 0..5 {
        out.push(WORDS[rand::Rng::gen_range(&mut rng, 0..WORDS.len())]);
    }
    let tail: u16 = rand::Rng::gen_range(&mut rng, 0x1000..0xffff);
    format!("{}-{tail:04x}", out.join("-"))
}

/// Two of those compared without letting the time taken say how much of one
/// was right, and without caring how somebody typed the spaces or the case.
pub fn secret_matches(typed: &str, stored: &str) -> bool {
    fn tidy(s: &str) -> String {
        s.trim().to_lowercase().replace([' ', '_'], "-")
    }
    let (a, b) = (tidy(typed), tidy(stored));
    if a.len() != b.len() || b.is_empty() {
        return false;
    }
    let mut same = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        same |= x ^ y;
    }
    same == 0
}

#[cfg(test)]
mod secret_tests {
    use super::*;

    #[test]
    fn a_code_is_readable_and_never_the_same_twice() {
        let one = readable_secret();
        let two = readable_secret();
        assert_ne!(one, two);
        assert!(one.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert_eq!(one.matches('-').count(), 5);
    }

    #[test]
    fn typing_it_back_differently_still_works() {
        let code = "beam-plate-weld-stud-truss-1a2b";
        assert!(secret_matches("  BEAM-Plate-weld-stud-truss-1A2B ", code));
        assert!(secret_matches("beam plate weld stud truss 1a2b", code));
        assert!(!secret_matches("beam-plate-weld-stud-truss-1a2c", code));
        assert!(!secret_matches("", code));
        // And an empty stored code lets nobody in, rather than everybody.
        assert!(!secret_matches("", ""));
        assert!(!secret_matches("anything", ""));
    }
}
