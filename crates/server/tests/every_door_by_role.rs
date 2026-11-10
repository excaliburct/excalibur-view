//! Every door of the server, knocked on by everybody who could knock.
//!
//! The authorization probe of the security baseline, kept as a test so it
//! runs on every change. Each route the router serves is called as nobody,
//! as a viewer, an estimator and an administrator signed in with a password,
//! as a key for another program (made by an administrator, and one whose
//! maker is now only an estimator), as a sync key, as an assistant key and as
//! a key that has run out. Each answer is sorted into "let in" or "turned
//! away" (401 or 403) and compared with the table below, which says who each
//! door is for. A route the table does not name fails the test, so a new
//! route has to be given its door on purpose. Any answer of 500 or more fails
//! it too: a door that falls over is not a door that held.

use std::collections::BTreeSet; // WHAT: an ordered set of strings. WHY: the routes the table covers, compared with the router's list.
use std::net::SocketAddr; // WHAT: a socket address. WHY: the server is served with each caller's address, as it is in an office.
use std::sync::Arc; // WHAT: shared ownership. WHY: the router takes the server as an Arc.

use hyperview_server::api::{self, Server}; // WHAT: the router and its state. WHY: the real doors, not a copy of them.
use hyperview_server::{auth, Config, Store}; // WHAT: hashing, keys, configuration and storage. WHY: the office is set up directly, which is quick.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: people, sessions and keys are written straight in.
use serde_json::{json, Value}; // WHAT: JSON. WHY: request bodies and answers.

const PASSWORD: &str = "brace-gusset-purlin-shim-42"; // WHAT: everybody's password. WHY: twelve characters and more, as the server asks.
const MAINTENANCE: &str = "probe-maintenance-key-0123456789abcdef"; // WHAT: the maintenance key this office is given. WHY: Fleet's doors are probed with and without it.

/// Who knocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Knock {
    Nobody,         // WHAT: no token at all. WHY: the internet, or a stranger on the shop network.
    Viewer,         // WHAT: a viewer signed in. WHY: the shop floor's account.
    Estimator,      // WHAT: an estimator signed in. WHY: the office's ordinary account.
    Admin,          // WHAT: an administrator signed in. WHY: the one who runs the server.
    Integration, // WHAT: an administrator's key for another program. WHY: never carries the administrator's power.
    DemotedProgram, // WHAT: a program key whose maker is now an estimator. WHY: acts with its maker's role as it is now.
    ViewerProgram, // WHAT: a program key whose maker is now a viewer. WHY: reads, and changes nothing, as its maker may now.
    Sync,          // WHAT: a sync key. WHY: its own short list of routes and nothing else.
    Assistant,     // WHAT: an assistant key. WHY: the assistant's door only, never the API.
    Expired,       // WHAT: a program key past its end. WHY: answered exactly as no key.
}

const EVERYBODY: [Knock; 10] = [
    Knock::Nobody,
    Knock::Viewer,
    Knock::Estimator,
    Knock::Admin,
    Knock::Integration,
    Knock::DemotedProgram,
    Knock::ViewerProgram,
    Knock::Sync,
    Knock::Assistant,
    Knock::Expired,
]; // WHAT: every caller, in a fixed order. WHY: each route is asked by each of them.

/// Who a door is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Door {
    Public,  // WHAT: open to anybody. WHY: finding the server, signing in, joining.
    Signed,  // WHAT: anybody signed in, or a program's key. WHY: reading what the office holds.
    Writer, // WHAT: estimators and administrators, or a program acting as one. WHY: changing drawings.
    Person, // WHAT: a person signed in with a password, any role. WHY: their own keys and password.
    Admin,  // WHAT: an administrator signed in with a password. WHY: running the server.
    Folders, // WHAT: an administrator signed in, or a sync key. WHY: deciding who sees a folder.
}

/// One route: its method, its path as the router names it, its door, and
/// whether a sync key may use it.
struct Route {
    method: &'static str, // WHAT: GET, POST or DELETE. WHY: a route is its method and path together.
    path: &'static str, // WHAT: the path under /api/v1 with `{id}` where an id goes. WHY: matched with `api::ROUTES`.
    door: Door,         // WHAT: who it is for. WHY: the expectation.
    sync: bool,         // WHAT: on the sync list. WHY: the only routes a sync key reaches.
}

