//! The API.
//!
//! Every route here is documented in [`crate::openapi`], and a test checks that
//! the two agree, so the published description of this server cannot drift
//! away from what it actually does.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use hub::update::{Channel, Release, UpdateOffer};
use hub::*;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

use crate::auth::{self, Who};
use crate::quantities::{self, SheetScale, Stored};
use crate::store::{is_digest, Store};
use crate::{Config, VERSION};

/// What a client sent with a request whose handler has no use for it. Reading
/// it anyway keeps the connection open for the next request: a body left
/// unread makes the server close the connection, and a Mac reports that to
/// the program that asked as "Invalid argument".
type Unused = axum::body::Bytes;

pub struct Server {
    pub store: Store,
    pub config: Config,
    /// When the publisher's feed was last looked at, and the one look allowed
    /// at a time.
    pub updates: crate::updates::Watch,
    /// How many wrong guesses this server has sat through, and from whom.
    /// Empty on every start, which is deliberate -- see `patience`.
    pub patience: crate::patience::Patience,
    /// The shop's own tunnel, when they have turned one on.
    pub tunnel: crate::tunnel::Tunnel,
    /// Change notices waiting to go (`crate::notices`).
    pub notices: crate::notices::Pending,
    /// Each plugin's manifest, by the plugin file's digest, read once.
    pub manifests: std::sync::Mutex<std::collections::HashMap<String, Option<plugin_api::Manifest>>>,
}

impl Server {
    pub fn new(store: Store, config: Config) -> Server {
        let server = Server {
            store,
            config,
            updates: crate::updates::Watch::default(),
            patience: crate::patience::Patience::default(),
            tunnel: crate::tunnel::Tunnel::default(),
            manifests: Default::default(),
            notices: Default::default(),
        };
        crate::license::begin(&server);
        server
    }
}

pub type Shared = Arc<Server>;

// ---- saying no politely ---------------------------------------------------

/// A failure, as the wire sees it. Built through the constructors below so
/// every message is a sentence somebody can act on rather than a status code.
pub struct Denied(StatusCode, Problem);

impl IntoResponse for Denied {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl Denied {
    pub(crate) fn unauthorised() -> Denied {
        Denied(
            StatusCode::UNAUTHORIZED,
            Problem::new("unauthorised", "Sign in first."),
        )
    }

    pub(crate) fn forbidden(what: &str) -> Denied {
        Denied(
            StatusCode::FORBIDDEN,
            Problem::new(
                "forbidden",
                format!("Your account is not allowed to {what}. An administrator can change that."),
            ),
        )
    }

    pub(crate) fn missing(what: &str) -> Denied {
        Denied(
            StatusCode::NOT_FOUND,
            Problem::new("not_found", format!("No such {what} on this server.")),
        )
    }

    /// The office's trial is over and there is no license. Never a 401: a
    /// program that signs its person out on a 401 must not do so over this.
    pub(crate) fn license_needed(message: impl Into<String>) -> Denied {
        Denied(StatusCode::FORBIDDEN, Problem::new("license_needed", message))
    }

    pub(crate) fn license_users(message: impl Into<String>) -> Denied {
        Denied(StatusCode::FORBIDDEN, Problem::new("license_users", message))
    }

    pub(crate) fn wrong(message: impl Into<String>) -> Denied {
        Denied(
            StatusCode::BAD_REQUEST,
            Problem::new("bad_request", message),
        )
    }

    /// Something went wrong inside. The caller is told plainly, and the detail
    /// goes to the log rather than to them.
    pub(crate) fn broke(context: &str, error: impl std::fmt::Display) -> Denied {
        tracing::error!("{context}: {error}");
        Denied(
            StatusCode::INTERNAL_SERVER_ERROR,
            Problem::new(
                "server_error",
                format!("The server could not {context}. Nothing was changed."),
            ),
        )
    }
}

pub(crate) type Answer<T> = Result<T, Denied>;

// ---- who is asking --------------------------------------------------------

fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    value
        .strip_prefix("Bearer ")
        .or_else(|| value.strip_prefix("bearer "))
        .map(|t| t.trim().to_string())
}

pub(crate) fn caller(server: &Server, headers: &HeaderMap) -> Answer<Who> {
    caller_as(server, headers, &[auth::purpose::INTEGRATION])
}

/// Who is asking: a signed-in person, or a key of one of `purposes` acting as
/// the person who made it.
fn caller_as(server: &Server, headers: &HeaderMap, purposes: &[&str]) -> Answer<Who> {
    let token = bearer(headers).ok_or_else(Denied::unauthorised)?;
    let now = now();
    let found = server
        .store
        .with(|db| {
            if token.starts_with(auth::KEY_PREFIX) {
                auth::whose_key(db, &token, purposes, &now)
            } else {
                auth::whose(db, &token, &now)
            }
        })
        .map_err(|e| Denied::broke("check who you are", e))?;
    found.ok_or_else(Denied::unauthorised)
}

/// A person signed in with a password, not a program with a key. Keys are
/// made by people; a key cannot make another key.
fn person(server: &Server, headers: &HeaderMap) -> Answer<Who> {
    if bearer(headers).is_some_and(|t| t.starts_with(auth::KEY_PREFIX)) {
        return Err(Denied::forbidden("make or list keys with a key; sign in as a person"));
    }
    caller(server, headers)
}

pub(crate) fn writer(server: &Server, headers: &HeaderMap) -> Answer<Who> {
    let who = caller(server, headers)?;
    if !who.role.may_write() {
        return Err(Denied::forbidden("change drawings"));
    }
    sharing(server)?;
    Ok(who)
}

/// New work can be shared through the server: always, unless an Office trial
/// has ended with no license. Reading and exporting are never behind this.
pub(crate) fn sharing(server: &Server) -> Answer<()> {
    crate::license::sharing(server).map_err(Denied::license_needed)
}

pub(crate) fn administrator(server: &Server, headers: &HeaderMap) -> Answer<Who> {
    let who = caller(server, headers)?;
    if !who.role.may_administer() {
        return Err(Denied::forbidden("administer this server"));
    }
    Ok(who)
}

pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".into())
}

pub fn fresh_id(prefix: &str) -> String {
    let mut bytes = [0u8; 12];
    rand::Rng::fill(&mut rand::thread_rng(), &mut bytes);
    format!("{prefix}_{}", crate::store::hex(&bytes))
}

// ---- the routes -----------------------------------------------------------

pub fn router(server: Shared) -> Router {
    let limit = server.config.largest_upload as usize;
    let api = Router::new()
        .route("/health", get(health))
        .route("/openapi.json", get(openapi))
        .route("/auth/token", post(sign_in))
        .route("/setup", post(claim))
        .route("/join", post(join))
        .route("/admin/joining", get(read_joining).post(change_joining))
        .route("/admin/invites", get(list_invites).post(make_invite))
        .route("/admin/invites/:code", axum::routing::delete(drop_invite))
        .route("/me", get(me))
        .route("/me/password", post(change_password))
        .route("/signout", post(sign_out))
        .route("/me/keys", get(list_keys).post(make_key))
        .route("/keys/:id/revoke", post(revoke_key))
        .route("/sets/:id/markuplist", get(set_markuplist))
        .route("/projects/:id/markuplist", get(project_markuplist))
        .route("/admin/updates", get(read_updates).post(change_updates))
        .route("/admin/fleet", get(read_fleet).post(change_fleet))
        .route("/admin/remote", get(read_remote).post(change_remote).delete(turn_remote_off))
        .route("/projects", get(crate::projects::list).post(crate::projects::create))
        .route("/projects/:id", get(crate::projects::one))
        .route(
            "/projects/:id/people",
            get(crate::projects::who_is_on).post(crate::projects::change_who_is_on),
        )
        .route("/projects/:id/archive", post(crate::projects::archive))
        .route("/projects/:id/place", post(crate::projects::place))
        .route("/projects/:id/stamp", get(crate::projects::stamp))
        .route("/projects/:id/sets", get(sets_of))
        .route("/sets", post(upload_set))
        .route("/sets/:id", get(one_set))
        .route("/sets/:id/file", get(set_file))
        .route("/sets/:id/sheets", get(sheets_of))
        .route("/sets/:id/markups", get(markups_of).post(add_markups))
        .route("/sets/:id/copy", post(copy_set))
        .route("/sets/:id/takeoff", get(takeoff_of))
        .route("/sets/:id/takeoff.csv", get(takeoff_csv))
        .route("/people", get(people).post(add_person))
        .route("/people/:id/role", post(change_role))
        .route("/people/:id/remove", post(remove_person))
        .route("/chests", get(chests).post(upload_chest))
        .route("/chests/:id/file", get(chest_file))
        .route("/plugins", get(plugins).post(add_plugin))
        .route("/plugins/:id/file", get(plugin_file))
        .route("/plugins/:id/run", post(run_plugin))
        .route("/plugins/:id/remove", post(remove_plugin))
        .route("/notices", get(list_notices).post(add_notice))
        .route("/notices/:id/test", post(test_notice))
        .route("/notices/:id/remove", post(remove_notice))
        .route("/license", get(read_license).post(add_license))
        .route("/audit", get(read_audit))
        .route("/audit.csv", get(audit_csv))
        .route("/admin/pin", post(set_pin))
        .route("/admin/releases", post(upload_release))
        .route("/admin/releases/:version/ready", post(release_ready))
        .route("/update/latest", get(update_latest))
        .route("/update/:version/download", get(update_download))
        .layer(DefaultBodyLimit::max(limit))
        // Who is connected, for the Fleet's board. Sees every API request,
        // records the signed-in ones, refuses nothing (crate::presence).
        .layer(axum::middleware::from_fn_with_state(
            server.clone(),
            crate::presence::seen,
        ))
        .with_state(server.clone());

    // The product's API, and beside it the small maintenance surface the
    // Excalibur Fleet watches a shop's server through. Separate on purpose:
    // one is documented, versioned and for customers to build against; the
    // other is key-gated, undocumented in the OpenAPI description, and absent
    // altogether until somebody sets a key.
    Router::new()
        .nest(API_ROOT, api)
        .merge(crate::fleet::router(server.clone()))
        // Where an assistant asks about the quantities. Same token as the
        // API, and read-only: see the note at the top of `crate::mcp`.
        .merge(crate::mcp::router(server))
}

/// Every path this server serves, for the test that keeps the description
/// honest. Kept beside the router above so they are changed together.
pub const ROUTES: &[(&str, &str)] = &[
    ("GET", "/health"),
    ("GET", "/openapi.json"),
    ("POST", "/auth/token"),
    ("POST", "/setup"),
    ("POST", "/join"),
    ("GET", "/admin/joining"),
    ("POST", "/admin/joining"),
    ("GET", "/me"),
    ("POST", "/me/password"),
    ("POST", "/signout"),
    ("GET", "/me/keys"),
    ("POST", "/me/keys"),
    ("POST", "/keys/{id}/revoke"),
    ("GET", "/sets/{id}/markuplist"),
    ("GET", "/projects/{id}/markuplist"),
    ("GET", "/admin/updates"),
    ("POST", "/admin/updates"),
    ("GET", "/admin/fleet"),
    ("POST", "/admin/fleet"),
    ("GET", "/projects"),
    ("POST", "/projects"),
    ("GET", "/projects/{id}"),
    ("POST", "/projects/{id}/archive"),
    ("POST", "/projects/{id}/place"),
    ("GET", "/projects/{id}/stamp"),
    ("GET", "/projects/{id}/sets"),
    ("POST", "/sets"),
    ("GET", "/sets/{id}"),
    ("GET", "/sets/{id}/file"),
    ("POST", "/sets/{id}/copy"),
    ("GET", "/sets/{id}/sheets"),
    ("GET", "/sets/{id}/markups"),
    ("POST", "/sets/{id}/markups"),
    ("GET", "/sets/{id}/takeoff"),
    ("GET", "/sets/{id}/takeoff.csv"),
    ("GET", "/people"),
    ("POST", "/people"),
    ("POST", "/people/{id}/role"),
    ("POST", "/people/{id}/remove"),
    ("GET", "/chests"),
    ("POST", "/chests"),
    ("GET", "/notices"),
    ("POST", "/notices"),
    ("POST", "/notices/{id}/test"),
    ("POST", "/notices/{id}/remove"),
    ("GET", "/license"),
    ("POST", "/license"),
    ("GET", "/audit"),
    ("GET", "/audit.csv"),
    ("POST", "/admin/pin"),
    ("POST", "/admin/releases"),
    ("POST", "/admin/releases/{version}/ready"),
    ("GET", "/chests/{id}/file"),
    ("GET", "/plugins"),
    ("POST", "/plugins"),
    ("GET", "/plugins/{id}/file"),
    ("POST", "/plugins/{id}/run"),
    ("POST", "/plugins/{id}/remove"),
    ("GET", "/update/latest"),
    ("GET", "/update/{version}/download"),
];

// ---- is it there ----------------------------------------------------------

async fn health(State(server): State<Shared>) -> Json<Health> {
    // Claimed unless the server can prove otherwise. If the database cannot be
    // read, the honest answer is not "help yourself" — a client that is told a
    // server is unclaimed offers to take it over.
    let claimed = !server.store.is_empty().unwrap_or(false);
    Json(Health {
        ok: true,
        api_version: API_VERSION,
        version: VERSION.to_string(),
        name: installation_name(&server),
        claimed,
        joining: if claimed { how_joining(&server).0 } else { String::new() },
        // Only when it has been checked from the outside. Handing a seat an
        // address that does not answer is worse than handing it nothing: it
        // would try the broken one in a truck and conclude the server is
        // down.
        reachable_at: match server.tunnel.state_now() {
            crate::tunnel::State::On { address } => Some(address),
            _ => None,
        },
    })
}

async fn openapi(State(server): State<Shared>) -> Json<serde_json::Value> {
    Json(crate::openapi::document(&installation_name(&server)))
}

/// What this installation is called. Whoever set the server up named it, and
/// that beats whatever the machine it runs on was started with — the person who
/// typed "Example Steel" should see "Example Steel", not "Hyperview".
pub fn installation_name(server: &Server) -> String {
    server
        .store
        .setting(settings::COMPANY)
        .ok()
        .flatten()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| server.config.name.clone())
}

/// Where the answers to "who set this up and how do people get in" are kept.
pub mod settings {
    pub const COMPANY: &str = "company";
    pub const JOINING: &str = "joining";
    pub const JOIN_CODE: &str = "join_code";
    pub const JOIN_ROLE: &str = "join_role";
    /// When the join code stops working, as an RFC 3339 date and time.
    /// Empty means never, which is what every server did before this and
    /// what an administrator has to choose on purpose from now on.
    pub const JOIN_UNTIL: &str = "join_until";
    /// This installation's own name for itself, made once and kept. Not a
    /// secret and not a key: it exists so that one server heard twice on a
    /// network is recognised as one server.
    pub const INSTALLATION: &str = "installation";
}

/// This installation's id, made on first use and kept from then on.
pub fn installation_id(server: &Server) -> String {
    if let Ok(Some(id)) = server.store.setting(settings::INSTALLATION) {
        if !id.trim().is_empty() {
            return id;
        }
    }
    let fresh = fresh_id("inst");
    let _ = server.store.set_setting(settings::INSTALLATION, &fresh);
    fresh
}

/// How many days a change asked for, if it asked.
fn days_of(body: &ChangeJoining) -> Option<u32> {
    body.days.filter(|d| *d > 0)
}

