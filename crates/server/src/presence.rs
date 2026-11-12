//! Who is connected to this server, for the board that watches it.
//!
//! The Excalibur Fleet asks every install it looks after one more question
//! now: *who is on this thing, how many, and from where?* Citadel answers
//! from its own ledger, and this module is Hyperview's answer, in the same
//! wire shape, so the Fleet needs no per-product code — which is the whole
//! design of `crate::fleet`.
//!
//! What it is: a bounded, in-memory ledger of the seats this process has
//! served lately. One row per person-at-an-address. **Telemetry, never
//! authorisation** — nothing anywhere reads this ledger to let anybody in,
//! and the routes it feeds are read-only.
//!
//! What it deliberately is not: a table in the database. A presence ledger
//! is a statement about *this running process*, and pretending it survives a
//! restart would let an empty server wear last week's crowd. It lives in
//! memory, dies with the process, and the payload says when it was born
//! (`sinceBoot`) so an empty answer after a restart reads as "fresh ledger",
//! never "nobody ever connects".
//!
//! Three rules, inherited from every other health surface here:
//!
//! - **It never throws and never blocks a drawing.** Recording rides the
//!   request path of every API call; a ledger that could take down an upload
//!   is a thermometer debugging you. Everything is best-effort.
//! - **Bounded.** At most [`MOST_ROWS`] rows, oldest retired first, however
//!   many addresses a chatty client invents.
//! - **It spends one lookup, not one per request.** Who a token belongs to
//!   is resolved once per row and refreshed at most every minute, so the
//!   ledger costs the database almost nothing beyond what auth already pays.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::header;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;

use crate::api::Shared;

/// Rows kept. A six-seat shop uses a dozen; the cap is for the day something
/// misbehaves, because an unbounded map on the request path is a memory leak
/// with a schedule.
pub const MOST_ROWS: usize = 300;

/// "Connected now" means seen within this long — the same fifteen minutes the
/// Fleet's board and Citadel's ledger use, so the numbers agree.
const NOW_WINDOW: Duration = Duration::from_secs(15 * 60);

/// How long a row trusts the name it resolved before asking the database
/// again (a person may be renamed, or a session may lapse).
const RESOLVE_AGAIN: Duration = Duration::from_secs(60);

struct Row {
    who: String,
    ip: String,
    ua: String,
    first_at: SystemTime,
    last_at: SystemTime,
    resolved_at: Instant,
    hits: u64,
}

struct Ledger {
    born: SystemTime,
    rows: HashMap<String, Row>,
}

/// Process-lifetime state, like uptime itself. A static rather than a field
/// on [`crate::api::Server`] because every `Server` in this process serves
/// the same seats, and because telemetry growing constructor parameters is
/// how telemetry starts costing features.
fn ledger() -> &'static Mutex<Ledger> {
    static LEDGER: OnceLock<Mutex<Ledger>> = OnceLock::new();
    LEDGER.get_or_init(|| {
        Mutex::new(Ledger {
            born: SystemTime::now(),
            rows: HashMap::new(),
        })
    })
}