/// Every route under /api/v1 and who it is for. The same list as
/// `api::SYNC_ROUTES` for sync keys, checked below.
const TABLE: &[Route] = &[
    Route {
        method: "GET",
        path: "/health",
        door: Door::Public,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/openapi.json",
        door: Door::Public,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/auth/token",
        door: Door::Public,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/setup",
        door: Door::Public,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/join",
        door: Door::Public,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/admin/joining",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/joining",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/admin/invites",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/invites",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "DELETE",
        path: "/admin/invites/{code}",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/me",
        door: Door::Signed,
        sync: true,
    },
    Route {
        method: "POST",
        path: "/me/password",
        door: Door::Person,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/signout",
        door: Door::Person,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/me/keys",
        door: Door::Person,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/me/keys",
        door: Door::Person,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/keys/{id}/revoke",
        door: Door::Person,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/markuplist",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/markuplist",
        door: Door::Signed,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/admin/updates",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/updates",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/admin/fleet",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/fleet",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/admin/remote",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/remote",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "DELETE",
        path: "/admin/remote",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/projects",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/projects",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/projects/{id}",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/people",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/projects/{id}/people",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/projects/{id}/archive",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "POST",
        path: "/projects/{id}/place",
        door: Door::Writer,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/stamp",
        door: Door::Signed,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/sets",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/sets",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/sets/{id}",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/file",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/sheets",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/markups",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/sets/{id}/markups",
        door: Door::Writer,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/sets/{id}/copy",
        door: Door::Writer,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/takeoff",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/takeoff.csv",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/people",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/people",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/people/{id}/role",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/people/{id}/remove",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/chests",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/chests",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/chests/{id}/file",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/plugins",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/plugins",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/plugins/{id}/file",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/plugins/{id}/run",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/plugins/{id}/remove",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/notices",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/notices",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/notices/{id}/test",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/notices/{id}/remove",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/license",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/license",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/cad/drawings",
        door: Door::Signed,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "POST",
        path: "/cad/drawings",
        door: Door::Writer,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "GET",
        path: "/cad/drawings/{id}",
        door: Door::Signed,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "GET",
        path: "/cad/drawings/{id}/file",
        door: Door::Signed,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "PUT",
        path: "/cad/drawings/{id}/file",
        door: Door::Writer,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "POST",
        path: "/cad/drawings/{id}/lock",
        door: Door::Writer,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "DELETE",
        path: "/cad/drawings/{id}/lock",
        door: Door::Writer,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "GET",
        path: "/cad/drawings/{id}/versions",
        door: Door::Signed,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "POST",
        path: "/cad/drawings/{id}/restore",
        door: Door::Writer,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "POST",
        path: "/cad/drawings/{id}/archive",
        door: Door::Admin,
        sync: false,
    }, // WHAT: CAD's office drawings. WHY: the cad part, switched on by this office's licence
    Route {
        method: "POST",
        path: "/admin/citadel/forget-pin",
        door: Door::Admin,
        sync: false,
    }, // WHAT: Citadel's pin forgotten. WHY: an administrator signed in decides which certificate is believed
    Route {
        method: "GET",
        path: "/office",
        door: Door::Signed,
        sync: false,
    }, // WHAT: the Office Server's parts. WHY: anybody signed in, like the licence's standing
    Route {
        method: "GET",
        path: "/audit",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/audit.csv",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/pin",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/releases",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/admin/releases/{version}/ready",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/update/latest",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/update/{version}/download",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/groups",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/groups",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/groups/{id}/people",
        door: Door::Admin,
        sync: false,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/folders",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/projects/{id}/folders",
        door: Door::Folders,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/projects/{id}/files",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/files",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/files/{id}/file",
        door: Door::Signed,
        sync: false,
    },
    Route {
        method: "POST",
        path: "/files/{id}/folder",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "POST",
        path: "/sets/{id}/folder",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/sets/{id}/signoffs",
        door: Door::Signed,
        sync: true,
    },
    Route {
        method: "POST",
        path: "/sets/{id}/signoffs",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/signoffs",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "POST",
        path: "/signoffs/{id}/sign",
        door: Door::Person,
        sync: false,
    }, // WHAT: a person, any role, never a key. WHY: only a person signs; the shop's viewers sign.
    Route {
        method: "POST",
        path: "/signoffs/{id}/withdraw",
        door: Door::Writer,
        sync: true,
    },
    Route {
        method: "GET",
        path: "/signoffs/{id}/signature.png",
        door: Door::Signed,
        sync: true,
    },
];