/// A date that many days from now, as the settings store it.
fn in_days(days: u32) -> String {
    let when = time::OffsetDateTime::now_utc() + time::Duration::days(days as i64);
    when.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// Whether the join code has run out, and when it does.
///
/// A join code is one server-wide secret that names nobody and is not used
/// up. That was defensible while the only way to type it was to be standing
/// in the building; with a public address it is the weakest thing about the
/// server, because one code shared in one group chat in 2026 still works in
/// 2028. So it can now be given a date, and after that date it is simply not
/// a code any more.
///
/// Empty means never, which is what every existing server has. They are not
/// broken by this and nothing changes under them.
pub fn join_runs_out(server: &Server) -> Option<String> {
    server
        .store
        .setting(settings::JOIN_UNTIL)
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
}

/// True when the code has a date on it and that date has gone by.
pub fn join_has_run_out(server: &Server) -> bool {
    let Some(until) = join_runs_out(server) else {
        return false;
    };
    ran_out(&until, &crate::audit::now())
}

/// Kept apart from the clock so it can be tested without waiting a day.
fn ran_out(until: &str, now: &str) -> bool {
    // Both are RFC 3339 in UTC, which compares correctly as text. A date
    // nobody can parse is treated as run out rather than as forever: the
    // safe way round for a secret that lets somebody in.
    match (until.trim(), now.trim()) {
        ("", _) => false,
        (until, now) if until.len() < 10 => {
            let _ = now;
            true
        }
        (until, now) => until < now,
    }
}

/// How people get accounts, and the code if there is one.
fn how_joining(server: &Server) -> (String, String, String) {
    let how = server
        .store
        .setting(settings::JOINING)
        .ok()
        .flatten()
        .unwrap_or_else(|| "closed".into());
    let code = server
        .store
        .setting(settings::JOIN_CODE)
        .ok()
        .flatten()
        .unwrap_or_default();
    let role = server
        .store
        .setting(settings::JOIN_ROLE)
        .ok()
        .flatten()
        .unwrap_or_else(|| "estimator".into());
    (how, code, role)
}

// ---- signing in -----------------------------------------------------------

async fn sign_in(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<SignIn>,
) -> Answer<Json<Session>> {
    let email = body.email.trim().to_lowercase();
    let from = seen_from(&headers);

    // Asked before anything is looked up, so a refusal costs this server one
    // lock and no database work. Counted against the account and against the
    // address, because each catches what the other misses: one address
    // working through a list of people, and one account guessed at from a
    // hundred addresses.
    if let Some(owed) = server
        .patience
        .wait_for_any(crate::patience::SIGNING_IN, &[email.as_str(), from.as_str()])
    {
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, &email)
                .saying("too many wrong tries")
                .from(from.clone()),
        );
        return Err(Denied(
            StatusCode::TOO_MANY_REQUESTS,
            Problem::new("too_many", &crate::patience::refusal(owed)),
        ));
    }

    let session = server
        .store
        .with(|db| {
            let found: Option<(String, String, String, String)> = db
                .query_row(
                    "SELECT id, name, password, role FROM people WHERE lower(email) = ?1",
                    params![email],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )
                .optional()?;
            Ok(found)
        })
        .map_err(|e| Denied::broke("look you up", e))?;

    // The same answer whether the address is unknown or the password is wrong.
    // Telling somebody which one they got right is telling them half of it.
    let Some((id, name, stored, role)) = session else {
        // Somebody trying addresses is exactly what an administrator wants to
        // see in the log, and it was the one refusal that went unrecorded.
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, &email)
                .saying("no account with that address")
                .from(from.clone()),
        );
        // Counted even though there is no such account: somebody working
        // through a list of addresses is the case this is for, and not
        // counting it would leave the one door that answers instantly wide
        // open.
        server.patience.wrong(&email);
        server.patience.wrong(&from);
        return Err(Denied(
            StatusCode::UNAUTHORIZED,
            Problem::new("unauthorised", "That email address and password do not match."),
        ));
    };
    if !auth::password_matches(&body.password, &stored) {
        record(
            &server,
            crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, &body.email)
                .by(&id)
                .saying("the password did not match")
                .from(from.clone()),
        );
        server.patience.wrong(&email);
        server.patience.wrong(&from);
        return Err(Denied(
            StatusCode::UNAUTHORIZED,
            Problem::new("unauthorised", "That email address and password do not match."),
        ));
    }

    // Right. Whatever was held against them goes.
    server.patience.right(&email);
    server.patience.right(&from);

    let (token, hashed) = auth::new_token();
    let issued = time::OffsetDateTime::now_utc();
    let expires = issued + time::Duration::days(auth::SESSION_DAYS);
    let format = &time::format_description::well_known::Rfc3339;
    let (issued_text, expires_text) = (
        issued.format(format).unwrap_or_default(),
        expires.format(format).unwrap_or_default(),
    );
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, ?3, ?4)",
                params![hashed, id, issued_text, expires_text],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("start a session", e))?;

    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::SIGNED_IN, &body.email)
            .by(&id)
            .from(seen_from(&headers)),
    );

    Ok(Json(Session {
        token,
        expires_in: (auth::SESSION_DAYS * 24 * 60 * 60) as u64,
        user: User {
            id,
            name,
            email,
            role: auth::role_from(&role),
        },
        api_version: API_VERSION,
    }))
}

async fn me(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<User>> {
    Ok(Json(caller(&server, &headers)?.as_user()))
}

async fn change_password(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<hub::ChangePassword>,
) -> Answer<Json<serde_json::Value>> {
    let who = caller(&server, &headers)?;
    if body.new.chars().count() < 12 {
        return Err(Denied::wrong("A password needs twelve characters at the very least."));
    }
    let stored: String = server
        .store
        .with(|db| {
            Ok(db.query_row(
                "SELECT password FROM people WHERE id = ?1",
                params![who.id],
                |r| r.get(0),
            )?)
        })
        .map_err(|e| Denied::broke("look you up", e))?;
    if !auth::password_matches(&body.current, &stored) {
        return Err(Denied(
            StatusCode::UNAUTHORIZED,
            Problem::new("unauthorised", "That is not your current password."),
        ));
    }
    let hashed = auth::hash_password(&body.new).map_err(|e| Denied::broke("keep that password", e))?;
    // Every other place this person is signed in stops working: changing a
    // password is very often what somebody does because they think somebody
    // else has it.
    let keep = bearer(&headers).map(|t| auth::token_hash(&t)).unwrap_or_default();
    server
        .store
        .with(|db| {
            db.execute(
                "UPDATE people SET password = ?1 WHERE id = ?2",
                params![hashed, who.id],
            )?;
            db.execute(
                "DELETE FROM sessions WHERE person = ?1 AND token != ?2",
                params![who.id, keep],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("change that password", e))?;
    tracing::info!("{} changed their password", who.email);
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::CHANGED_PASSWORD, &who.email).by(&who.id),
    );
    Ok(Json(serde_json::json!({ "changed": true })))
}

fn update_settings(server: &Server) -> hub::UpdateSettings {
    use crate::updates::settings;
    let setting = |key: &str| server.store.setting(key).ok().flatten();
    let offering = server
        .store
        .with(|db| {
            // Every platform, not just this server's own: what the office is
            // offering is a version, and the Mac seats in it are offered the
            // same one as the Windows seats.
            let mut statement =
                db.prepare("SELECT DISTINCT version FROM releases WHERE ready = 1")?;
            let all = statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(all)
        })
        .unwrap_or_default()
        .into_iter()
        .max_by(|a, b| hub::update::compare(a, b));
    hub::UpdateSettings {
        running: crate::VERSION.to_string(),
        channel: match crate::updates::office_channel(server) {
            Channel::Preview => "early".into(),
            Channel::Stable => "stable".into(),
        },
        checked: setting(settings::CHECKED),
        note: setting(settings::NOTE),
        offering,
    }
}

async fn read_updates(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<hub::UpdateSettings>> {
    administrator(&server, &headers)?;
    Ok(Json(update_settings(&server)))
}

#[derive(Deserialize)]
pub struct ChangeUpdates {
    pub channel: String,
}

async fn change_updates(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeUpdates>,
) -> Answer<Json<hub::UpdateSettings>> {
    let who = administrator(&server, &headers)?;
    let channel = match body.channel.trim() {
        "stable" => "stable",
        "early" | "preview" => "preview",
        other => {
            return Err(Denied::wrong(format!(
                "'{other}' is not a way of getting updates. Use 'stable' or 'early'."
            )))
        }
    };
    server
        .store
        .set_setting(crate::updates::settings::CHANNEL, channel)
        .map_err(|e| Denied::broke("change how this server updates", e))?;
    tracing::info!("{} set updates to {channel}", who.email);
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::CHANGED_SETTING, &who.email)
            .by(&who.id)
            .about("update channel")
            .saying(channel),
    );
    Ok(Json(update_settings(&server)))
}

async fn read_fleet(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<hub::FleetAccess>> {
    administrator(&server, &headers)?;
    Ok(Json(hub::FleetAccess {
        on: crate::fleet::fleet_is_on(&server),
        key: None,
    }))
}

fn remote_now(server: &Server) -> hub::Remote {
    use crate::tunnel::State;
    let state = server.tunnel.state_now();
    let said = state.in_words();
    let hostname = crate::tunnel::held(&server.store).map(|(_, host)| host);
    hub::Remote {
        off: matches!(state, State::Off),
        sealed: matches!(state, State::Sealed),
        starting: matches!(state, State::Starting),
        on: matches!(state, State::On { .. }),
        hostname,
        said,
    }
}

async fn read_remote(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<hub::Remote>> {
    administrator(&server, &headers)?;
    Ok(Json(remote_now(&server)))
}

#[derive(Deserialize)]
pub struct ChangeRemote {
    pub token: String,
    pub hostname: String,
}

async fn change_remote(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeRemote>,
) -> Answer<Json<hub::Remote>> {
    let who = administrator(&server, &headers)?;
    // A sealed server refuses before anything is stored, so a token never
    // even lands on disk somewhere it could not be used.
    if hub::sealed::is_sealed() {
        return Err(Denied::wrong(
            "This server is sealed, so it makes no outward connections at all \u{2014} including this one. That is what the Sealed edition is.",
        ));
    }
    let token = crate::tunnel::a_token(&body.token).map_err(|e| Denied::wrong(e))?;
    let hostname =
        crate::tunnel::a_hostname(&body.hostname).map_err(|e| Denied::wrong(e))?;
    crate::tunnel::remember(&server.store, &token, &hostname)
        .map_err(|e| Denied::broke("keep the tunnel settings", e))?;
    // Whatever was running before belongs to the old token.
    server.tunnel.stop();
    crate::tunnel::start(&server.tunnel, &server.config.data, token, hostname.clone());
    // Recorded without the token. An audit line is read by people and kept
    // forever; the secret is not part of what happened.
    let _ = server.store.audit(
        &crate::audit::Entry::new("remote_access_on", &who.email)
            .by(&who.id)
            .about(hostname.clone()),
    );
    tracing::info!("{} turned remote access on at {hostname}", who.email);
    Ok(Json(remote_now(&server)))
}

async fn turn_remote_off(
    State(server): State<Shared>,
    headers: HeaderMap,
    _sent: Unused,
) -> Answer<Json<hub::Remote>> {
    let who = administrator(&server, &headers)?;
    server.tunnel.stop();
    crate::tunnel::forget(&server.store)
        .map_err(|e| Denied::broke("forget the tunnel settings", e))?;
    let _ = server.store.audit(
        &crate::audit::Entry::new("remote_access_off", &who.email).by(&who.id),
    );
    tracing::info!("{} turned remote access off", who.email);
    Ok(Json(remote_now(&server)))
}

#[derive(Deserialize)]
pub struct ChangeFleet {
    pub on: bool,
}

async fn change_fleet(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeFleet>,
) -> Answer<Json<hub::FleetAccess>> {
    let who = administrator(&server, &headers)?;
    let key = crate::fleet::set_fleet_access(&server, body.on)
        .map_err(|e| Denied::broke("change the Fleet's access", e))?;
    tracing::info!(
        "{} {} Excalibur Fleet's access",
        who.email,
        if body.on { "turned on" } else { "turned off" }
    );
    Ok(Json(hub::FleetAccess { on: body.on, key }))
}

// ---- projects -------------------------------------------------------------

// ---- drawing sets ---------------------------------------------------------

/// The columns a drawing set is read from, in the order [`set_from_row`]
/// takes them.
pub(crate) const SET_COLUMNS: &str = "id, project, name, filename, digest, bytes, sheets, uploaded, \
                                      uploaded_by, revision, supersedes, superseded_by, folder";

pub(crate) fn set_from_row(r: &rusqlite::Row) -> rusqlite::Result<DrawingSet> {
    Ok(DrawingSet {
        id: r.get(0)?,
        project: r.get(1)?,
        name: r.get(2)?,
        filename: r.get(3)?,
        digest: r.get(4)?,
        bytes: r.get::<_, i64>(5)? as u64,
        sheets: r.get::<_, i64>(6)? as usize,
        uploaded: r.get(7)?,
        uploaded_by: r.get(8)?,
        revision: r.get::<_, i64>(9)? as u64,
        supersedes: r.get(10)?,
        superseded_by: r.get(11)?,
        already_here: false,
        carried_forward: 0,
        carry_note: None,
        folder: r.get(12)?,
    })
}

/// The one gate for a project and everything hanging off it.
///
/// Every route that reaches a project, a set, a sheet, a markup or a takeoff
/// goes through this or through [`may_reach_set`]. The rule lives in the
/// store so a listing and a direct fetch can never disagree -- and the answer
/// when somebody may not is the same "not found" they would get for a project
/// that does not exist, so nobody learns a job exists by being refused it.
pub(crate) fn may_reach_project(server: &Server, who: &Who, project: &str) -> Answer<()> {
    let allowed = server
        .store
        .may_see(project, &who.id, who.role == Role::Admin)
        .map_err(|e| Denied::broke("check who is on that project", e))?;
    if allowed {
        Ok(())
    } else {
        Err(Denied::missing("project"))
    }
}

/// The same, for anything named by a drawing set: the set's own project
/// decides.
pub(crate) fn may_reach_set(server: &Server, who: &Who, set: &str) -> Answer<()> {
    let project = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT project FROM sets WHERE id = ?1",
                    params![set],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that drawing set", e))?;
    match project {
        // A set nobody can find is refused the same way as one somebody may
        // not have, so the two are indistinguishable from outside.
        None => Err(Denied::missing("drawing set")),
        Some(project) => may_reach_project(server, who, &project),
    }
}

pub(crate) fn read_set(db: &Connection, id: &str) -> rusqlite::Result<Option<DrawingSet>> {
    db.query_row(
        &format!("SELECT {SET_COLUMNS} FROM sets WHERE id = ?1"),
        params![id],
        set_from_row,
    )
    .optional()
}

#[derive(Deserialize, Default)]
pub struct Everything {
    /// Include what is normally left off: archived projects, superseded sets.
    #[serde(default, deserialize_with = "said_yes")]
    pub all: bool,
}

async fn sets_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(everything): Query<Everything>,
) -> Answer<Json<Vec<DrawingSet>>> {
    let who = caller(&server, &headers)?;
    may_reach_project(&server, &who, &project)?;
    let list = server
        .store
        .with(|db| {
            // A drawing issued again replaces the one before it on the list.
            // The one before is still there, with its markups, for anybody who
            // asks for everything.
            let mut statement = db.prepare(&format!(
                "SELECT {SET_COLUMNS} FROM sets
                 WHERE project = ?1 AND (?2 OR superseded_by IS NULL)
                 ORDER BY uploaded DESC"
            ))?;
            let rows = statement
                .query_map(params![project, everything.all], set_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the drawing sets", e))?;
    Ok(Json(list))
}

async fn one_set(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<DrawingSet>> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let found = server
        .store
        .with(|db| Ok(read_set(db, &id)?))
        .map_err(|e| Denied::broke("read that drawing set", e))?;
    found.map(Json).ok_or_else(|| Denied::missing("drawing set"))
}

async fn upload_set(
    State(server): State<Shared>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Answer<Json<DrawingSet>> {
    let who = writer(&server, &headers)?;
    let mut project = String::new();
    let mut name = String::new();
    let mut filename = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    let mut carry = false;
    let mut folder = String::new();

    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| Denied::wrong(format!("That upload was malformed: {e}")))?
    {
        match field.name().unwrap_or("") {
            "project" => project = field.text().await.unwrap_or_default(),
            "name" => name = field.text().await.unwrap_or_default(),
            "folder" => folder = field.text().await.unwrap_or_default(),
            "carry_forward" => {
                let said = field.text().await.unwrap_or_default();
                carry = matches!(said.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes");
            }
            "file" => {
                filename = field.file_name().unwrap_or("drawing.pdf").to_string();
                bytes = field
                    .bytes()
                    .await
                    .map_err(|e| Denied::wrong(format!("That file did not arrive whole: {e}")))?
                    .to_vec();
            }
            _ => {}
        }
    }

    if bytes.is_empty() {
        return Err(Denied::wrong("No file came with that upload."));
    }
    if !bytes.starts_with(b"%PDF") {
        return Err(Denied::wrong(
            "That is not a PDF. Excalibur View holds drawing sets as PDFs, because the PDF is the \
             file of record.",
        ));
    }
    let folder = crate::projects::folders(&folder, "A drawing's folder").map_err(Denied::wrong)?;
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
        .map_err(|e| Denied::broke("check that project", e))?;
    if !known {
        return Err(Denied::missing("project"));
    }

    // Read the sheets out of the drawing now, so a client that only wants the
    // list does not have to download sixty megabytes to get it.
    let document = pdf::Document::from_bytes(bytes.clone());
    let count = document.page_count();
    let mut sheets = Vec::with_capacity(count);
    for page in 0..count {
        let Some(dict) = document.page(page) else {
            continue;
        };
        let area = document.page_box(&dict);
        let rotation = document.page_rotation(&dict);
        let frame = annot::Frame::new(area, rotation);
        let (width, height) = frame.size();
        let scale = annot::viewport::scale_of(&document, &dict).map(|m| m.ratio);
        sheets.push(Sheet {
            page,
            number: String::new(),
            title: String::new(),
            width,
            height,
            rotation,
            scale,
        });
    }

    let digest = server
        .store
        .put_blob(&bytes)
        .map_err(|e| Denied::broke("store that drawing", e))?;

    let name = if name.trim().is_empty() {
        filename.clone()
    } else {
        name.trim().to_string()
    };
    let mut set = DrawingSet {
        id: fresh_id("set"),
        project,
        name,
        filename,
        digest,
        bytes: bytes.len() as u64,
        sheets: sheets.len(),
        uploaded: now(),
        uploaded_by: who.name.clone(),
        revision: 0,
        supersedes: None,
        superseded_by: None,
        already_here: false,
        carried_forward: 0,
        carry_note: None,
        folder,
    };

    let settled = server
        .store
        .with(|db| {
            // One step: a set half-recorded — sheets without the set, or a
            // new issue that has not yet replaced the old — is never visible.
            let tx = db.unchecked_transaction()?;
            let db: &Connection = &tx;
            // The same drawing, byte for byte, already in this project: that
            // one is the answer. A push that is retried, or run twice from two
            // screens, does not leave two copies of every sheet behind.
            if let Some(mut there) = crate::projects::same_drawing(db, &set.project, &set.digest)? {
                there.already_here = true;
                return Ok(there);
            }
            // The same drawing issued again: it takes the old one's place.
            let before =
                crate::projects::issued_before(db, &set.project, &set.name, set.folder.as_deref())?;
            set.supersedes = before.as_ref().map(|b| b.id.clone());
            // The issue before it named its folder first: the new one keeps
            // that spelling, so a folder doesn't show twice for its case. One
            // sent before folders were takes the new one's.
            if let Some(folder) = before.as_ref().and_then(|b| b.folder.clone()) {
                set.folder = Some(folder);
            }
            db.execute(
                "INSERT INTO sets (id, project, name, filename, digest, bytes, sheets,
                                   uploaded, uploaded_by, revision, supersedes, folder)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10, ?11)",
                params![
                    set.id,
                    set.project,
                    set.name,
                    set.filename,
                    set.digest,
                    set.bytes as i64,
                    set.sheets as i64,
                    set.uploaded,
                    set.uploaded_by,
                    set.supersedes,
                    set.folder
                ],
            )?;
            for sheet in &sheets {
                db.execute(
                    "INSERT INTO sheets (set_id, page, number, title, width, height, rotation, scale)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        set.id,
                        sheet.page as i64,
                        sheet.number,
                        sheet.title,
                        sheet.width,
                        sheet.height,
                        sheet.rotation as i64,
                        sheet.scale
                    ],
                )?;
            }
            if let Some(before) = before {
                db.execute(
                    "UPDATE sets SET superseded_by = ?1 WHERE id = ?2",
                    params![set.id, before.id],
                )?;
                if carry {
                    let (carried, note) =
                        crate::projects::carry_forward(db, &before.id, &set.id, &sheets)?;
                    set.carried_forward = carried;
                    set.carry_note = note;
                    if carried > 0 {
                        set.revision = read_set(db, &set.id)?.map(|s| s.revision).unwrap_or(0);
                    }
                }
            }
            tx.commit()?;
            Ok(set.clone())
        })
        .map_err(|e| Denied::broke("record that drawing set", e))?;

    Ok(Json(settled))
}

async fn set_file(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Response> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let found = server
        .store
        .with(|db| Ok(read_set(db, &id)?))
        .map_err(|e| Denied::broke("read that drawing set", e))?;
    let set = found.ok_or_else(|| Denied::missing("drawing set"))?;

    // Reaching a drawing is the line an auditor most wants to see.
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::DOWNLOADED_SET, &who.email)
            .by(&who.id)
            .about(set.name.clone())
            .saying(format!("{} sheets", set.sheets)),
    );

    // A client that already has these exact bytes is told so and downloads
    // nothing. Six people opening a sixty-megabyte set every morning is four
    // hundred megabytes that need not move.
    if let Some(have) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if have.trim_matches('"') == set.digest {
            return Ok(StatusCode::NOT_MODIFIED.into_response());
        }
    }

    let bytes = server
        .store
        .get_blob(&set.digest)
        .map_err(|e| Denied::broke("read that drawing", e))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/pdf".to_string()),
            (header::ETAG, format!("\"{}\"", set.digest)),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{}\"", safe_filename(&set.filename)),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