/// The middleware: sees every API request, records the signed-in ones, and
/// never says no to anything. `ConnectInfo` is optional on purpose — a test
/// that serves without it still runs; its rows just say `?` for the address.
pub async fn seen(
    State(server): State<Shared>,
    connect: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Response {
    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(|t| t.trim().to_string());
    let ua = request
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(120)
        .collect::<String>();
    if let Some(token) = token {
        let ip = connect
            .map(|ConnectInfo(addr)| addr.ip().to_string())
            .unwrap_or_else(|| "?".into());
        record(&server, &token, &ip, &ua);
    }
    next.run(request).await
}

fn record(server: &crate::api::Server, token: &str, ip: &str, ua: &str) {
    // The key is the token's hash — the token itself never sits in memory
    // longer than the request that carried it — plus the address, so the same
    // person on two machines is two devices, which is what the question asks.
    let key = format!("{}@{ip}", &crate::auth::token_hash(token)[..12]);
    let now = SystemTime::now();

    // Resolve outside the lock: the database must never wait on the ledger,
    // nor the ledger's lock on the database.
    let needs_name = {
        let Ok(ledger) = ledger().lock() else { return };
        match ledger.rows.get(&key) {
            Some(row) => row.resolved_at.elapsed() > RESOLVE_AGAIN,
            None => true,
        }
    };
    let resolved = if needs_name {
        let now_text = crate::api::now();
        server
            .store
            .with(|db| crate::auth::whose_still(db, token, &now_text)) // WHAT: whose session, without keeping it alive. WHY: watching who is connected must never make a session last longer (security review H2e)
            .ok()
            .flatten()
            .map(|who| who.name)
    } else {
        None
    };
    // A token the database does not recognise records nothing: the ledger
    // lists people this server let in, not strings that knocked.
    let Ok(mut ledger) = ledger().lock() else { return };
    match (ledger.rows.get_mut(&key), resolved, needs_name) {
        (Some(row), maybe_name, _) => {
            if let Some(name) = maybe_name {
                row.who = name;
                row.resolved_at = Instant::now();
            }
            row.last_at = now;
            row.ua = ua.to_string();
            row.hits += 1;
        }
        (None, Some(name), _) => {
            ledger.rows.insert(
                key,
                Row {
                    who: name,
                    ip: ip.to_string(),
                    ua: ua.to_string(),
                    first_at: now,
                    last_at: now,
                    resolved_at: Instant::now(),
                    hits: 1,
                },
            );
            if ledger.rows.len() > MOST_ROWS {
                // Retire the oldest. Rare enough that a scan is fine.
                if let Some(oldest) = ledger
                    .rows
                    .iter()
                    .min_by_key(|(_, r)| r.last_at)
                    .map(|(k, _)| k.clone())
                {
                    ledger.rows.remove(&oldest);
                }
            }
        }
        (None, None, true) => {}  // knocked with a token nobody recognises
        (None, None, false) => {} // unreachable in practice; nothing to do
    }
}

// ---- what the Fleet reads --------------------------------------------------

/// One row, in the exact shape Citadel serves and the Fleet parses —
/// camelCase and all — so the board treats a Hyperview seat like any other
/// connected device.
#[derive(Serialize, Clone)]
pub struct Seat {
    /// A Hyperview seat is somebody in the office; the Fleet's board words it
    /// that way when the kind is not `portal`.
    pub kind: &'static str,
    pub who: String,
    /// The installation's name — every seat here belongs to the shop this
    /// server serves, which is exactly how the board groups by company.
    pub company: String,
    pub ip: String,
    pub country: String,
    pub ua: String,
    #[serde(rename = "firstAt")]
    pub first_at: String,
    #[serde(rename = "lastAt")]
    pub last_at: String,
    pub hits: u64,
}

#[derive(Serialize)]
pub struct Roster {
    pub at: String,
    #[serde(rename = "sinceBoot")]
    pub since_boot: String,
    #[serde(rename = "connectedNow")]
    pub connected_now: usize,
    pub now: Vec<Seat>,
    pub recent: Vec<Seat>,
}

fn rfc3339(t: SystemTime) -> String {
    time::OffsetDateTime::from(t)
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

pub fn roster(company: &str) -> Roster {
    let Ok(ledger) = ledger().lock() else {
        return Roster {
            at: crate::api::now(),
            since_boot: crate::api::now(),
            connected_now: 0,
            now: Vec::new(),
            recent: Vec::new(),
        };
    };
    let mut recent: Vec<(SystemTime, Seat)> = ledger
        .rows
        .values()
        .map(|r| {
            (
                r.last_at,
                Seat {
                    kind: "office",
                    who: r.who.clone(),
                    company: company.to_string(),
                    ip: r.ip.clone(),
                    country: String::new(), // a LAN has no edge to name a country
                    ua: r.ua.clone(),
                    first_at: rfc3339(r.first_at),
                    last_at: rfc3339(r.last_at),
                    hits: r.hits,
                },
            )
        })
        .collect();
    recent.sort_by(|a, b| b.0.cmp(&a.0));
    let cutoff = SystemTime::now()
        .checked_sub(NOW_WINDOW)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let now: Vec<Seat> = recent
        .iter()
        .filter(|(t, _)| *t >= cutoff)
        .map(|(_, s)| s.clone())
        .collect();
    Roster {
        at: crate::api::now(),
        since_boot: rfc3339(ledger.born),
        connected_now: now.len(),
        now,
        recent: recent.into_iter().map(|(_, s)| s).collect(),
    }
}

/// For tests only: a ledger that remembers the last test is a ledger that
/// fails the next one.
#[cfg(test)]
pub fn forget_everything() {
    if let Ok(mut ledger) = ledger().lock() {
        ledger.rows.clear();
        ledger.born = SystemTime::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ledger is one per process, and tests run side by side: each test
    /// here holds this for as long as it uses the ledger, or one test's
    /// `forget_everything` empties another's rows half way through.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    fn alone() -> std::sync::MutexGuard<'static, ()> {
        ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn put(key: &str, who: &str, age: Duration) {
        let mut ledger = ledger().lock().unwrap();
        let when = SystemTime::now().checked_sub(age).unwrap();
        ledger.rows.insert(
            key.into(),
            Row {
                who: who.into(),
                ip: key.split('@').nth(1).unwrap_or("?").into(),
                ua: String::new(),
                first_at: when,
                last_at: when,
                resolved_at: Instant::now(),
                hits: 1,
            },
        );
    }

    #[test]
    fn connected_now_is_a_fifteen_minute_claim_and_the_roster_states_its_birth() {
        let _alone = alone();
        forget_everything();
        put("aaaa@10.0.0.5", "Dana", Duration::from_secs(60));
        put("bbbb@10.0.0.6", "Ron", Duration::from_secs(2 * 60 * 60));
        let roster = roster("Example Steel Shop");
        assert_eq!(roster.recent.len(), 2);
        assert_eq!(roster.connected_now, 1, "two hours ago is not 'now'");
        assert_eq!(roster.now[0].who, "Dana");
        assert!(!roster.since_boot.is_empty(), "an empty ledger must say when it was born");
        assert_eq!(roster.recent[0].company, "Example Steel Shop", "seats group under the shop");
        assert_eq!(roster.recent[0].kind, "office");
    }

    #[test]
    fn the_same_person_on_two_machines_is_two_devices() {
        let _alone = alone();
        forget_everything();
        put("cccc@10.0.0.7", "Dana", Duration::from_secs(10));
        put("cccc@10.0.0.8", "Dana", Duration::from_secs(10));
        assert_eq!(roster("x").recent.len(), 2);
    }

    #[test]
    fn the_ledger_is_bounded_and_keeps_the_newest() {
        let _alone = alone();
        forget_everything();
        for i in 0..(MOST_ROWS + 40) {
            put(
                &format!("t{i:04}@10.1.{}.{}", i / 250, i % 250),
                "Flood",
                Duration::from_secs((MOST_ROWS + 40 - i) as u64),
            );
            // the cap is enforced on insert in record(); here we mimic it
            let mut ledger = ledger().lock().unwrap();
            if ledger.rows.len() > MOST_ROWS {
                if let Some(oldest) = ledger
                    .rows
                    .iter()
                    .min_by_key(|(_, r)| r.last_at)
                    .map(|(k, _)| k.clone())
                {
                    ledger.rows.remove(&oldest);
                }
            }
        }
        let roster = roster("x");
        assert_eq!(roster.recent.len(), MOST_ROWS);
        // the newest row survived the flood
        assert!(roster.recent.iter().any(|s| s.ip.ends_with(&format!("10.1.{}.{}", (MOST_ROWS + 39) / 250, (MOST_ROWS + 39) % 250))));
    }
}