/// Whether `knock` should get through `route`'s door.
fn let_in(route: &Route, knock: Knock) -> bool {
    if knock == Knock::Sync {
        return route.sync; // WHAT: a sync key reaches its list and nothing else, public routes included. WHY: the list is checked before any route runs.
    }
    match route.door {
        Door::Public => true, // WHAT: anybody. WHY: a token is not needed, and one that is no good is not looked at.
        Door::Signed => matches!(
            knock,
            Knock::Viewer
                | Knock::Estimator
                | Knock::Admin
                | Knock::Integration
                | Knock::DemotedProgram
                | Knock::ViewerProgram
        ), // WHAT: a person or a program's key. WHY: reading; an assistant key is for /mcp only, and a key run out is nobody.
        Door::Writer => matches!(
            knock,
            Knock::Estimator | Knock::Admin | Knock::Integration | Knock::DemotedProgram
        ), // WHAT: anybody who may change drawings. WHY: a viewer reads only.
        Door::Person => matches!(knock, Knock::Viewer | Knock::Estimator | Knock::Admin), // WHAT: a person signed in. WHY: a key can't make keys, change a password or sign out.
        Door::Admin => knock == Knock::Admin, // WHAT: an administrator with a password. WHY: no key runs the server.
        Door::Folders => knock == Knock::Admin, // WHAT: an administrator; the sync key is let in by the list above. WHY: who sees a folder is theirs to say.
    }
}

/// A folder that is gone when the test is.
struct Scratch(std::path::PathBuf); // WHAT: the office's data folder. WHY: its own, so no other test sees it.

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random(); // WHAT: a random number. WHY: a name no other test uses.
        let path = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("hyperview-doors-{n:016x}")); // WHAT: the folder, in the build's own temporary folder. WHY: never one another account shares.
        std::fs::create_dir_all(&path).expect("a scratch directory"); // WHAT: made. WHY: the store writes into it.
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0); // WHAT: removed. WHY: tests leave nothing behind.
    }
}

/// The office under test.
struct Office {
    base: String,        // WHAT: http://127.0.0.1:port. WHY: every request goes there.
    server: Arc<Server>, // WHAT: the server's state. WHY: sessions and keys are written straight into its store.
    _scratch: Scratch,   // WHAT: its folder. WHY: kept while it runs.
    _runtime: tokio::runtime::Runtime, // WHAT: the runtime serving it. WHY: dropped, the server stops.
}

const LICENSE_KEY: &str = "probe-2026"; // WHAT: the test licence key's name. WHY: named in the licence it signs

/// The test's own licence signing key, from a made-up seed.
fn licensing() -> ed25519_dalek::SigningKey {
    ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]) // WHAT: never a real key. WHY: only this test's server trusts it
}

/// A licence switching the cad part on, signed as the publishing script signs one.
fn a_license_with_cad() -> String {
    use ed25519_dalek::Signer;
    let mut license = hub::license::License {
        id: "evl_probe".into(),
        company: "Probe Steel".into(),
        edition: hub::license::Edition::Office,
        users: 0,
        updates_through: "2099-12-31".into(),
        issued: "2026-11-10".into(),
        note: String::new(),
        modules: vec!["cad".into()],
        key: LICENSE_KEY.into(),
        signature: String::new(),
    }; // WHAT: an office licence with CAD. WHY: the probe's office runs it
    license.signature = hub::update::hex(&licensing().sign(&license.signing_payload()).to_bytes()); // WHAT: signed. WHY: unsigned, it switches nothing on
    serde_json::to_string(&license).unwrap()
}

const ADMIN: &str = "usr_admin"; // WHAT: the administrator's id. WHY: keys and sessions are made for them.
const ESTIMATOR: &str = "usr_estimator"; // WHAT: the estimator's id. WHY: likewise.
const VIEWER: &str = "usr_viewer"; // WHAT: the viewer's id. WHY: likewise.