/// A filename going into a header, with anything that could break out of the
/// quoting taken out.
fn safe_filename(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '"' | '\\' | '\r' | '\n'))
        .take(120)
        .collect()
}

pub(crate) fn sheets_for(db: &Connection, set: &str) -> rusqlite::Result<Vec<Sheet>> {
    let mut statement = db.prepare(
        "SELECT page, number, title, width, height, rotation, scale
         FROM sheets WHERE set_id = ?1 ORDER BY page",
    )?;
    let rows: Vec<Sheet> = statement
        .query_map(params![set], |r| {
            Ok(Sheet {
                page: r.get::<_, i64>(0)? as usize,
                number: r.get(1)?,
                title: r.get(2)?,
                width: r.get(3)?,
                height: r.get(4)?,
                rotation: r.get::<_, i64>(5)? as i32,
                scale: r.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

async fn sheets_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<Vec<Sheet>>> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let list = server
        .store
        .with(|db| Ok(sheets_for(db, &id)?))
        .map_err(|e| Denied::broke("list those sheets", e))?;
    if list.is_empty() {
        let exists = server
            .store
            .with(|db| Ok(read_set(db, &id)?))
            .map_err(|e| Denied::broke("read that drawing set", e))?;
        if exists.is_none() {
            return Err(Denied::missing("drawing set"));
        }
    }
    Ok(Json(list))
}

// ---- markups --------------------------------------------------------------

#[derive(Deserialize)]
pub struct Since {
    #[serde(default)]
    pub since: u64,
}

fn markups_since(db: &Connection, set: &str, since: u64) -> rusqlite::Result<Vec<hub::Markup>> {
    use base64::Engine;
    let mut statement = db.prepare(
        "SELECT id, page, subject, kind, caption, author, colour, created, dictionary,
                revision, removed
         FROM markups WHERE set_id = ?1 AND revision > ?2 ORDER BY revision, id",
    )?;
    let rows: Vec<hub::Markup> = statement
        .query_map(params![set, since as i64], |r| {
            let bytes: Vec<u8> = r.get(8)?;
            Ok(hub::Markup {
                id: r.get(0)?,
                page: r.get::<_, i64>(1)? as usize,
                subject: r.get(2)?,
                kind: r.get(3)?,
                caption: r.get(4)?,
                author: r.get(5)?,
                colour: r.get(6)?,
                created: r.get(7)?,
                dictionary: base64::engine::general_purpose::STANDARD.encode(&bytes),
                revision: r.get::<_, i64>(9)? as u64,
                removed: r.get::<_, i64>(10)? != 0,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

async fn markups_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(since): Query<Since>,
) -> Answer<Json<MarkupPage>> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let page = server
        .store
        .with(|db| {
            let Some(set) = read_set(db, &id)? else {
                return Ok(None);
            };
            Ok(Some(MarkupPage {
                markups: markups_since(db, &id, since.since)?,
                revision: set.revision,
            }))
        })
        .map_err(|e| Denied::broke("read those markups", e))?;
    page.map(Json).ok_or_else(|| Denied::missing("drawing set"))
}

async fn add_markups(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(incoming): Json<Vec<NewMarkup>>,
) -> Answer<Json<MarkupPage>> {
    use base64::Engine;
    let who = writer(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;

    // Everything is parsed and checked before anything is written, so a batch
    // with one bad markup in it does not leave half of itself in the file.
    let mut ready = Vec::with_capacity(incoming.len());
    for one in &incoming {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(one.dictionary.as_bytes())
            .map_err(|_| Denied::wrong("A markup's dictionary was not valid base64."))?;
        let markup = quantities::read_markup(&bytes).ok_or_else(|| {
            Denied::wrong(
                "A markup's dictionary could not be read as a PDF annotation. Nothing was saved.",
            )
        })?;
        ready.push((one, bytes, markup));
    }

    let now_text = now();
    let result = server
        .store
        .with(|db| {
            if read_set(db, &id)?.is_none() {
                return Ok(None);
            }
            let sheets = sheets_for(db, &id)?;
            let revision = Store::bump(db, &id)?;

            for (one, bytes, markup) in &ready {
                // A replacement is a removal and an addition, so a client
                // syncing from an older revision sees both halves.
                if let Some(old) = &one.replaces {
                    db.execute(
                        "UPDATE markups SET removed = 1, revision = ?1
                         WHERE id = ?2 AND set_id = ?3",
                        params![revision as i64, old, id],
                    )?;
                }
                let scale = sheets
                    .get(one.page)
                    .and_then(|s| s.scale.as_ref())
                    .map(|_| ());
                let caption = if scale.is_some() {
                    markup.contents()
                } else {
                    // No scale on that sheet means no number, here as well as
                    // in the viewer.
                    String::new()
                };
                let colour = markup.colour();
                db.execute(
                    "INSERT INTO markups (id, set_id, page, subject, kind, caption, author,
                                          colour, created, dictionary, revision, removed)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)",
                    params![
                        fresh_id("mk"),
                        id,
                        one.page as i64,
                        markup.subject(),
                        markup.kind().name(),
                        caption,
                        who.name,
                        format!(
                            "#{:02X}{:02X}{:02X}",
                            (colour[0] * 255.0) as u8,
                            (colour[1] * 255.0) as u8,
                            (colour[2] * 255.0) as u8
                        ),
                        now_text,
                        bytes,
                        revision as i64
                    ],
                )?;
            }

            Ok(Some(MarkupPage {
                markups: markups_since(db, &id, revision - 1)?,
                revision,
            }))
        })
        .map_err(|e| Denied::broke("save those markups", e))?;

    if result.is_some() {
        crate::notices::changed(&server, &id);
    }
    result.map(Json).ok_or_else(|| Denied::missing("drawing set"))
}

// ---- change notices -------------------------------------------------------

#[derive(Deserialize)]
struct NewNotice {
    url: String,
}

fn notice_rows(db: &Connection) -> rusqlite::Result<Vec<ChangeNotice>> {
    let mut q = db.prepare(
        "SELECT id, url, created, created_by, last_status, last_at FROM notices ORDER BY created",
    )?;
    let rows = q.query_map([], |r| {
        Ok(ChangeNotice {
            id: r.get(0)?,
            url: r.get(1)?,
            created: r.get(2)?,
            created_by: r.get(3)?,
            last_status: r.get(4)?,
            last_at: r.get(5)?,
            secret: None,
        })
    })?;
    rows.collect()
}

/// Where the office tells other programs a takeoff changed. Administrators only.
async fn list_notices(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<ChangeNotice>>> {
    administrator(&server, &headers)?;
    let list = server
        .store
        .with(|db| Ok(notice_rows(db)?))
        .map_err(|e| Denied::broke("list the notice addresses", e))?;
    Ok(Json(list))
}

/// A new address to tell. The secret its notices are signed with comes back
/// this once.
async fn add_notice(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(asked): Json<NewNotice>,
) -> Answer<Json<ChangeNotice>> {
    let who = administrator(&server, &headers)?;
    let url = asked.url.trim().to_string();
    crate::notices::acceptable(&url).map_err(Denied::wrong)?;
    let notice = ChangeNotice {
        id: fresh_id("ntc"),
        url,
        created: now(),
        created_by: who.name.clone(),
        last_status: None,
        last_at: None,
        secret: Some(crate::notices::secret()),
    };
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO notices (id, url, secret, created, created_by) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    notice.id,
                    notice.url,
                    notice.secret.as_deref().unwrap_or_default(),
                    notice.created,
                    notice.created_by
                ],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("keep that notice address", e))?;
    tracing::info!("{} added a change notice to {}", who.name, notice.url);
    Ok(Json(notice))
}

/// Sends a test notice now, and says how it went.
async fn test_notice(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _sent: Unused,
) -> Answer<Json<serde_json::Value>> {
    administrator(&server, &headers)?;
    let tried = server.clone();
    let status = tokio::task::spawn_blocking(move || crate::notices::test(&tried, &id))
        .await
        .map_err(|e| Denied::broke("send a test notice", e))?
        .map_err(Denied::wrong)?;
    Ok(Json(serde_json::json!({ "status": status })))
}

async fn remove_notice(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _sent: Unused,
) -> Answer<Json<serde_json::Value>> {
    let who = administrator(&server, &headers)?;
    let gone = server
        .store
        .with(|db| Ok(db.execute("DELETE FROM notices WHERE id = ?1", params![id])?))
        .map_err(|e| Denied::broke("remove that notice address", e))?;
    if gone == 0 {
        return Err(Denied::missing("notice address"));
    }
    tracing::info!("{} removed change notice {id}", who.name);
    Ok(Json(serde_json::json!({ "removed": id })))
}

// ---- the takeoff ----------------------------------------------------------

fn gather(server: &Server, id: &str) -> Answer<(u64, Vec<Stored>, Vec<SheetScale>)> {
    let found = server
        .store
        .with(|db| {
            let Some(set) = read_set(db, id)? else {
                return Ok(None);
            };
            let sheets = sheets_for(db, id)?
                .into_iter()
                .map(|s| SheetScale {
                    number: if s.number.is_empty() {
                        format!("Sheet {}", s.page + 1)
                    } else {
                        s.number
                    },
                    // The scale is stored as the words the drawing uses. It is
                    // turned back into a measure by the same code the viewer
                    // uses, so the two cannot diverge.
                    scale: s.scale.as_deref().and_then(scale_named),
                })
                .collect::<Vec<_>>();

            let mut statement = db.prepare(
                "SELECT id, page, dictionary, author FROM markups
                 WHERE set_id = ?1 AND removed = 0 ORDER BY revision, id",
            )?;
            let markups = statement
                .query_map(params![id], |r| {
                    Ok(Stored {
                        id: r.get(0)?,
                        page: r.get::<_, i64>(1)? as usize,
                        dictionary: r.get(2)?,
                        author: r.get(3)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Some((set.revision, markups, sheets)))
        })
        .map_err(|e| Denied::broke("gather that takeoff", e))?;
    found.ok_or_else(|| Denied::missing("drawing set"))
}

/// The named scales, as words on a drawing.
///
/// A sheet whose scale is not one of these — a calibrated one, say — measures
/// from the `/Measure` dictionary on each markup, which every measurement
/// carries. Nothing here guesses a scale from a name it does not recognise.
fn scale_named(text: &str) -> Option<annot::measure::Measure> {
    const NAMED: &[(&str, f64)] = &[
        ("3\" = 1'-0\"", 4.0),
        ("1 1/2\" = 1'-0\"", 8.0),
        ("1\" = 1'-0\"", 12.0),
        ("3/4\" = 1'-0\"", 16.0),
        ("1/2\" = 1'-0\"", 24.0),
        ("3/8\" = 1'-0\"", 32.0),
        ("1/4\" = 1'-0\"", 48.0),
        ("3/16\" = 1'-0\"", 64.0),
        ("1/8\" = 1'-0\"", 96.0),
        ("3/32\" = 1'-0\"", 128.0),
        ("1/16\" = 1'-0\"", 192.0),
    ];
    NAMED
        .iter()
        .find(|(name, _)| *name == text)
        .map(|(name, ratio)| annot::measure::imperial(*ratio, name, 16))
}

async fn takeoff_of(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<Takeoff>> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let (revision, markups, sheets) = gather(&server, &id)?;
    let result = quantities::takeoff(&id, revision, &markups, &sheets)
        .map_err(|e| Denied::broke("work out that takeoff", e))?;
    Ok(Json(result))
}

async fn takeoff_csv(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Response> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let (_, markups, sheets) = gather(&server, &id)?;
    // A takeoff leaving the server is the thing an estimator takes to another
    // job, and it was the one export nobody could see had happened.
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::EXPORTED_TAKEOFF, &who.email)
            .by(&who.id)
            .about(&id)
            .saying("as a spreadsheet")
            .from(seen_from(&headers)),
    );
    let columns = [
        takeoff::Column::Page,
        takeoff::Column::Subject,
        takeoff::Column::Measurement,
        takeoff::Column::Quantity,
        takeoff::Column::Count,
        takeoff::Column::Length,
        takeoff::Column::Area,
        takeoff::Column::Pounds,
        takeoff::Column::Tons,
        takeoff::Column::Author,
    ];
    let csv = quantities::to_csv(&markups, &sheets, &columns);
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/csv; charset=utf-8")],
        csv,
    )
        .into_response())
}

// ---- keys for programs ------------------------------------------------------

async fn make_key(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(asked): Json<hub::NewKey>,
) -> Answer<Json<hub::ApiKey>> {
    let who = person(&server, &headers)?;
    let purpose = match asked.purpose.trim() {
        p if p == auth::purpose::ASSISTANT => auth::purpose::ASSISTANT,
        p if p == auth::purpose::INTEGRATION => {
            if !who.role.may_administer() {
                return Err(Denied::forbidden("make a key for another system"));
            }
            auth::purpose::INTEGRATION
        }
        _ => return Err(Denied::wrong("A key is for an `assistant` or an `integration`.")),
    };
    let name = asked.name.trim();
    if name.is_empty() {
        return Err(Denied::wrong("Give the key a name, so it can be recognised later."));
    }
    let (key, digest) = auth::fresh_key();
    let id = fresh_id("key");
    let made = now();
    server
        .store
        .with(|db| {
            // One key per person, purpose and name: asking again replaces it,
            // which is what "connect again" means.
            db.execute(
                "DELETE FROM keys WHERE person = ?1 AND purpose = ?2 AND name = ?3",
                params![who.id, purpose, name],
            )?;
            db.execute(
                "INSERT INTO keys (id, digest, person, purpose, name, made) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, digest, who.id, purpose, name, made],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("make that key", e))?;
    tracing::info!("{} made a {purpose} key called {name}", who.email);
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::MADE_KEY, &who.email)
            .by(&who.id)
            .about(name)
            .saying(purpose.to_string()),
    );
    Ok(Json(hub::ApiKey {
        id,
        name: name.to_string(),
        purpose: purpose.to_string(),
        person: who.name,
        made,
        last_used: None,
        key: Some(key),
    }))
}

async fn list_keys(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<hub::ApiKey>>> {
    let who = person(&server, &headers)?;
    let everybody = who.role.may_administer();
    let list = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT keys.id, keys.name, keys.purpose, people.name, keys.made, keys.last_used
                 FROM keys JOIN people ON people.id = keys.person
                 WHERE ?1 OR keys.person = ?2
                 ORDER BY keys.made DESC",
            )?;
            let rows = statement
                .query_map(params![everybody, who.id], |r| {
                    Ok(hub::ApiKey {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        purpose: r.get(2)?,
                        person: r.get(3)?,
                        made: r.get(4)?,
                        last_used: r.get(5)?,
                        key: None,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the keys", e))?;
    Ok(Json(list))
}

async fn revoke_key(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _sent: Unused,
) -> Answer<Json<serde_json::Value>> {
    let who = person(&server, &headers)?;
    let gone = server
        .store
        .with(|db| {
            Ok(db.execute(
                "DELETE FROM keys WHERE id = ?1 AND (person = ?2 OR ?3)",
                params![id, who.id, who.role.may_administer()],
            )?)
        })
        .map_err(|e| Denied::broke("take that key back", e))?;
    if gone == 0 {
        return Err(Denied::missing("key"));
    }
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::REVOKED_KEY, &who.email)
            .by(&who.id)
            .about(&id),
    );
    Ok(Json(serde_json::json!({ "revoked": id })))
}

// ---- the markup list, the way Bluebeam hands one over ------------------------

/// The custom columns the office's tool chest defines — "LBS Per FT" and the
/// rest — read from the newest shared chest that names any. What the markup
/// list calls them, and which of them are unit weights.
///
/// Any kind of chest counts: "Share one" puts an `.evtools` on the server, and
/// a Revu profile (`.bpx`) carries names too. A Revu tool set (`.btx`) names
/// none, so an older chest that does is used instead of it.
fn office_columns(server: &Server) -> Vec<takeoff::user::UserColumn> {
    let digests: Vec<String> = server
        .store
        .with(|db| {
            let mut q = db.prepare("SELECT digest FROM chests WHERE shared = 1 ORDER BY uploaded DESC")?;
            let rows = q.query_map([], |r| r.get::<_, String>(0))?;
            Ok(rows.filter_map(Result::ok).collect())
        })
        .unwrap_or_default();
    for digest in digests {
        let Ok(bytes) = server.store.get_blob(&digest) else { continue };
        let Some(profile) = chest::Profile::read(&bytes) else { continue };
        if profile.custom_columns.is_empty() {
            continue;
        }
        return profile
            .custom_columns
            .into_iter()
            .map(|c| takeoff::user::UserColumn {
                index: c.index,
                name: c.name.clone(),
                formula: c.is_formula().then(|| c.expression.clone()),
                precision: c.precision,
            })
            .collect();
    }
    Vec::new()
}

fn markuplist_of(server: &Server, id: &str, user: &[takeoff::user::UserColumn]) -> Answer<serde_json::Value> {
    let (set, markups, sheets, carried) = {
        let found = server
            .store
            .with(|db| read_set(db, id).map_err(Into::into))
            .map_err(|e| Denied::broke("read that drawing set", e))?;
        let set = found.ok_or_else(|| Denied::missing("drawing set"))?;
        let (_, markups, sheets) = gather(server, id)?;
        let carried: std::collections::HashSet<String> = server
            .store
            .with(|db| {
                let mut statement = db.prepare(
                    "SELECT id FROM markups
                     WHERE set_id = ?1 AND removed = 0 AND carried_from IS NOT NULL",
                )?;
                let ids = statement
                    .query_map(params![id], |r| r.get::<_, String>(0))?
                    .collect::<Result<_, _>>()?;
                Ok(ids)
            })
            .map_err(|e| Denied::broke("read that drawing set", e))?;
        (set, markups, sheets, carried)
    };
    let measured = quantities::measured(&markups, &sheets);
    let rows: Vec<takeoff::Row> = measured.iter().map(|(_, row, _)| row.clone()).collect();
    let lines: Vec<takeoff::export::Line> = rows
        .iter()
        .map(|row| takeoff::export::Line {
            row,
            sheet: sheets
                .get(row.page)
                .map(|s| s.number.clone())
                .filter(|n| !n.is_empty())
                .unwrap_or_else(|| format!("{}", row.page + 1)),
            drawing: set.name.clone(),
        })
        .collect();
    let listing = takeoff::export::Listing {
        title: set.name.clone(),
        who: String::new(),
        when: now(),
        columns: Vec::new(),
        user: user.to_vec(),
        weights: takeoff::user::weight_columns(user),
        lines,
        group: takeoff::export::GroupBy::Subject,
    };
    let mut answer = listing.to_json();
    // Beside what a Bluebeam reader already reads, the same numbers with
    // nothing left to parse: which markup, what kind, how many, how long, in
    // stated units — and whether it was measured on this issue or brought
    // forward from the last one and not yet looked at.
    let mut brought = 0usize;
    let mut unchecked = 0usize;
    if let Some(list) = answer["Markups"].as_array_mut() {
        for (entry, (markup, row, _)) in list.iter_mut().zip(measured.iter()) {
            let each = if row.kind == annot::Kind::Count {
                row.count * row.quantity
            } else {
                row.quantity
            };
            let carried_forward = carried.contains(markup);
            let needs_check = row.status == crate::projects::CARRIED;
            brought += carried_forward as usize;
            unchecked += needs_check as usize;
            entry["MarkupId"] = serde_json::json!(markup);
            entry["Hyperview"] = serde_json::json!({
                "MarkupId": markup,
                "Set": set.id,
                "PageIndex": row.page,
                "Kind": row.kind.name(),
                "Scaled": row.scaled,
                "Quantity": row.quantity,
                "Each": each,
                "LengthFt": row.scaled.then_some(row.length).flatten(),
                "AreaSqFt": row.scaled.then_some(row.area).flatten(),
                "VolumeCuFt": row.scaled.then_some(row.volume).flatten(),
                "Status": row.status,
                "CarriedForward": carried_forward,
                "NeedsCheck": needs_check,
            });
        }
    }
    answer["Set"] = serde_json::Value::String(set.id.clone());
    answer["File"] = serde_json::Value::String(set.filename.clone());
    answer["Revision"] = serde_json::json!(set.revision);
    answer["CarriedForward"] = serde_json::json!(brought);
    answer["NeedsCheck"] = serde_json::json!(unchecked);
    if let Some(newer) = &set.superseded_by {
        answer["SupersededBy"] = serde_json::json!(newer);
    }
    Ok(answer)
}

/// One set's markups: `Markups`, each with `Subject`, `Comment` (the
/// measurement, as Revu writes it) and the custom columns under
/// `ExtendedProperties` — what anything that reads a Bluebeam markup list
/// already reads — plus the totals, and the warning when the takeoff is short.
async fn set_markuplist(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Json<serde_json::Value>> {
    let who = caller(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let user = office_columns(&server);
    Ok(Json(markuplist_of(&server, &id, &user)?))
}

/// Every set in a project that counts, one after another: `{ "results": [ {
/// "file", "set", "markups": { "Markups": [...] } } ] }` — the shape a whole
/// bid's pull comes back in.
///
/// A drawing issued again is read from its newest issue only; the ones it
/// replaced are named under `superseded`, with how many markups each still
/// holds, so a takeoff left behind on an old issue is never silently missing.
/// `?all=1` reads those as well.
async fn project_markuplist(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(project): Path<String>,
    Query(everything): Query<Everything>,
) -> Answer<Json<serde_json::Value>> {
    let who = caller(&server, &headers)?;
    may_reach_project(&server, &who, &project)?;
    type Listed = (String, String, Option<String>, i64);
    let (sets, stamp): (Vec<Listed>, Option<crate::projects::Stamp>) = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT s.id, s.name, s.superseded_by,
                        (SELECT count(*) FROM markups m WHERE m.set_id = s.id AND m.removed = 0)
                 FROM sets s WHERE s.project = ?1 ORDER BY s.uploaded",
            )?;
            let rows = statement
                .query_map(params![project], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok((rows, crate::projects::stamp_of(db, &project)?))
        })
        .map_err(|e| Denied::broke("list that project's drawing sets", e))?;
    let Some(stamp) = stamp else {
        return Err(Denied::missing("project"));
    };
    let user = office_columns(&server);
    let mut results = Vec::new();
    let mut superseded = Vec::new();
    for (id, name, newer, markups) in sets {
        if let (Some(newer), false) = (&newer, everything.all) {
            superseded.push(serde_json::json!({
                "file": name, "set": id, "supersededBy": newer, "markups": markups,
            }));
            continue;
        }
        let list = markuplist_of(&server, &id, &user)?;
        results.push(serde_json::json!({ "file": name, "set": id, "markups": list }));
    }
    Ok(Json(serde_json::json!({
        "project": project,
        "stamp": stamp.stamp,
        "archived": stamp.archived,
        "carriedForward": stamp.carried_forward,
        "results": results,
        "superseded": superseded,
    })))
}

// ---- shared tool chests ---------------------------------------------------

async fn chests(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<Chest>>> {
    caller(&server, &headers)?;
    let list = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT id, name, filename, digest, bytes, tools, sets, uploaded, shared
                 FROM chests ORDER BY name",
            )?;
            let rows = statement
                .query_map([], |r| {
                    Ok(Chest {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        filename: r.get(2)?,
                        digest: r.get(3)?,
                        bytes: r.get::<_, i64>(4)? as u64,
                        tools: r.get::<_, i64>(5)? as usize,
                        sets: r.get::<_, i64>(6)? as usize,
                        uploaded: r.get(7)?,
                        shared: r.get::<_, i64>(8)? != 0,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the tool chests", e))?;
    Ok(Json(list))
}

async fn chest_file(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Response> {
    caller(&server, &headers)?;
    let digest = server
        .store
        .with(|db| {
            Ok(db
                .query_row("SELECT digest FROM chests WHERE id = ?1", params![id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that tool chest", e))?;
    let digest = digest.ok_or_else(|| Denied::missing("tool chest"))?;
    let bytes = server
        .store
        .get_blob(&digest)
        .map_err(|e| Denied::broke("read that tool chest", e))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}

// ---- plugins ---------------------------------------------------------------
//
// An office's own tools. The server holds them and hands them out; it never
// runs one. It does check the signature on the way in, against the same keys
// a release must be signed by, so an office server cannot become the place a
// plugin gets trusted — and every seat checks again before loading one.

async fn plugins(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<hub::PluginInfo>>> {
    caller(&server, &headers)?;
    let list = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT id, name, version, digest, bytes, uploaded, uploaded_by, key
                 FROM plugins ORDER BY name",
            )?;
            let rows = statement
                .query_map([], |r| {
                    Ok(hub::PluginInfo {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        version: r.get(2)?,
                        digest: r.get(3)?,
                        bytes: r.get::<_, i64>(4)? as u64,
                        uploaded: r.get(5)?,
                        uploaded_by: r.get(6)?,
                        key: r.get(7)?,
                        manifest: None,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the plugins", e))?;
    let mut list = list;
    for plugin in list.iter_mut() {
        plugin.manifest = manifest_of(&server, &plugin.digest).await;
    }
    Ok(Json(list))
}

/// A plugin's manifest, read in the sandbox the first time it is asked for
/// and kept. `None` when the file can't be read or won't say.
async fn manifest_of(server: &Shared, digest: &str) -> Option<plugin_api::Manifest> {
    if let Some(known) = server.manifests.lock().ok()?.get(digest) {
        return known.clone();
    }
    let bytes = server.store.get_blob(digest).ok()?;
    let read = tokio::task::spawn_blocking(move || {
        let (_, wasm) = plugin_api::open(&bytes).ok()?;
        plugin_host::manifest_of(wasm).ok()
    })
    .await
    .ok()
    .flatten();
    if let Ok(mut known) = server.manifests.lock() {
        known.insert(digest.to_string(), read.clone());
    }
    read
}

/// Runs a plugin here, for a seat that can't run one itself: the Mac App
/// Store copy, which isn't allowed to run code it downloads. The body is the
/// input a seat would hand the plugin (`plugin_api::pack`, or plain JSON); the
/// answer is the plugin's own. Same sandbox and limits as on a seat, and the
/// signature is checked again first, so a plugin runs here only if every seat
/// would have run it.
async fn run_plugin(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    body: axum::body::Bytes,
) -> Answer<Json<plugin_api::Answer>> {
    let who = caller(&server, &headers)?;
    let digest = server
        .store
        .with(|db| {
            Ok(db
                .query_row("SELECT digest FROM plugins WHERE id = ?1", params![id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that plugin", e))?
        .ok_or_else(|| Denied::missing("plugin"))?;
    let file = server
        .store
        .get_blob(&digest)
        .map_err(|e| Denied::broke("read that plugin", e))?;
    let mut trusted = crate::plugin_trust();
    trusted.keys.extend(server.config.plugin_keys_for_tests.iter().cloned());
    let checked = hub::plugin::check(&trusted, &file).map_err(Denied::wrong)?;
    let input = plugin_api::unpack(&body)
        .map_err(|e| Denied::wrong(format!("That isn't a plugin's input: {e}")))?;
    let command = input.command.clone();
    let answer = tokio::task::spawn_blocking(move || plugin_host::run(&checked.wasm, &input))
        .await
        .map_err(|e| Denied::broke("run that plugin", e))?;
    tracing::info!(
        "ran plugin {id} {command} for {}: {}",
        who.name,
        if answer.is_ok() { "done" } else { "failed" }
    );
    Ok(Json(match answer {
        Ok(output) => plugin_api::Answer::Done(output),
        Err(why) => plugin_api::Answer::Failed(why),
    }))
}

async fn plugin_file(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Answer<Response> {
    caller(&server, &headers)?;
    let digest = server
        .store
        .with(|db| {
            Ok(db
                .query_row("SELECT digest FROM plugins WHERE id = ?1", params![id], |r| {
                    r.get::<_, String>(0)
                })
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that plugin", e))?;
    let digest = digest.ok_or_else(|| Denied::missing("plugin"))?;
    let bytes = server
        .store
        .get_blob(&digest)
        .map_err(|e| Denied::broke("read that plugin", e))?;
    Ok((
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from(bytes),
    )
        .into_response())
}

/// Hands a plugin to the whole office. The body is the `.hvplugin` file.
async fn add_plugin(
    State(server): State<Shared>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Answer<Json<hub::PluginInfo>> {
    let who = administrator(&server, &headers)?;
    sharing(&server)?;
    if body.is_empty() {
        return Err(Denied::wrong("No plugin came with that."));
    }
    let mut trusted = crate::plugin_trust();
    trusted.keys.extend(server.config.plugin_keys_for_tests.iter().cloned());
    let checked = hub::plugin::check(&trusted, &body).map_err(Denied::wrong)?;
    let digest = server
        .store
        .put_blob(&body)
        .map_err(|e| Denied::broke("store that plugin", e))?;
    let manifest = plugin_host::manifest_of(&checked.wasm).ok();
    let plugin = hub::PluginInfo {
        id: checked.header.id.clone(),
        name: if checked.header.name.trim().is_empty() {
            checked.header.id.clone()
        } else {
            checked.header.name.clone()
        },
        version: checked.header.version.clone(),
        digest,
        bytes: body.len() as u64,
        uploaded: now(),
        uploaded_by: who.name.clone(),
        key: checked.header.key.clone(),
        manifest,
    };
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT OR REPLACE INTO plugins (id, name, version, digest, bytes, key, uploaded, uploaded_by)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    plugin.id,
                    plugin.name,
                    plugin.version,
                    plugin.digest,
                    plugin.bytes as i64,
                    plugin.key,
                    plugin.uploaded,
                    plugin.uploaded_by
                ],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("record that plugin", e))?;
    tracing::info!("{} added plugin {} {}", who.name, plugin.id, plugin.version);
    Ok(Json(plugin))
}

async fn remove_plugin(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _sent: Unused,
) -> Answer<Json<serde_json::Value>> {
    let who = administrator(&server, &headers)?;
    let gone = server
        .store
        .with(|db| Ok(db.execute("DELETE FROM plugins WHERE id = ?1", params![id])?))
        .map_err(|e| Denied::broke("remove that plugin", e))?;
    if gone == 0 {
        return Err(Denied::missing("plugin"));
    }
    tracing::info!("{} removed plugin {id}", who.name);
    Ok(Json(serde_json::json!({ "removed": id })))
}

// ---- updates --------------------------------------------------------------

#[derive(Deserialize)]
pub struct Asking {
    /// Look at the publisher's feed before answering. Anything but `true` is
    /// the ordinary question.
    #[serde(default, deserialize_with = "said_yes")]
    pub fresh: bool,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub platform: String,
}

/// `true`, `1` or `yes`, and nothing else, is yes.
fn said_yes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    let said = <String as serde::Deserialize>::deserialize(d)?;
    Ok(matches!(said.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"))
}

async fn update_latest(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(asking): Query<Asking>,
) -> Answer<Json<UpdateOffer>> {
    caller(&server, &headers)?;
    // A seat asking is a reason to look at the publisher's feed, when the last
    // look was a while ago: the office hears about a fix within minutes of
    // somebody opening Hyperview, not at the server's next turn. Somebody who
    // pressed "Check for Updates" is waiting, so that look is waited for;
    // any other asking is answered at once and the look goes on behind it.
    let looking = crate::updates::on_asking(&server, asking.fresh).await;
    // The office's channel and the seat's, whichever is earlier. A shop that
    // has asked to try new versions first gets them on every seat, not only on
    // the seat that happened to be told.
    let channel = match (asking.channel.as_str(), crate::updates::office_channel(&server)) {
        ("preview", _) | (_, Channel::Preview) => Channel::Preview,
        _ => Channel::Stable,
    };
    let platform = if asking.platform.trim().is_empty() {
        hub::feed::ASSUMED_PLATFORM.to_string()
    } else {
        asking.platform.clone()
    };

    // An administrator holding the office on one version is answered first,
    // because it is an instruction rather than a preference. A fabricator
    // halfway through a bid package should not have the tool change under them.
    // A pin written through the API is the administrator's most recent
    // instruction, so it wins over the one in the environment — including when
    // it is an explicit empty one, which means "let them move again". Only an
    // installation where nobody has ever set a pin falls back to the
    // environment.
    let pinned = match server
        .store
        .setting("pin_version")
        .map_err(|e| Denied::broke("read the pinned version", e))?
    {
        Some(explicit) => {
            let explicit = explicit.trim().to_string();
            (!explicit.is_empty()).then_some(explicit)
        }
        None => server.config.pin_version.clone(),
    };

    let wanted = pinned.clone();
    let found = server
        .store
        .with(|db| {
            let sql = "SELECT version, channel, platform, published, notes, bytes, digest,
                              signature, signing_key, minimum_api
                       FROM releases
                       WHERE platform = ?1 AND ready = 1
                         AND (channel = 'stable' OR ?2 = 'preview')";
            let mut statement = db.prepare(sql)?;
            let all = statement
                .query_map(params![platform, channel_name(channel)], |r| {
                    Ok(Release {
                        version: r.get(0)?,
                        channel: match r.get::<_, String>(1)?.as_str() {
                            "preview" => Channel::Preview,
                            _ => Channel::Stable,
                        },
                        platform: r.get(2)?,
                        published: r.get(3)?,
                        notes: r.get(4)?,
                        download: String::new(),
                        bytes: r.get::<_, i64>(5)? as u64,
                        digest: r.get(6)?,
                        signature: r.get(7)?,
                        key: r.get(8)?,
                        minimum_api_version: r.get::<_, i64>(9)? as u32,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(all)
        })
        .map_err(|e| Denied::broke("look for an update", e))?;

    // Newest by version, not by the day somebody published it. Two releases
    // put up the same afternoon still have an order, and 1.10 comes after 1.9.
    let mut found = found;
    found.sort_by(|a, b| hub::update::compare(&b.version, &a.version));

    let chosen = match &wanted {
        // Pinned: exactly that version, or nothing.
        Some(version) => found.into_iter().find(|r| &r.version == version),
        None => found
            .into_iter()
            .find(|r| hub::update::newer(&r.version, &asking.version)),
    };

    let release = chosen.map(|mut r| {
        r.download = format!("{API_ROOT}/update/{}/download?platform={}", r.version, r.platform);
        r
    });
    let required = release
        .as_ref()
        .map(|r| r.minimum_api_version > API_VERSION)
        .unwrap_or(false);

    Ok(Json(UpdateOffer {
        release,
        pinned_to: pinned,
        required,
        looking,
    }))
}

pub fn channel_name(channel: Channel) -> &'static str {
    match channel {
        Channel::Stable => "stable",
        Channel::Preview => "preview",
    }
}

async fn update_download(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(version): Path<String>,
    Query(asking): Query<Asking>,
) -> Answer<Response> {
    caller(&server, &headers)?;
    let platform = if asking.platform.trim().is_empty() {
        hub::feed::ASSUMED_PLATFORM.to_string()
    } else {
        asking.platform
    };
    let digest = server
        .store
        .with(|db| {
            Ok(db
                .query_row(
                    "SELECT digest FROM releases WHERE version = ?1 AND platform = ?2",
                    params![version, platform],
                    |r| r.get::<_, String>(0),
                )
                .optional()?)
        })
        .map_err(|e| Denied::broke("find that release", e))?;
    let digest = digest.ok_or_else(|| Denied::missing("release"))?;
    // The version came from outside, so the file is found by its digest out of
    // the database rather than by building a path from what was asked for.
    if !is_digest(&digest) {
        return Err(Denied::broke("read that release", "its digest is malformed"));
    }
    let bytes = server
        .store
        .get_blob(&digest)
        .map_err(|e| Denied::broke("read that release", e))?;
    Ok((
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"Hyperview-{}-Setup.exe\"", safe_filename(&version)),
            ),
        ],
        Body::from(bytes),
    )
        .into_response())
}

// ---- what an administrator does -------------------------------------------

#[derive(Deserialize)]
pub struct NewPerson {
    pub name: String,
    pub email: String,
    pub password: String,
    /// viewer, estimator or admin. Anything else is an ordinary seat.
    #[serde(default)]
    pub role: String,
}

async fn people(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Vec<User>>> {
    administrator(&server, &headers)?;
    let list = server
        .store
        .with(|db| {
            let mut statement =
                db.prepare("SELECT id, name, email, role FROM people ORDER BY name")?;
            let rows: Vec<User> = statement
                .query_map([], |r| {
                    Ok(User {
                        id: r.get(0)?,
                        name: r.get(1)?,
                        email: r.get(2)?,
                        role: auth::role_from(&r.get::<_, String>(3)?),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .map_err(|e| Denied::broke("list the people", e))?;
    Ok(Json(list))
}

async fn add_person(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewPerson>,
) -> Answer<Json<User>> {
    let who = administrator(&server, &headers)?;
    crate::license::room_for_another(&server).map_err(Denied::license_users)?;
    let email = body.email.trim().to_lowercase();
    if !email.contains('@') {
        return Err(Denied::wrong("That does not look like an email address."));
    }
    // Short passwords are the commonest way a shop server ends up open, and
    // the only moment anybody will listen about it is when one is being set.
    if body.password.chars().count() < 12 {
        return Err(Denied::wrong(
            "Use at least twelve characters. Four unrelated words is easier to type and \
             far harder to guess than a short one with symbols in it.",
        ));
    }
    let hashed = auth::hash_password(&body.password)
        .map_err(|e| Denied::broke("set that password", e))?;
    let user = User {
        id: fresh_id("usr"),
        name: body.name.trim().to_string(),
        email: email.clone(),
        role: auth::role_from(body.role.trim()),
    };
    let created = now();
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    user.id,
                    user.name,
                    user.email,
                    hashed,
                    auth::role_name(user.role),
                    created
                ],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("add that person", e))?;
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::ADDED_PERSON, &who.email)
            .by(&who.id)
            .about(&user.email)
            .saying(auth::role_name(user.role)),
    );
    Ok(Json(user))
}

// ---- people who leave --------------------------------------------------------
//
// A shop that cannot take somebody's access away has no access control, only
// a suggestion. Both of these end the person's sessions the instant they are
// written, because every request reads the role and the account fresh out of
// `people` rather than trusting what was true when they signed in.
//
// One rule guards both: a server must never be left without an administrator.
// There is no way back from that except editing the database by hand, and the
// person it happens to is always somebody who was trying to tidy up.

#[derive(Deserialize)]
struct NewRole {
    /// viewer, estimator or admin.
    role: String,
}

/// How many administrators there would be if `excluding` were not one.
fn other_admins(db: &rusqlite::Connection, excluding: &str) -> rusqlite::Result<i64> {
    db.query_row(
        "SELECT count(*) FROM people WHERE role = 'admin' AND id != ?1",
        params![excluding],
        |r| r.get(0),
    )
}

async fn change_role(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<NewRole>,
) -> Answer<Json<User>> {
    let who = administrator(&server, &headers)?;
    let role = auth::role_from(body.role.trim());

    let changed = server
        .store
        .with(|db| {
            let found: Option<(String, String, String)> = db
                .query_row(
                    "SELECT name, email, role FROM people WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            let Some((name, email, was)) = found else {
                return Ok(None);
            };
            // Taking the last administrator's badge off is how a shop locks
            // itself out of its own server.
            if was == "admin" && !role.may_administer() && other_admins(db, &id)? == 0 {
                return Ok(Some(Err(())));
            }
            db.execute(
                "UPDATE people SET role = ?1 WHERE id = ?2",
                params![auth::role_name(role), id],
            )?;
            Ok(Some(Ok((name, email, was))))
        })
        .map_err(|e| Denied::broke("change what that person may do", e))?;

    let Some(outcome) = changed else {
        return Err(Denied::missing("person"));
    };
    let Ok((name, email, was)) = outcome else {
        return Err(Denied::wrong(
            "That is the only administrator on this server. Make somebody else an \
             administrator first, or nobody will be able to administer it at all.",
        ));
    };

    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::CHANGED_ROLE, &who.email)
            .by(&who.id)
            .about(&email)
            .saying(format!("{was} to {}", auth::role_name(role))),
    );
    Ok(Json(User { id, name, email, role }))
}

/// Takes somebody's account away, and with it every session and key they had.
///
/// What they made stays. Markups carry an author's name and drawing sets carry
/// who uploaded them, as text rather than as a link to this row, so a takeoff
/// does not lose its history because somebody left the company.
async fn remove_person(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    _sent: Unused,
) -> Answer<Json<serde_json::Value>> {
    let who = administrator(&server, &headers)?;
    if id == who.id {
        return Err(Denied::wrong(
            "You cannot remove your own account. Another administrator can.",
        ));
    }

    let removed = server
        .store
        .with(|db| {
            let found: Option<(String, String)> = db
                .query_row(
                    "SELECT email, role FROM people WHERE id = ?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((email, role)) = found else {
                return Ok(None);
            };
            if role == "admin" && other_admins(db, &id)? == 0 {
                return Ok(Some(Err(())));
            }
            // Sessions go with the person by the schema's own cascade; the
            // keys they made are taken back here.
            db.execute("DELETE FROM keys WHERE person = ?1", params![id])?;
            db.execute("DELETE FROM people WHERE id = ?1", params![id])?;
            Ok(Some(Ok(email)))
        })
        .map_err(|e| Denied::broke("remove that person", e))?;

    let Some(outcome) = removed else {
        return Err(Denied::missing("person"));
    };
    let Ok(email) = outcome else {
        return Err(Denied::wrong(
            "That is the only administrator on this server. Make somebody else an \
             administrator first.",
        ));
    };

    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::REMOVED_PERSON, &who.email)
            .by(&who.id)
            .about(&email)
            .saying("their sessions and keys went with them"),
    );
    Ok(Json(serde_json::json!({ "removed": id })))
}

/// Ends this session on the server, rather than only forgetting it here.
///
/// Before this, signing out dropped the connection in the program and left the
/// token working for another thirty days. Anybody who had it — a shared
/// machine, a stolen laptop — still had the shop's drawings.
async fn sign_out(State(server): State<Shared>, headers: HeaderMap, _sent: Unused) -> Answer<Json<serde_json::Value>> {
    let who = caller(&server, &headers)?;
    let Some(token) = bearer(&headers) else {
        return Err(Denied::unauthorised());
    };
    // A key is not a session. Taking one back is `/keys/{id}/revoke`, which
    // says so, rather than something that happens because a program quit.
    if token.starts_with(auth::KEY_PREFIX) {
        return Err(Denied::wrong(
            "That is a key, not a sign-in. Take a key back with /keys/{id}/revoke.",
        ));
    }
    server
        .store
        .with(|db| {
            db.execute(
                "DELETE FROM sessions WHERE token = ?1",
                params![auth::token_hash(&token)],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("sign you out", e))?;
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::SIGNED_OUT, &who.email).by(&who.id),
    );
    Ok(Json(serde_json::json!({ "signed_out": true })))
}

// ---- the license ------------------------------------------------------------

/// Where this office stands with Excalibur View Office. Anybody signed in may
/// ask; an administrator is also told the license id.
async fn read_license(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<hub::license::Standing>> {
    let who = caller(&server, &headers)?;
    Ok(Json(crate::license::standing(&server, who.role.may_administer())))
}

#[derive(Deserialize)]
struct NewLicense {
    /// The whole text of the .evlicense file.
    license: String,
}

/// Adds a license file. Checked here, against the keys built into this
/// server; nothing is sent anywhere to do it.
async fn add_license(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewLicense>,
) -> Answer<Json<hub::license::Standing>> {
    let who = administrator(&server, &headers)?;
    let license = crate::license::install(&server, &body.license).map_err(Denied::wrong)?;
    tracing::info!("license {} for {} added by {}", license.id, license.company, who.email);
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::ADDED_LICENSE, &who.email)
            .by(&who.id)
            .about(license.id.clone())
            .saying(format!("{} for {}", license.edition.title(), license.company)),
    );
    Ok(Json(crate::license::standing(&server, true)))
}



#[derive(Deserialize, Default)]
struct CopyAsked {
    /// What to call it. Empty gets "<the set> — Takeoff".
    #[serde(default)]
    name: String,
}

/// A copy of a drawing set to mark up.
///
/// An estimator does not take off on the set everybody else is reading. They
/// take a copy, mark it to pieces, and leave the issued set clean — so this
/// is one button rather than a re-upload.
///
/// **It costs no bytes.** A set's file is kept by its SHA-256, so the copy
/// points at the same file the original does. What is new is the row, the
/// sheets and, from then on, its own markups.
///
/// The copy supersedes nothing and is superseded by nothing: it is not
/// another issue of the drawing, it is somebody's working copy, and it must
/// never take the issued set's place in the project.
async fn copy_set(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(asked): Json<CopyAsked>,
) -> Answer<Json<DrawingSet>> {
    let who = writer(&server, &headers)?;
    may_reach_set(&server, &who, &id)?;
    let made = server
        .store
        .with(|db| {
            let tx = db.unchecked_transaction()?;
            let db: &Connection = &tx;
            let Some(from) = read_set(db, &id)? else {
                return Ok(None);
            };
            let wanted = asked.name.trim();
            let name = if wanted.is_empty() {
                format!("{} — Takeoff", from.name)
            } else {
                wanted.to_string()
            };
            let copy = DrawingSet {
                id: fresh_id("set"),
                name,
                uploaded: now(),
                uploaded_by: who.name.clone(),
                revision: 0,
                // A working copy is not an issue of the drawing.
                supersedes: None,
                superseded_by: None,
                ..from.clone()
            };
            db.execute(
                "INSERT INTO sets (id, project, name, filename, digest, bytes, sheets,
                                   uploaded, uploaded_by, revision, folder)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 0, ?10)",
                params![
                    copy.id,
                    copy.project,
                    copy.name,
                    copy.filename,
                    copy.digest,
                    copy.bytes as i64,
                    copy.sheets as i64,
                    copy.uploaded,
                    copy.uploaded_by,
                    copy.folder,
                ],
            )?;
            // The sheets come across with their numbers, titles and — the
            // part that matters — the scales somebody already calibrated.
            // Taking off on a copy that has forgotten the scales would be
            // worse than no copy at all.
            db.execute(
                "INSERT INTO sheets (set_id, page, number, title, width, height, rotation, scale)
                 SELECT ?1, page, number, title, width, height, rotation, scale
                 FROM sheets WHERE set_id = ?2",
                params![copy.id, from.id],
            )?;
            tx.commit()?;
            Ok(Some(copy))
        })
        .map_err(|e| Denied::broke("copy that drawing set", e))?;
    let copy = made.ok_or_else(|| Denied::missing("drawing set"))?;
    record(
        &server,
        crate::audit::Entry::new(crate::audit::action::UPLOADED_SET, &who.email)
            .by(&who.id)
            .about(copy.name.clone())
            .saying("a copy to take off on"),
    );
    Ok(Json(copy))
}

// ---- the audit log -------------------------------------------------------

/// Records one thing that happened. Deliberately swallows its own failure: a
/// log that can stop somebody opening a drawing is a log that gets switched
/// off, and a server whose disk has filled should still let the shop work.
/// Where a request came from, for the log's `address` column.
///
/// The column has existed since the log did and was never filled in, which
/// made "signed in" and "sign-in refused" half a record: an administrator
/// looking at a string of refusals wants to know whether they came from one
/// machine or forty.
pub(crate) fn seen_from(headers: &HeaderMap) -> String {
    // Behind a tunnel or a reverse proxy the socket is the proxy, so the
    // forwarded address is the one worth keeping when there is one. First
    // entry only: the rest is whatever the client claimed on the way in.
    for name in ["x-forwarded-for", "x-real-ip"] {
        if let Some(value) = headers.get(name).and_then(|v| v.to_str().ok()) {
            if let Some(first) = value.split(',').next() {
                let first = first.trim();
                if !first.is_empty() {
                    return first.to_string();
                }
            }
        }
    }
    String::new()
}

fn record(server: &Server, entry: crate::audit::Entry) {
    if let Err(e) = server.store.audit(&entry) {
        tracing::warn!("the audit log could not be written: {e}");
    }
}


#[derive(Deserialize)]
struct AuditAsked {
    /// Only what happened at or after this time (RFC 3339).
    since: Option<String>,
    /// At most this many lines, newest first.
    limit: Option<usize>,
}

/// How many lines an unasked-for read hands back, and the most it ever will.
const AUDIT_PAGE: usize = 500;
const AUDIT_MOST: usize = 50_000;

/// The audit log. Administrators only — it says who did what, which is not
/// everybody's to read.
async fn read_audit(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(asked): Query<AuditAsked>,
) -> Answer<Json<Vec<crate::audit::Entry>>> {
    administrator(&server, &headers)?;
    let limit = asked.limit.unwrap_or(AUDIT_PAGE).clamp(1, AUDIT_MOST);
    let entries = server
        .store
        .audit_since(asked.since.as_deref(), limit)
        .map_err(|e| Denied::broke("read the audit log", e))?;
    Ok(Json(entries))
}

/// The same thing as a CSV, which is what an auditor asks to be sent.
async fn audit_csv(
    State(server): State<Shared>,
    headers: HeaderMap,
    Query(asked): Query<AuditAsked>,
) -> Answer<Response> {
    administrator(&server, &headers)?;
    let limit = asked.limit.unwrap_or(AUDIT_MOST).clamp(1, AUDIT_MOST);
    let entries = server
        .store
        .audit_since(asked.since.as_deref(), limit)
        .map_err(|e| Denied::broke("read the audit log", e))?;
    let body = crate::audit::as_csv(&entries);
    Ok((
        [
            (header::CONTENT_TYPE, "text/csv; charset=utf-8"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename=\"excalibur-audit.csv\"",
            ),
        ],
        body,
    )
        .into_response())
}

// ---- the first person in --------------------------------------------------
//
// A server arrives with nothing on it. Somebody has to be the first account,
// and asking a fabricator to run a command to make one is asking them to ring
// somebody who knows how. So the program does it: whoever first finds an
// unclaimed server and fills in four boxes becomes its administrator.
//
// It works exactly once. The moment there is an account here this route says
// so and stops, which is what makes it safe to leave open — a server on a shop
// floor is unclaimed for the ten minutes between being started and being set
// up, and claimed for the rest of its life.

async fn claim(State(server): State<Shared>, Json(body): Json<Setup>) -> Answer<Json<Claimed>> {
    let company = body.company.trim().to_string();
    let name = body.name.trim().to_string();
    let email = body.email.trim().to_lowercase();
    if company.is_empty() {
        return Err(Denied::wrong(
            "Give the server a name — whatever the office calls itself is right. \
             It shows in everybody's title bar so nobody marks up a drawing on the wrong server.",
        ));
    }
    if name.is_empty() {
        return Err(Denied::wrong(
            "Put your own name in. It goes on every markup you make.",
        ));
    }
    check_email(&email)?;
    check_password(&body.password)?;

    let hashed =
        auth::hash_password(&body.password).map_err(|e| Denied::broke("set that password", e))?;
    let id = fresh_id("usr");
    let created = now();

    // Counted and inserted without letting go in between, so two people who
    // both found the same fresh server cannot both become its administrator.
    let claimed = server
        .store
        .with(|db| {
            let people: i64 = db.query_row("SELECT count(*) FROM people", [], |r| r.get(0))?;
            if people > 0 {
                return Ok(false);
            }
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created)
                 VALUES (?1, ?2, ?3, ?4, 'admin', ?5)",
                params![id, name, email, hashed, created],
            )?;
            Ok(true)
        })
        .map_err(|e| Denied::broke("set this server up", e))?;

    if !claimed {
        return Err(Denied(
            StatusCode::CONFLICT,
            Problem::new(
                "already_set_up",
                "This server already belongs to somebody. Sign in, or ask whoever set it \
                 up for an account.",
            ),
        ));
    }

    let code = auth::readable_secret();
    let keep = |key: &str, value: &str| -> Answer<()> {
        server
            .store
            .set_setting(key, value)
            .map_err(|e| Denied::broke("remember how this server is set up", e))
    };
    keep(settings::COMPANY, &company)?;
    keep(settings::JOINING, "code")?;
    keep(settings::JOIN_CODE, &code)?;
    keep(settings::JOIN_ROLE, "estimator")?;

    tracing::info!("{company} set up by {email}");
    let session = issue_session(&server, &id, &name, &email, Role::Admin)?;
    Ok(Json(Claimed {
        session,
        company,
        join_code: code,
    }))
}

// ---- and everybody after --------------------------------------------------

async fn join(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Join>,
) -> Answer<Json<Session>> {
    let (how, code, role) = how_joining(&server);
    let from = seen_from(&headers);

    // Harder than signing in, and on purpose. A join code has no username in
    // front of it: a guess is a guess at the whole secret, and the secret is
    // one string that names nobody, never expires and is not used up. Three
    // tries free and then an hour is the difference between a shared code
    // being a convenience and a shared code being the way in.
    if let Some(owed) = server.patience.wait_for(crate::patience::JOINING, &from) {
        return Err(Denied(
            StatusCode::TOO_MANY_REQUESTS,
            Problem::new("too_many", &crate::patience::refusal(owed)),
        ));
    }

    // An invitation first, whatever the server's own setting is. It names
    // one person, carries the role they get, runs out on a date and dies when
    // it is used -- which is every weakness of the shared code, answered. A
    // server that is otherwise closed still honours one, because somebody
    // deliberately invited this person.
    let mut invited: Option<crate::store::Invite> = None;
    if let Ok(Some(invite)) = server.store.invite(body.code.trim()) {
        // Used or expired is refused exactly like a wrong code. Somebody
        // holding a dead invitation learns nothing about why.
        let dead = invite.used.is_some() || ran_out(&invite.expires, &crate::audit::now());
        if dead {
            server.patience.wrong(&from);
            record(
                &server,
                crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, &invite.email)
                    .saying(if invite.used.is_some() {
                        "that invitation has already been used"
                    } else {
                        "that invitation has run out"
                    })
                    .from(from.clone()),
            );
            return Err(Denied(
                StatusCode::FORBIDDEN,
                Problem::new(
                    "wrong_code",
                    "That code is not right. Ask whoever set the server up for another one.",
                ),
            ));
        }
        server.patience.right(&from);
        invited = Some(invite);
    }

    let role = match invited.as_ref() {
        Some(invite) => invite.role.clone(),
        None => role,
    };

    match how.as_str() {
        _ if invited.is_some() => {}
        "open" => {}
        "code" => {
            // A code with a date that has gone by is not a code. Checked
            // before the code itself and answered the same way, so somebody
            // guessing cannot tell a wrong code from an expired one.
            if join_has_run_out(&server) {
                server.patience.wrong(&from);
                record(
                    &server,
                    crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, "")
                        .saying("the join code has run out")
                        .from(from.clone()),
                );
                return Err(Denied(
                    StatusCode::FORBIDDEN,
                    Problem::new(
                        "wrong_code",
                        "That join code is not right. Ask whoever set the server up \
                         for the current one — it can be changed, so an old one stops working.",
                    ),
                ));
            }
            // The same refusal whether the code is wrong or the server is not
            // taking anybody, so guessing tells nobody anything.
            if !auth::secret_matches(&body.code, &code) {
                server.patience.wrong(&from);
                record(
                    &server,
                    crate::audit::Entry::new(crate::audit::action::SIGN_IN_REFUSED, "")
                        .saying("the join code did not match")
                        .from(from.clone()),
                );
                return Err(Denied(
                    StatusCode::FORBIDDEN,
                    Problem::new(
                        "wrong_code",
                        "That join code is not right. Ask whoever set the server up \
                         for the current one — it can be changed, so an old one stops working.",
                    ),
                ));
            }
            server.patience.right(&from);
        }
        _ => {
            return Err(Denied(
                StatusCode::FORBIDDEN,
                Problem::new(
                    "not_joining",
                    "This server is not taking new accounts. An administrator can add you.",
                ),
            ))
        }
    }

    crate::license::room_for_another(&server).map_err(Denied::license_users)?;
    let name = body.name.trim().to_string();
    let email = body.email.trim().to_lowercase();
    if name.is_empty() {
        return Err(Denied::wrong(
            "Put your own name in. It goes on every markup you make.",
        ));
    }
    check_email(&email)?;
    check_password(&body.password)?;

    let hashed =
        auth::hash_password(&body.password).map_err(|e| Denied::broke("set that password", e))?;
    // An invitation made out to one address is for that address. Otherwise
    // one forwarded email is a spare account for whoever got it.
    if let Some(invite) = invited.as_ref() {
        if !invite.email.trim().is_empty() && invite.email.trim().to_lowercase() != email {
            return Err(Denied::wrong(format!(
                "That invitation is for {}. Ask for one of your own.",
                invite.email
            )));
        }
    }
    let role = auth::role_from(&role);
    let id = fresh_id("usr");
    let created = now();

    let taken = server
        .store
        .with(|db| {
            let already: i64 = db.query_row(
                "SELECT count(*) FROM people WHERE lower(email) = ?1",
                params![email],
                |r| r.get(0),
            )?;
            if already > 0 {
                return Ok(true);
            }
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![id, name, email, hashed, auth::role_name(role), created],
            )?;
            Ok(false)
        })
        .map_err(|e| Denied::broke("make that account", e))?;

    if taken {
        return Err(Denied(
            StatusCode::CONFLICT,
            Problem::new(
                "email_taken",
                "There is already an account on this server with that email address. \
                 Sign in instead, or ask an administrator to reset the password.",
            ),
        ));
    }

    // Used up, and only once. `use_invite` returns false when somebody got
    // there first, which is what makes it single-use even if two people press
    // at the same moment -- and if that happens the account has already been
    // made, so the honest thing is to let them in and say so in the log
    // rather than leave somebody with a password and no account.
    if let Some(invite) = invited.as_ref() {
        match server.store.use_invite(&invite.code, &id) {
            Ok(true) => {}
            Ok(false) => tracing::warn!("{email} used an invitation that was already spent"),
            Err(e) => tracing::error!("could not mark an invitation used: {e}"),
        }
        record(
            &server,
            crate::audit::Entry::new("invitation_used", &email)
                .by(&id)
                .saying(format!("invited as {}", auth::role_name(role))),
        );
    }

    tracing::info!("{email} made an account");
    Ok(Json(issue_session(&server, &id, &name, &email, role)?))
}

// ---- inviting one person --------------------------------------------------

#[derive(Deserialize)]
pub struct NewInvite {
    /// Who it is for. Kept, and checked when it is used, so a forwarded
    /// invitation does not become a spare account for whoever got it.
    pub email: String,
    #[serde(default)]
    pub name: String,
    /// What they will be: `viewer`, `estimator` or `admin`.
    #[serde(default)]
    pub role: String,
    /// How many days it lasts. Thirty when not said.
    #[serde(default)]
    pub days: Option<u32>,
}

/// `POST /admin/invites` — one code, for one person, used once.
///
/// Nothing is emailed and nothing ever will be. Sending mail means an SMTP
/// server, a sending reputation, a bounce nobody sees and a support call when
/// a shop's spam filter eats it. An administrator already has a way to reach
/// somebody they work with; what they need is something to send them.
async fn make_invite(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<NewInvite>,
) -> Answer<Json<hub::Invitation>> {
    let who = administrator(&server, &headers)?;
    let email = body.email.trim().to_lowercase();
    check_email(&email)?;
    let asked = auth::role_from(body.role.trim());
    if asked == Role::Admin {
        return Err(Denied::wrong(
            "Invite them as an ordinary seat and promote them afterwards. An invitation              that makes an administrator is one forwarded email away from making somebody              else one.",
        ));
    }
    let code = auth::readable_secret();
    let days = body.days.filter(|d| *d > 0).unwrap_or(30);
    let expires = in_days(days);
    server
        .store
        .make_invite(
            &code,
            &email,
            body.name.trim(),
            auth::role_name(asked),
            &who.id,
            &expires,
        )
        .map_err(|e| Denied::broke("write that invitation down", e))?;
    record(
        &server,
        crate::audit::Entry::new("invitation_made", &who.email)
            .by(&who.id)
            .about(email.clone())
            .saying(format!("as {}", auth::role_name(asked))),
    );
    Ok(Json(hub::Invitation {
        code,
        email,
        name: body.name.trim().to_string(),
        role: auth::role_name(asked).to_string(),
        expires,
        used: false,
    }))
}

/// `GET /admin/invites` — the ones nobody has used yet.
async fn list_invites(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<Vec<hub::Invitation>>> {
    administrator(&server, &headers)?;
    let now = crate::audit::now();
    let list = server
        .store
        .invites_outstanding()
        .map_err(|e| Denied::broke("list the invitations", e))?
        .into_iter()
        .map(|i| hub::Invitation {
            code: i.code,
            email: i.email,
            name: i.name,
            role: i.role,
            // An expired one reads as used rather than being hidden: an
            // administrator wondering why somebody never joined should be
            // able to see the thing that ran out.
            used: ran_out(&i.expires, &now),
            expires: i.expires,
        })
        .collect();
    Ok(Json(list))
}

/// `DELETE /admin/invites/:code` — for one sent to the wrong person.
async fn drop_invite(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(code): Path<String>,
    _sent: Unused,
) -> Answer<Json<Vec<hub::Invitation>>> {
    let who = administrator(&server, &headers)?;
    server
        .store
        .drop_invite(&code)
        .map_err(|e| Denied::broke("throw that invitation away", e))?;
    record(
        &server,
        crate::audit::Entry::new("invitation_dropped", &who.email).by(&who.id),
    );
    list_invites(State(server), headers).await
}

async fn read_joining(
    State(server): State<Shared>,
    headers: HeaderMap,
) -> Answer<Json<hub::Joining>> {
    administrator(&server, &headers)?;
    let (how, code, role) = how_joining(&server);
    Ok(Json(hub::Joining {
        how,
        code,
        role,
        until: join_runs_out(&server).unwrap_or_default(),
        run_out: join_has_run_out(&server),
    }))
}

#[derive(Deserialize)]
pub struct ChangeJoining {
    /// `code`, `open` or `closed`. Absent leaves it alone.
    #[serde(default)]
    pub how: Option<String>,
    /// What a new account gets. Absent leaves it alone.
    #[serde(default)]
    pub role: Option<String>,
    /// True throws the old code away and makes a new one, which is what
    /// somebody does when a person leaves.
    #[serde(default)]
    pub new_code: bool,
    /// How many days the code should last from now. `Some(0)` means forever,
    /// which is the old behaviour and now has to be asked for. Absent leaves
    /// whatever is set alone.
    #[serde(default)]
    pub days: Option<u32>,
}

async fn change_joining(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<ChangeJoining>,
) -> Answer<Json<hub::Joining>> {
    administrator(&server, &headers)?;
    let (mut how, mut code, mut role) = how_joining(&server);

    if let Some(asked) = body.how.as_deref() {
        how = match asked.trim() {
            "code" => "code".to_string(),
            "open" => "open".to_string(),
            "closed" => "closed".to_string(),
            other => {
                return Err(Denied::wrong(format!(
                    "'{other}' is not a way of letting people in. Use 'code', 'open' or 'closed'."
                )))
            }
        };
    }
    if let Some(asked) = body.role.as_deref() {
        // Round-tripped through the parser so a typo cannot quietly make every
        // new account an administrator.
        role = auth::role_name(auth::role_from(asked.trim())).to_string();
        if auth::role_from(asked.trim()) == Role::Admin {
            return Err(Denied::wrong(
                "New accounts should not make themselves administrators. Add one by hand \
                 instead, or promote somebody after they have joined.",
            ));
        }
    }
    let mut until = join_runs_out(&server).unwrap_or_default();
    if body.new_code || (how == "code" && code.trim().is_empty()) {
        code = auth::readable_secret();
        // A fresh code starts its clock again. Making a new code because
        // somebody left, and having it inherit a date that has already gone
        // by, would be a code that never worked.
        if !until.is_empty() {
            until = in_days(days_of(&body).unwrap_or(30));
        }
    }
    if let Some(days) = body.days {
        until = if days == 0 { String::new() } else { in_days(days) };
    }

    let keep = |key: &str, value: &str| -> Answer<()> {
        server
            .store
            .set_setting(key, value)
            .map_err(|e| Denied::broke("change how people join", e))
    };
    keep(settings::JOINING, &how)?;
    keep(settings::JOIN_CODE, &code)?;
    keep(settings::JOIN_ROLE, &role)?;
    keep(settings::JOIN_UNTIL, &until)?;
    let run_out = !until.is_empty() && ran_out(&until, &crate::audit::now());
    Ok(Json(hub::Joining {
        how,
        code,
        role,
        until,
        run_out,
    }))
}

// ---- the bits all three of those share ------------------------------------

fn check_email(email: &str) -> Answer<()> {
    // Deliberately not a pattern that tries to be right about what an address
    // may contain. The point is to catch somebody typing their name in the
    // wrong box, not to adjudicate RFC 5322.
    if email.len() < 3 || !email.contains('@') || email.starts_with('@') || email.ends_with('@') {
        return Err(Denied::wrong("That does not look like an email address."));
    }
    Ok(())
}

fn check_password(password: &str) -> Answer<()> {
    if password.chars().count() < 12 {
        return Err(Denied::wrong(
            "Use at least twelve characters. Four unrelated words is easier to type and \
             far harder to guess than a short one with symbols in it.",
        ));
    }
    Ok(())
}

/// Starts a session for somebody who has just proved who they are, whether
/// that was by password, by claiming the server or by joining it.
fn issue_session(
    server: &Server,
    id: &str,
    name: &str,
    email: &str,
    role: Role,
) -> Answer<Session> {
    let (token, hashed) = auth::new_token();
    let issued = time::OffsetDateTime::now_utc();
    let expires = issued + time::Duration::days(auth::SESSION_DAYS);
    let format = &time::format_description::well_known::Rfc3339;
    let (issued_text, expires_text) = (
        issued.format(format).unwrap_or_default(),
        expires.format(format).unwrap_or_default(),
    );
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, ?3, ?4)",
                params![hashed, id, issued_text, expires_text],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("start a session", e))?;
    Ok(Session {
        token,
        expires_in: (auth::SESSION_DAYS * 24 * 60 * 60) as u64,
        user: User {
            id: id.to_string(),
            name: name.to_string(),
            email: email.to_string(),
            role,
        },
        api_version: API_VERSION,
    })
}

#[derive(Deserialize)]
pub struct Pin {
    /// Empty or absent lets the office move to whatever is newest again.
    #[serde(default)]
    pub version: String,
}

async fn set_pin(
    State(server): State<Shared>,
    headers: HeaderMap,
    Json(body): Json<Pin>,
) -> Answer<Json<UpdateOffer>> {
    administrator(&server, &headers)?;
    let version = body.version.trim();
    server
        .store
        .set_setting("pin_version", version)
        .map_err(|e| Denied::broke("set the pinned version", e))?;
    Ok(Json(UpdateOffer {
        release: None,
        pinned_to: (!version.is_empty()).then(|| version.to_string()),
        required: false,
        looking: false,
    }))
}

/// Publishes an installer and the signed description of it.
///
/// The server does not sign anything and does not hold a signing key. It is
/// handed a release that was signed elsewhere, and every seat checks that
/// signature for itself against a key compiled into its own binary. That is
/// what makes a server that has been tampered with unable to push anything
/// onto six machines.
async fn upload_release(
    State(server): State<Shared>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Answer<Json<Release>> {
    administrator(&server, &headers)?;
    let mut manifest: Option<Release> = None;
    let mut installer: Vec<u8> = Vec::new();

    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| Denied::wrong(format!("That upload was malformed: {e}")))?
    {
        match field.name().unwrap_or("") {
            "manifest" => {
                let text = field.text().await.unwrap_or_default();
                manifest = Some(
                    serde_json::from_str(&text)
                        .map_err(|e| Denied::wrong(format!("That manifest is not readable: {e}")))?,
                );
            }
            "file" => {
                installer = field
                    .bytes()
                    .await
                    .map_err(|e| Denied::wrong(format!("That file did not arrive whole: {e}")))?
                    .to_vec();
            }
            _ => {}
        }
    }

    let release = manifest.ok_or_else(|| Denied::wrong("That upload had no manifest."))?;
    if installer.is_empty() {
        return Err(Denied::wrong("That upload had no installer."));
    }

    // The server cannot check the signature — it does not decide which keys
    // are trusted, the seats do — but it can check that the file and the
    // manifest describe each other, and refuse a pairing that is already
    // wrong rather than serving it to six machines that will each reject it.
    use sha2::{Digest, Sha256};
    let digest = crate::store::hex(&Sha256::digest(&installer));
    if digest != release.digest.to_lowercase() {
        return Err(Denied::wrong(format!(
            "That installer does not match its manifest: the manifest says {}, the file is {digest}.",
            release.digest
        )));
    }
    if installer.len() as u64 != release.bytes {
        return Err(Denied::wrong(
            "That installer is not the size the manifest says it is.",
        ));
    }
    if release.signature.trim().is_empty() || release.key.trim().is_empty() {
        return Err(Denied::wrong(
            "That release is not signed. An unsigned release would be refused by every seat, \
             so it is refused here.",
        ));
    }

    server
        .store
        .put_blob(&installer)
        .map_err(|e| Denied::broke("store that installer", e))?;

    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO releases (version, channel, platform, published, notes, bytes,
                                       digest, signature, signing_key, minimum_api, ready)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)
                 ON CONFLICT(version, platform) DO UPDATE SET
                     channel = excluded.channel, published = excluded.published,
                     notes = excluded.notes, bytes = excluded.bytes,
                     digest = excluded.digest, signature = excluded.signature,
                     signing_key = excluded.signing_key, minimum_api = excluded.minimum_api",
                params![
                    release.version,
                    channel_name(release.channel),
                    release.platform,
                    release.published,
                    release.notes,
                    release.bytes as i64,
                    release.digest.to_lowercase(),
                    release.signature,
                    release.key,
                    release.minimum_api_version as i64
                ],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("record that release", e))?;

    Ok(Json(release))
}

#[derive(Deserialize)]
pub struct Ready {
    #[serde(default)]
    pub platform: String,
    #[serde(default = "yes")]
    pub ready: bool,
}

fn yes() -> bool {
    true
}

/// Lets the office have it, or takes it back.
///
/// A release sits here unoffered until somebody says so, which is how one seat
/// tries a new version while the other five carry on working.
async fn release_ready(
    State(server): State<Shared>,
    headers: HeaderMap,
    Path(version): Path<String>,
    Json(body): Json<Ready>,
) -> Answer<Json<serde_json::Value>> {
    administrator(&server, &headers)?;
    let platform = if body.platform.trim().is_empty() {
        hub::feed::ASSUMED_PLATFORM.to_string()
    } else {
        body.platform.clone()
    };
    let changed = server
        .store
        .with(|db| {
            Ok(db.execute(
                "UPDATE releases SET ready = ?1 WHERE version = ?2 AND platform = ?3",
                params![body.ready as i64, version, platform],
            )?)
        })
        .map_err(|e| Denied::broke("change that release", e))?;
    if changed == 0 {
        return Err(Denied::missing("release"));
    }
    Ok(Json(serde_json::json!({
        "version": version,
        "platform": platform,
        "ready": body.ready,
    })))
}

/// Uploads a tool chest for the whole office.
async fn upload_chest(
    State(server): State<Shared>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Answer<Json<Chest>> {
    administrator(&server, &headers)?;
    sharing(&server)?;
    let mut name = String::new();
    let mut filename = String::new();
    let mut bytes: Vec<u8> = Vec::new();
    while let Some(field) = form
        .next_field()
        .await
        .map_err(|e| Denied::wrong(format!("That upload was malformed: {e}")))?
    {
        match field.name().unwrap_or("") {
            "name" => name = field.text().await.unwrap_or_default(),
            "file" => {
                filename = field.file_name().unwrap_or("chest.bpx").to_string();
                bytes = field
                    .bytes()
                    .await
                    .map_err(|e| Denied::wrong(format!("That file did not arrive whole: {e}")))?
                    .to_vec();
            }
            _ => {}
        }
    }
    if bytes.is_empty() {
        return Err(Denied::wrong("No file came with that upload."));
    }

    // Read it now, so the office is told how many tools it got rather than
    // finding out when somebody opens the panel.
    let profile = chest::Profile::read(&bytes);
    let (tools, sets) = match &profile {
        Some(p) => (p.sets.iter().map(|s| s.tools.len()).sum(), p.sets.len()),
        None => (0usize, 0usize),
    };
    if tools == 0 {
        return Err(Denied::wrong(
            "No tools could be read out of that file. It should be an Excalibur View tool chest \
             (.evtools), or a Revu profile (.bpx) or tool set (.btx).",
        ));
    }

    let digest = server
        .store
        .put_blob(&bytes)
        .map_err(|e| Denied::broke("store that tool chest", e))?;
    let chest = Chest {
        id: fresh_id("cst"),
        name: if name.trim().is_empty() {
            profile.map(|p| p.name).unwrap_or_else(|| filename.clone())
        } else {
            name.trim().to_string()
        },
        filename,
        digest,
        bytes: bytes.len() as u64,
        tools,
        sets,
        uploaded: now(),
        shared: true,
    };
    server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO chests (id, name, filename, digest, bytes, tools, sets, uploaded, shared)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 1)",
                params![
                    chest.id,
                    chest.name,
                    chest.filename,
                    chest.digest,
                    chest.bytes as i64,
                    chest.tools as i64,
                    chest.sets as i64,
                    chest.uploaded
                ],
            )?;
            Ok(())
        })
        .map_err(|e| Denied::broke("record that tool chest", e))?;
    Ok(Json(chest))
}

// ---- answering an assistant ------------------------------------------------
//
// The MCP endpoint (`crate::mcp`) asks its questions through these. They live
// here because this is where the store is read, and they hand back text rather
// than JSON on purpose: what comes out is read by something that will turn it
// into sentences for a person, and a warning written as a sentence survives
// that journey where a field named `left_out` does not.
//
// Every one of them is read-only. Nothing in this section writes.

/// Who is asking, for the MCP endpoint. Any signed-in person; the answers are
/// exactly what that person would get from the API.
pub fn mcp_caller(server: &Server, headers: &HeaderMap) -> Result<Who, ()> {
    caller_as(
        server,
        headers,
        &[auth::purpose::ASSISTANT, auth::purpose::INTEGRATION],
    )
    .map_err(|_| ())
}

fn broke(what: &str, e: impl std::fmt::Display) -> String {
    format!("Could not {what}: {e}")
}

pub fn mcp_projects(server: &Server) -> Result<String, String> {
    let list: Vec<(String, String, String, i64, Option<String>)> = server
        .store
        .with(|db| {
            let mut statement = db.prepare(
                "SELECT p.id, p.number, p.name,
                        (SELECT count(*) FROM sets WHERE sets.project = p.id AND sets.superseded_by IS NULL),
                        p.archived
                 FROM projects p ORDER BY p.archived IS NOT NULL, p.number",
            )?;
            let rows = statement
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .map_err(|e| broke("read the jobs", e))?;
    if list.is_empty() {
        return Ok("There are no jobs on this server yet.".into());
    }
    let mut out = format!("{} job(s):\n", list.len());
    for (id, number, name, sets, archived) in list {
        let put_away = match archived {
            Some(when) => format!("  (archived {})", when.get(..10).unwrap_or(&when)),
            None => String::new(),
        };
        out.push_str(&format!(
            "  {number}  {name}  —  {sets} current drawing set(s){put_away}   [id {id}]\n"
        ));
    }
    Ok(out)
}

/// The drawing sets, newest first. A set that a newer issue of the same
/// drawing replaced is marked so, because its quantities are not the job's
/// any more; so is takeoff carried forward onto a new issue and not yet
/// checked.
pub fn mcp_sets(server: &Server, project: Option<&str>) -> Result<String, String> {
    type Row = (String, String, String, i64, i64, Option<String>, Option<String>, i64);
    let list: Vec<Row> = server
        .store
        .with(|db| {
            // The job by its id, its number (26-114) or another program's
            // reference to it: whichever the person or the assistant has to hand.
            let mut statement = db.prepare(
                "SELECT s.id, s.name, s.uploaded, s.sheets, s.revision,
                        (SELECT n.name FROM sets n WHERE n.id = s.superseded_by),
                        s.superseded_by,
                        (SELECT count(*) FROM markups m
                          WHERE m.set_id = s.id AND m.removed = 0 AND m.carried_from IS NOT NULL)
                 FROM sets s
                 WHERE (?1 IS NULL OR s.project = ?1
                        OR s.project IN (SELECT id FROM projects WHERE number = ?1 OR reference = ?1))
                 ORDER BY s.superseded_by IS NOT NULL, s.uploaded DESC",
            )?;
            let rows = statement
                .query_map(params![project], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?, r.get(7)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .map_err(|e| broke("read the drawing sets", e))?;
    if list.is_empty() {
        return Ok(match project {
            Some(p) => format!("There are no drawing sets on job {p}."),
            None => "There are no drawing sets on this server yet.".into(),
        });
    }
    let current = list.iter().filter(|r| r.6.is_none()).count();
    let mut out = format!("{} drawing set(s), {current} current, newest first:\n", list.len());
    let mut said_old = false;
    for (id, name, uploaded, sheets, revision, newer_name, newer, carried) in list {
        if newer.is_some() && !said_old {
            out.push_str("Replaced by a newer issue (kept for the record; NOT the job's quantities):\n");
            said_old = true;
        }
        let mut notes = String::new();
        if let (Some(newer), Some(newer_name)) = (&newer, &newer_name) {
            notes.push_str(&format!("  — superseded by {newer_name} [id {newer}]"));
        }
        if carried > 0 {
            notes.push_str(&format!(
                "  — {carried} markup(s) carried forward from the issue before, marked \"{}\" until somebody checks them",
                crate::projects::CARRIED
            ));
        }
        out.push_str(&format!(
            "  {name}  —  {sheets} sheets, revision {revision}, uploaded {uploaded}   [id {id}]{notes}\n"
        ));
    }
    Ok(out)
}

/// The measurements on a set, and the sheets they were measured against.
fn mcp_gather(
    server: &Server,
    id: &str,
) -> Result<(Vec<takeoff::Row>, Vec<quantities::SheetScale>), String> {
    let (_, markups, sheets) =
        gather(server, id).map_err(|_| format!("There is no drawing set called {id} on this server."))?;
    Ok((quantities::rows_of(&markups, &sheets), sheets))
}

/// What is missing from a set of measurements, as a sentence. Empty when
/// nothing is.
fn mcp_short_by(rows: &[takeoff::Row], sheets: &[quantities::SheetScale]) -> String {
    let summary = takeoff::summarise(rows, quantities::WEIGHTS);
    if summary.unscaled == 0 {
        return String::new();
    }
    let named: Vec<String> = summary
        .unscaled_pages
        .iter()
        .map(|page| {
            sheets
                .get(*page)
                .map(|s| s.number.clone())
                .unwrap_or_else(|| format!("sheet {}", page + 1))
        })
        .collect();
    format!(
        "\nSHORT: {} measurement(s) are NOT in these totals, because their sheets have \
         no scale set — {}. They are not counted as zero; they are simply not counted. \
         Any total above is short by whatever is on those sheets, and you must say so.\n",
        summary.unscaled,
        named.join(", ")
    )
}

pub fn mcp_takeoff(server: &Server, id: &str) -> Result<String, String> {
    let (rows, sheets) = mcp_gather(server, id)?;
    if rows.is_empty() {
        return Ok(format!("Nothing has been taken off {id} yet."));
    }
    let summary = takeoff::summarise(&rows, quantities::WEIGHTS);
    let mut out = format!("Takeoff for {id} — {} markup(s).\n\n", summary.picks);
    for group in &summary.groups {
        out.push_str(&format!("  {}  ({})\n", group.subject, group.kind.name()));
        out.push_str(&format!("      {} markup(s)", group.picks));
        if group.length > 0.0 {
            out.push_str(&format!(", {:.2} ft", group.length));
        }
        if group.area > 0.0 {
            out.push_str(&format!(", {:.2} sq ft", group.area));
        }
        if group.count > 0.0 && group.kind == annot::Kind::Count {
            out.push_str(&format!(", count {:.0}", group.count));
        }
        // Absent, never zero. The distinction is the whole argument.
        match group.weighed {
            true => out.push_str(&format!(", {:.0} lb", group.pounds)),
            false => out.push_str(", NO unit weight in the tool chest for this shape — \
                                   it has no weight, which is not a weight of zero"),
        }
        if group.unscaled > 0 {
            out.push_str(&format!(
                " — {} of them left out for want of a scale",
                group.unscaled
            ));
        }
        out.push('\n');
    }
    match summary.pounds > 0.0 {
        true => out.push_str(&format!(
            "\nTOTAL: {:.0} lb · {:.3} tons\n",
            summary.pounds,
            summary.tons()
        )),
        false => out.push_str(
            "\nTOTAL: no weight — nothing on this takeoff carries a unit weight.\n",
        ),
    }
    out.push_str(&mcp_short_by(&rows, &sheets));
    Ok(out)
}

pub fn mcp_shop_list(
    server: &Server,
    id: &str,
    stock_feet: Option<f64>,
) -> Result<String, String> {
    let (rows, sheets) = mcp_gather(server, id)?;
    let list = takeoff::shoplist::build(
        &rows,
        quantities::WEIGHTS,
        takeoff::shoplist::INCH,
    );
    if list.shapes.is_empty() {
        return Ok(format!(
            "There are no length measurements on {id}, so there is no cut list. \
             Areas and counts are on the takeoff instead."
        ));
    }
    let mut out = format!("Cut list for {id} — lengths in feet, every one rounded UP.\n\n");
    for shape in &list.shapes {
        out.push_str(&format!("  {}\n", shape.name));
        for cut in &shape.cuts {
            out.push_str(&format!(
                "      {:.3} ft  × {}",
                cut.length, cut.count
            ));
            if !cut.places.is_empty() {
                out.push_str(&format!("   at {}", cut.places.join(", ")));
            }
            out.push('\n');
        }
        out.push_str(&format!(
            "      {} piece(s), {:.2} ft, {}\n",
            shape.pieces,
            shape.total_length,
            match shape.pounds {
                Some(lb) => format!("{lb:.0} lb"),
                None => "NO unit weight in the tool chest — no weight, not a weight of zero"
                    .to_string(),
            }
        ));
    }
    out.push_str(&format!(
        "\n{} piece(s) altogether · {}\n",
        list.pieces,
        list.weight_says()
    ));

    match stock_feet {
        Some(stock) if stock > 0.0 => {
            out.push_str(&format!("\nWHAT TO BUY, in {stock} ft stock:\n"));
            for nest in takeoff::shoplist::nest_all(&list, stock, takeoff::shoplist::INCH / 8.0)
            {
                out.push_str(&format!(
                    "  {}: {} stick(s), {:.0}% of the steel ends up in the building\n",
                    nest.shape,
                    nest.sticks_to_buy(),
                    nest.yield_of() * 100.0
                ));
                if !nest.is_whole() {
                    out.push_str(&format!(
                        "      {} piece(s) WILL NOT come out of {stock} ft stock and are on \
                         no stick: {}\n",
                        nest.too_long.len(),
                        nest.too_long
                            .iter()
                            .map(|p| format!("{p:.2} ft"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
        }
        _ => out.push_str(
            "\nNothing has been nested, because no stock length was given. Do not pick \
             one — twenty, forty and sixty feet are all ordinary, they differ by shape \
             and by supplier, and a guess here orders the wrong steel. Ask whoever wants \
             this what length they buy.\n",
        ),
    }

    let missing = list.what_is_missing();
    if !missing.is_empty() {
        out.push_str(&format!("\n{missing}\n"));
    }
    out.push_str(&mcp_short_by(&rows, &sheets));
    Ok(out)
}

pub fn mcp_doubles(server: &Server, id: &str) -> Result<String, String> {
    let (rows, sheets) = mcp_gather(server, id)?;
    let found = takeoff::doubles::find(&rows, takeoff::doubles::HowClose::default());
    if !found.found_anything() {
        return Ok(format!(
            "Nothing on {id} looks counted twice. That is not a guarantee: two markups \
             on different sheets for the same member look like an ordinary takeoff from \
             here, and always will."
        ));
    }
    let name_of = |page: usize| {
        sheets
            .get(page)
            .map(|s| s.number.clone())
            .unwrap_or_else(|| format!("sheet {}", page + 1))
    };
    let mut out = format!(
        "{} pair(s) on {id} sit in the same place measuring the same thing. \
         NOTHING HAS BEEN CHANGED — this is a list, not a fix, and no total has been \
         adjusted.\n\n",
        found.pairs.len()
    );
    for pair in &found.pairs {
        out.push_str(&format!(
            "  {}  {}  ({})\n      {}\n",
            name_of(pair.page),
            pair.subject,
            pair.sure.name(),
            pair.why
        ));
    }
    let worth = found.what_it_is_worth(quantities::WEIGHTS, &rows);
    if !worth.is_empty() {
        out.push_str(&format!("\n{worth}\n"));
    }
    out.push_str(
        "\nWhether any of these is really a double is for the estimator to decide. A \
         doorway measured once for the frame and once for the opening is an ordinary \
         takeoff. Do not recommend deleting anything.\n",
    );
    Ok(out)
}

pub fn mcp_revision(server: &Server, was_bid: &str, arrived: &str) -> Result<String, String> {
    let (before, _) = mcp_gather(server, was_bid)?;
    let (after, _) = mcp_gather(server, arrived)?;
    let revision = takeoff::revision::compare(&before, &after, quantities::WEIGHTS);
    let mut out = format!(
        "{was_bid} → {arrived}\n\n{}\n\n",
        revision.headline()
    );
    for line in revision.changes() {
        out.push_str(&format!(
            "  {}  ({})  {} → {} {}  [{}]\n",
            line.subject,
            line.what.name(),
            format_args!("{:.2}", line.quantity.0),
            format_args!("{:.2}", line.quantity.1),
            line.unit,
            match line.pounds_moved() {
                Some(lb) => format!("{lb:+.0} lb"),
                None => "no unit weight on one side or the other, so no weight change \
                         can be worked out"
                    .to_string(),
            }
        ));
    }
    let missing = revision.what_is_missing();
    if !missing.is_empty() {
        out.push_str(&format!("\n{missing}\n"));
    }
    Ok(out)
}

pub fn mcp_sheets(server: &Server, id: &str) -> Result<String, String> {
    let (_, _, sheets) =
        gather(server, id).map_err(|_| format!("There is no drawing set called {id} on this server."))?;
    if sheets.is_empty() {
        return Ok(format!("{id} has no sheets recorded."));
    }
    let without: Vec<&str> = sheets
        .iter()
        .filter(|s| s.scale.is_none())
        .map(|s| s.number.as_str())
        .collect();
    let mut out = format!("{} sheet(s) in {id}:\n", sheets.len());
    for sheet in &sheets {
        out.push_str(&format!(
            "  {}  —  {}\n",
            sheet.number,
            match &sheet.scale {
                Some(scale) => scale.ratio.clone(),
                None => "NO SCALE SET".to_string(),
            }
        ));
    }
    if !without.is_empty() {
        out.push_str(&format!(
            "\n{} sheet(s) have no scale: {}. Any measurement on those is left out of \
             every total — never counted as zero. A takeoff of this set is short by \
             whatever is on them.\n",
            without.len(),
            without.join(", ")
        ));
    }
    Ok(out)
}


#[cfg(test)]
mod a_join_code_that_runs_out {
    //! One server-wide secret that names nobody and is never used up was
    //! defensible while the only way to type it was to be standing in the
    //! building. With a public address it is the weakest thing about the
    //! server: one code shared in one group chat in 2026 still works in 2028.

    use super::ran_out;

    #[test]
    fn no_date_means_it_never_runs_out() {
        // Every server that already exists is in this state, and none of
        // them may be broken by adding the feature.
        assert!(!ran_out("", "2026-09-24T00:00:00Z"));
        assert!(!ran_out("   ", "2099-01-01T00:00:00Z"));
    }

    #[test]
    fn a_date_in_the_future_is_still_good() {
        assert!(!ran_out("2026-12-01T00:00:00Z", "2026-09-24T12:00:00Z"));
    }

    #[test]
    fn a_date_that_has_gone_by_is_not() {
        assert!(ran_out("2026-09-01T00:00:00Z", "2026-09-24T12:00:00Z"));
    }

    #[test]
    fn the_same_instant_is_still_good() {
        // A code that expires "in thirty days" should work for the whole of
        // the thirtieth day rather than stopping at an arbitrary moment
        // somebody cannot predict.
        assert!(!ran_out("2026-09-24T12:00:00Z", "2026-09-24T12:00:00Z"));
    }

    #[test]
    fn something_that_is_not_a_date_reads_as_run_out() {
        // The safe way round. A setting nobody can parse must not become
        // "forever" on a secret that lets people in.
        assert!(ran_out("soon", "2026-09-24T00:00:00Z"));
        assert!(ran_out("2026", "2026-09-24T00:00:00Z"));
    }
}