fn an_office() -> Office {
    let scratch = Scratch::new(); // WHAT: a fresh folder. WHY: a fresh office.
    let config = Config {
        name: "Probe Steel".into(), // WHAT: the office's name. WHY: any will do.
        data: scratch.0.clone(),    // WHAT: its folder. WHY: the scratch one.
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port here. WHY: tests run side by side.
        maintenance_key: Some(MAINTENANCE.into()), // WHAT: Fleet's doors are open to this key. WHY: so they can be probed.
        license_keys_for_tests: vec![(LICENSE_KEY.into(), licensing().verifying_key().to_bytes())], // WHAT: this test's own licence key. WHY: the cad part is switched on by a licence it really signs
        ..Config::default() // WHAT: everything else as shipped. WHY: the server an office runs.
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store"); // WHAT: the database. WHY: the people go in it.
    let hashed = auth::hash_password(PASSWORD).unwrap(); // WHAT: the password hashed once. WHY: Argon2 is slow on purpose.
    store
        .with(|db| {
            for (id, name, email, role) in [
                (ADMIN, "Sam", "sam@example.com", "admin"),
                (ESTIMATOR, "Eve", "eve@example.com", "estimator"),
                (VIEWER, "Val", "val@example.com", "viewer"),
            ] {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![id, name, email, hashed, role, "2026-09-01T00:00:00Z"],
                )?; // WHAT: one person, from before licensing began. WHY: a founding office, so sharing is never paused.
            }
            Ok(())
        })
        .expect("three people"); // WHAT: all in. WHY: every knock is one of them.
    let server = Arc::new(Server::new(store, config.clone())); // WHAT: the server's state. WHY: kept, to write sessions and keys into.
    hyperview_server::license::install(&server, &a_license_with_cad()).expect("the licence with CAD"); // WHAT: CAD switched on. WHY: its doors are probed like every other
    let mut cad = hyperview_server::cad::Store::open(&config.database(), &config.blobs()).expect("CAD's drawings"); // WHAT: CAD's store. WHY: a real drawing to knock on
    assert_eq!(cad.create("Probe", "Plate.excad", b"v1", "sam@example.com"), Ok(1)); // WHAT: drawing 1. WHY: the id the CAD routes are asked with
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime"); // WHAT: a small runtime. WHY: enough for one test.
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .expect("a socket"); // WHAT: the listening socket. WHY: its port is the address.
    let address = listener.local_addr().expect("an address"); // WHAT: the port it got. WHY: the base address.
    let app = api::router(server.clone()); // WHAT: the real router. WHY: the doors under test.
    runtime.spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await; // WHAT: served with each caller's address. WHY: exactly as `run::serve` does in an office.
    });
    Office {
        base: format!("http://{address}"),
        server,
        _scratch: scratch,
        _runtime: runtime,
    }
}

/// A new session for `person`, written straight into the store.
fn session(office: &Office, person: &str) -> String {
    let (token, hashed) = auth::new_token(); // WHAT: a token and what is kept of it. WHY: the same as signing in makes.
    office
        .server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO sessions (token, person, issued, expires) VALUES (?1, ?2, ?3, ?4)",
                params![
                    hashed,
                    person,
                    "2026-01-01T00:00:00Z",
                    "2999-01-01T00:00:00Z"
                ],
            )?;
            Ok(())
        })
        .expect("a session"); // WHAT: a session that lasts. WHY: a sign-in per knock would be Argon2 hundreds of times.
    token
}

/// A new key for `person` of `purpose`, ending at `expires` if given.
fn key(office: &Office, person: &str, purpose: &str, expires: Option<&str>) -> (String, String) {
    let (key, digest) = auth::fresh_key(); // WHAT: a key and its digest. WHY: as making one through the API does.
    let id = format!("key_probe_{}", rand::random::<u32>()); // WHAT: its id. WHY: a revoke needs one.
    office
        .server
        .store
        .with(|db| {
            db.execute(
                "INSERT INTO keys (id, digest, person, purpose, name, made, expires) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![id, digest, person, purpose, id, "2026-01-01T00:00:00Z", expires],
            )?;
            Ok(())
        })
        .expect("a key"); // WHAT: written. WHY: used as a bearer.
    (key, id)
}

/// The token `knock` carries, fresh for this one request.
fn token_for(office: &Office, knock: Knock) -> Option<String> {
    match knock {
        Knock::Nobody => None, // WHAT: no token. WHY: a stranger.
        Knock::Viewer => Some(session(office, VIEWER)),
        Knock::Estimator => Some(session(office, ESTIMATOR)),
        Knock::Admin => Some(session(office, ADMIN)),
        Knock::Integration => Some(key(office, ADMIN, "integration", None).0),
        Knock::DemotedProgram => Some(key(office, ESTIMATOR, "integration", None).0), // WHAT: an integration key standing for one whose maker was made an estimator. WHY: it must act as an estimator now.
        Knock::ViewerProgram => Some(key(office, VIEWER, "integration", None).0), // WHAT: an integration key whose maker was made a viewer. WHY: it must read and change nothing, as a viewer does.
        Knock::Sync => Some(key(office, ADMIN, "sync", None).0),
        Knock::Assistant => Some(key(office, ADMIN, "assistant", None).0),
        Knock::Expired => Some(key(office, ADMIN, "integration", Some("2020-01-01T00:00:00Z")).0),
    }
}

/// A request's answer: its status, and its body as text.
fn send(
    office: &Office,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: &Body,
) -> (u16, String) {
    let url = format!("{}{path}", office.base); // WHAT: the whole address. WHY: paths are absolute here.
    let mut request = ureq::request(method, &url); // WHAT: the request. WHY: any method.
    if let Some(token) = token {
        request = request.set("Authorization", &format!("Bearer {token}")); // WHAT: the bearer. WHY: who is knocking.
    }
    let answer = match body {
        Body::None => request.call(), // WHAT: nothing sent. WHY: reads.
        Body::Json(value) => request.send_json(value.clone()), // WHAT: JSON. WHY: most writes.
        Body::Raw(kind, bytes) => request.set("Content-Type", kind).send_bytes(bytes), // WHAT: bytes of a kind. WHY: uploads and plugins.
    };
    match answer {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(e) => panic!("{method} {path}: nothing answered: {e}"), // WHAT: no answer at all. WHY: a server that dropped the connection has failed the probe.
    }
}

/// What a request carries.
#[derive(Clone)]
enum Body {
    None,                 // WHAT: nothing. WHY: reads.
    Json(Value),          // WHAT: a JSON body. WHY: most writes.
    Raw(String, Vec<u8>), // WHAT: a content type and bytes. WHY: multipart uploads and plugin files.
}

/// A multipart form with the fields given and, if any, one file.
fn form(fields: &[(&str, &str)], file: Option<(&str, &[u8])>) -> Body {
    let boundary = "----probe"; // WHAT: the boundary. WHY: any string the parts don't hold.
    let mut out = Vec::new(); // WHAT: the body. WHY: built part by part.
    for (name, value) in fields {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    if let Some((name, bytes)) = file {
        out.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
        out.extend_from_slice(bytes);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes()); // WHAT: the closing boundary. WHY: a whole form even with no parts.
    Body::Raw(format!("multipart/form-data; boundary={boundary}"), out)
}

/// A small two-sheet drawing.
fn a_drawing() -> Vec<u8> {
    let mut out = b"%PDF-1.7\n".to_vec(); // WHAT: the header. WHY: what a PDF starts with.
    let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: for the cross-reference.
    for body in [
        "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n",
        "2 0 obj\n<</Type/Pages/Kids[3 0 R]/Count 1>>\nendobj\n",
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    ] {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    }
    let xref = out.len(); // WHAT: where the table starts. WHY: written at the end.
    out.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes(),
    );
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!(
            "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n",
            offsets.len() + 1
        )
        .as_bytes(),
    );
    out
}

/// The things the routes are asked about, made once.
struct Things {
    project: String, // WHAT: a program's project nobody is named on. WHY: everybody may see it, a sync key included.
    set: String,     // WHAT: a drawing set in it. WHY: the set routes.
    file: String,    // WHAT: a file in it. WHY: the file routes.
    group: String,   // WHAT: a group. WHY: the group route.
    join: String,    // WHAT: the join code. WHY: joining is asked with the right one.
}

fn things(office: &Office) -> Things {
    let admin = session(office, ADMIN); // WHAT: an administrator. WHY: makes everything.
    let (_, made) = send(
        office,
        "POST",
        "/api/v1/projects",
        Some(&admin),
        &Body::Json(json!({"number": "P-1", "name": "Probe", "reference": "probe:job:1"})),
    );
    let project = serde_json::from_str::<Value>(&made).unwrap()["id"]
        .as_str()
        .expect("a project")
        .to_string(); // WHAT: its id. WHY: used in paths.
    let (_, made) = send(
        office,
        "POST",
        "/api/v1/sets",
        Some(&admin),
        &form(&[("project", &project)], Some(("S-101.pdf", &a_drawing()))),
    );
    let set = serde_json::from_str::<Value>(&made).unwrap()["id"]
        .as_str()
        .expect("a set")
        .to_string(); // WHAT: its id. WHY: used in paths.
    let (_, made) = send(
        office,
        "POST",
        "/api/v1/files",
        Some(&admin),
        &form(&[("project", &project)], Some(("Notes.txt", b"notes"))),
    );
    let file = serde_json::from_str::<Value>(&made).unwrap()["id"]
        .as_str()
        .expect("a file")
        .to_string(); // WHAT: its id. WHY: used in paths.
    let (_, made) = send(
        office,
        "POST",
        "/api/v1/groups",
        Some(&admin),
        &Body::Json(json!({"name": "Shop"})),
    );
    let group = serde_json::from_str::<Value>(&made).unwrap()["id"]
        .as_str()
        .expect("a group")
        .to_string(); // WHAT: its id. WHY: used in paths.
    let (_, joining) = send(
        office,
        "POST",
        "/api/v1/admin/joining",
        Some(&admin),
        &Body::Json(json!({"how": "code", "new_code": true})),
    ); // WHAT: people join with a code. WHY: the join door is open to be knocked on.
    let join = serde_json::from_str::<Value>(&joining).unwrap()["code"]
        .as_str()
        .unwrap_or_default()
        .to_string(); // WHAT: the join code. WHY: joining is asked rightly, so a refusal is the door's.
    Things {
        project,
        set,
        file,
        group,
        join,
    }
}

/// The path and body for one route, made fresh: a person to remove, a key
/// to take back, an invitation to drop.
fn ask(office: &Office, things: &Things, route: &Route, n: usize) -> (String, Body) {
    let fresh_person = || {
        let id = format!("usr_probe_{n}"); // WHAT: a throwaway person. WHY: a removal or role change must not touch the people knocking.
        office.server.store.with(|db| {
            db.execute("INSERT OR IGNORE INTO people (id, name, email, password, role, created) VALUES (?1, 'Throwaway', ?2, 'x', 'viewer', '2026-09-01T00:00:00Z')", params![id, format!("{id}@example.com")])?;
            Ok(())
        }).expect("a throwaway person");
        id
    };
    let fresh_signoff = || {
        let admin = session(office, ADMIN); // WHAT: an administrator. WHY: asks for the line.
        let (_, made) = send(
            office,
            "POST",
            &format!("/api/v1/sets/{}/signoffs", things.set),
            Some(&admin),
            &Body::Json(json!([{"label": "Probe", "ref": format!("probe:{n}")}])),
        ); // WHAT: one line on the probe's set. WHY: a real id to knock with.
        serde_json::from_str::<Value>(&made).unwrap()[0]["id"]
            .as_str()
            .expect("a sign-off")
            .to_string()
    };
    let path = route
        .path
        .replace(
            "{id}",
            &match route.path {
                p if p.starts_with("/projects/") => things.project.clone(),
                p if p.starts_with("/sets/") => things.set.clone(),
                p if p.starts_with("/files/") => things.file.clone(),
                p if p.starts_with("/groups/") => things.group.clone(),
                p if p.starts_with("/people/") => fresh_person(),
                p if p.starts_with("/keys/") => key(office, ADMIN, "assistant", None).1,
                p if p.starts_with("/signoffs/") => fresh_signoff(), // WHAT: a line of its own. WHY: a signature is final, so each knock signs a fresh one.
                p if p.starts_with("/cad/drawings/") => "1".to_string(), // WHAT: the probe's CAD drawing. WHY: a real one, so the door is what answers
                _ => "nothing_by_that_id".to_string(),
            },
        )
        .replace("{version}", "9.9.9")
        .replace("{code}", "beam-beam-beam-beam-beam-1234"); // WHAT: the ids put in. WHY: real ones where the thing exists, a missing one where it doesn't matter.
    let unique = format!("probe{n}"); // WHAT: a name used once. WHY: emails and job numbers mustn't clash.
    let body = match (route.method, route.path) {
        ("POST", "/auth/token") => {
            Body::Json(json!({"email": "eve@example.com", "password": PASSWORD}))
        }
        ("POST", "/setup") => Body::Json(
            json!({"company": "X", "name": "X", "email": "x@example.com", "password": PASSWORD}),
        ),
        ("POST", "/join") => Body::Json(
            json!({"code": things.join, "name": "Joiner", "email": format!("{unique}@example.com"), "password": PASSWORD}),
        ),
        ("POST", "/admin/joining") => Body::Json(json!({})),
        ("POST", "/admin/invites") => {
            Body::Json(json!({"email": format!("{unique}@example.com"), "role": "viewer"}))
        }
        ("POST", "/me/password") => Body::Json(json!({"current": PASSWORD, "new": PASSWORD})),
        ("POST", "/me/keys") => Body::Json(json!({"purpose": "assistant", "name": unique})),
        ("POST", "/admin/updates") => Body::Json(json!({"channel": "stable"})),
        ("POST", "/admin/fleet") => Body::Json(json!({"on": false})),
        ("POST", "/admin/remote") => {
            Body::Json(json!({"token": "short", "hostname": "drawings.example.com"}))
        }
        ("POST", "/projects") => Body::Json(json!({"number": unique, "name": "Probe"})),
        ("POST", "/projects/{id}/people") => Body::Json(json!({"person": VIEWER, "on": false})),
        ("POST", "/projects/{id}/archive") => Body::Json(json!({"archived": false})),
        ("POST", "/projects/{id}/place") => Body::Json(json!({"place": ""})),
        ("POST", "/sets") => form(
            &[("project", &things.project)],
            Some(("S-101.pdf", &a_drawing())),
        ),
        ("POST", "/sets/{id}/markups") => Body::Json(json!([])),
        ("POST", "/sets/{id}/copy") => Body::Json(json!({})),
        ("POST", "/people") => Body::Json(
            json!({"name": "New", "email": format!("{unique}@example.com"), "password": PASSWORD, "role": "viewer"}),
        ),
        ("POST", "/people/{id}/role") => Body::Json(json!({"role": "estimator"})),
        ("POST", "/chests") => form(&[], None),
        ("POST", "/plugins") => {
            Body::Raw("application/octet-stream".into(), b"not a plugin".to_vec())
        }
        ("POST", "/plugins/{id}/run") => Body::Raw("application/json".into(), b"{}".to_vec()),
        ("POST", "/notices") => Body::Json(json!({"url": "http://127.0.0.1:9/hook"})),
        ("POST", "/license") => Body::Json(json!({"license": "not a license"})),
        ("POST", "/admin/pin") => Body::Json(json!({"version": ""})),
        ("POST", "/admin/releases") => form(&[], None),
        ("POST", "/admin/releases/{version}/ready") => Body::Json(json!({"ready": false})),
        ("POST", "/groups") => Body::Json(json!({"name": "Field"})),
        ("POST", "/groups/{id}/people") => Body::Json(json!({"person": VIEWER, "on": false})),
        ("POST", "/projects/{id}/folders") => Body::Json(json!({"folders": ["Probe"]})),
        ("POST", "/files") => form(
            &[("project", &things.project)],
            Some((&format!("{unique}.txt"), b"x")),
        ),
        ("POST", "/sets/{id}/signoffs") => {
            Body::Json(json!([{"label": "Knock", "ref": format!("knock:{unique}")}]))
        }
        ("POST", "/signoffs/{id}/sign") => {
            Body::Json(json!({"name": "Knock", "strokes": [[[1.0, 1.0], [9.0, 9.0]]]}))
        }
        ("POST", "/files/{id}/folder") | ("POST", "/sets/{id}/folder") => {
            Body::Json(json!({"folder": null}))
        }
        ("POST", "/cad/drawings") | ("PUT", "/cad/drawings/{id}/file") => Body::Raw("application/octet-stream".into(), b"a drawing".to_vec()), // WHAT: a drawing's bytes. WHY: what CAD sends
        ("POST", _) | ("DELETE", _) => Body::Raw("application/json".into(), Vec::new()), // WHAT: nothing in the body. WHY: the routes that take none.
        _ => Body::None,
    };
    (format!("/api/v1{path}"), body)
}

#[test]
fn every_route_lets_in_exactly_the_callers_its_door_is_for() {
    let office = an_office(); // WHAT: a running office. WHY: the real doors.
    let things = things(&office); // WHAT: the things asked about. WHY: real ids.

    // WHAT: the table covers every route the router describes. WHY: a new route must be given its door on purpose.
    let described: BTreeSet<String> = api::ROUTES
        .iter()
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    let tabled: BTreeSet<String> = TABLE
        .iter()
        .map(|r| format!("{} {}", r.method, r.path))
        .collect();
    let missing: Vec<&String> = described.difference(&tabled).collect(); // WHAT: routes with no door here. WHY: each one is a failure.
    assert!(
        missing.is_empty(),
        "routes the probe doesn't know the door of: {missing:?}"
    );

    // WHAT: the sync column is the server's own list. WHY: two lists that disagree are a door left open.
    let sync_here: BTreeSet<String> = TABLE
        .iter()
        .filter(|r| r.sync)
        .map(|r| format!("{} {}", r.method, r.path.replace("{id}", ":id")))
        .collect();
    let sync_there: BTreeSet<String> = api::SYNC_ROUTES
        .iter()
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert_eq!(sync_here, sync_there, "the sync key's list");

    let mut wrong = Vec::new(); // WHAT: every door that let in the wrong caller or shut out the right one. WHY: all reported at once.
    let mut fell_over = Vec::new(); // WHAT: every answer of 500 or more. WHY: a broken door is a finding too.
    let mut n = 0; // WHAT: a counter. WHY: fresh names per request.
    for route in TABLE {
        for knock in EVERYBODY {
            n += 1;
            let token = token_for(&office, knock); // WHAT: a fresh token for this knock. WHY: a sign-out or a password change spoils nothing after it.
            let (path, body) = ask(&office, &things, route, n); // WHAT: what is asked. WHY: a valid body, so the door is what answers.
            let (status, said) = send(&office, route.method, &path, token.as_deref(), &body);
            if status >= 500 {
                fell_over.push(format!(
                    "{} {} as {knock:?}: {status} {said}",
                    route.method, route.path
                ));
                continue;
            }
            let refused = status == 401 || status == 403; // WHAT: turned away. WHY: what a door says when it holds.
            let expected = let_in(route, knock); // WHAT: what the table says. WHY: compared.
                                                 // WHAT: a key that asks to sign out is told so with 400, which lets nothing happen. WHY: it is not a session, and the route says why rather than refusing.
            let key_told_off = route.path == "/signout"
                && matches!(
                    knock,
                    Knock::Integration | Knock::DemotedProgram | Knock::ViewerProgram
                )
                && status == 400;
            if refused == expected && !key_told_off {
                wrong.push(format!(
                    "{} {} as {knock:?}: {status} ({}) {said}",
                    route.method,
                    route.path,
                    if expected {
                        "should be let in"
                    } else {
                        "should be turned away"
                    }
                ));
            }
        }
    }
    assert!(
        fell_over.is_empty(),
        "doors that fell over:\n{}",
        fell_over.join("\n")
    );
    assert!(
        wrong.is_empty(),
        "{} wrong of {} knocks:\n{}",
        wrong.len(),
        TABLE.len() * EVERYBODY.len(),
        wrong.join("\n")
    );
}

#[test]
fn the_assistant_door_is_for_people_assistants_and_programs_never_sync_keys() {
    let office = an_office(); // WHAT: a running office. WHY: the real /mcp.
    let _ = things(&office); // WHAT: a job in it. WHY: so listing jobs has something to say.
    let call = Body::Json(
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "list_projects", "arguments": {}}}),
    ); // WHAT: one tool call. WHY: every tool asks who first.
    for knock in EVERYBODY {
        let token = token_for(&office, knock); // WHAT: a fresh token. WHY: as above.
        let (status, said) = send(&office, "POST", "/mcp", token.as_deref(), &call);
        assert_eq!(status, 200, "{knock:?}: {said}"); // WHAT: the protocol answers 200. WHY: a refusal is inside the answer.
        let answer: Value = serde_json::from_str(&said).unwrap(); // WHAT: the JSON-RPC answer. WHY: read.
        let turned_away = answer.get("error").is_some(); // WHAT: refused. WHY: how /mcp says no.
        let should = matches!(
            knock,
            Knock::Viewer
                | Knock::Estimator
                | Knock::Admin
                | Knock::Integration
                | Knock::DemotedProgram
                | Knock::ViewerProgram
                | Knock::Assistant
        ); // WHAT: who the door is for. WHY: an assistant's key works here and only here; a sync key never.
        assert_eq!(!turned_away, should, "{knock:?} at /mcp: {said}");
    }
}

#[test]
fn fleet_doors_open_to_the_maintenance_key_and_to_nothing_any_person_or_program_holds() {
    let office = an_office(); // WHAT: a running office with a maintenance key. WHY: Fleet's doors exist.
    for (method, path) in [
        ("GET", "/api/admin/ops"),
        ("GET", "/api/admin/logtail"),
        ("GET", "/api/admin/presence"),
        ("POST", "/api/admin/update/rollback"),
    ] {
        for knock in EVERYBODY {
            let token = token_for(&office, knock); // WHAT: whatever this caller holds. WHY: none of it is the maintenance key.
            let (status, said) = send(&office, method, path, token.as_deref(), &Body::None);
            assert!(
                status == 401 || status == 404,
                "{method} {path} as {knock:?}: {status} {said}"
            ); // WHAT: refused. WHY: a session or a key is not the maintenance key.
        }
        let url = format!("{}{path}", office.base); // WHAT: the address. WHY: asked with the key.
        let answer = ureq::request(method, &url)
            .set("x-av-update-key", MAINTENANCE)
            .call(); // WHAT: with the maintenance key. WHY: the one thing that opens it.
        assert!(
            answer.is_ok(),
            "{method} {path} with the maintenance key: {answer:?}"
        );
    }
    let (status, _) = send(&office, "GET", "/api/version", None, &Body::None); // WHAT: the version, asked by nobody. WHY: Fleet's contract keeps it public.
    assert_eq!(status, 200);
}
