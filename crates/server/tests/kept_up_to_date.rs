//! A company's server keeping itself and its seats up to date, and an
//! administrator looking after the office — run for real, over sockets.
//!
//! The publisher's feed here is a stand-in served from this test, shaped
//! exactly like the GitHub releases page the real one is: a list of releases,
//! each with a `release.json` and the program files. Releases are signed with
//! a throwaway key the test trusts, never the real one.

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use ed25519_dalek::{Signer, SigningKey};
use hub::feed::{Feed, Published, APP_PLATFORM, SERVER_PLATFORM};
use hub::update::{hex, Channel, Release, Trusted};
use hub::Client;
use hyperview_server::api::{self, Server};
use hyperview_server::updates::{check_with, Outcome};
use hyperview_server::{auth, Config, Store};
use rusqlite::params;
use sha2::{Digest, Sha256};

struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random();
        let path = std::env::temp_dir().join(format!("hyperview-kept-{n:016x}"));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publisher() -> (SigningKey, Trusted) {
    let key = SigningKey::from_bytes(&[42u8; 32]);
    let trusted = Trusted {
        keys: vec![("test-2026".into(), key.verifying_key().to_bytes())],
    };
    (key, trusted)
}

fn sign(
    key: &SigningKey,
    version: &str,
    channel: Channel,
    platform: &str,
    file: &str,
    bytes: &[u8],
) -> Release {
    let mut release = Release {
        version: version.into(),
        channel,
        platform: platform.into(),
        published: "2026-09-21T12:00:00Z".into(),
        notes: "Better grid bubbles.".into(),
        download: file.into(),
        bytes: bytes.len() as u64,
        digest: hex(&Sha256::digest(bytes)),
        signature: String::new(),
        key: "test-2026".into(),
        minimum_api_version: 1,
    };
    release.signature = hex(&key.sign(&release.signing_payload()).to_bytes());
    release
}

/// One release on the stand-in feed.
struct Listed {
    tag: String,
    prerelease: bool,
    published: Published,
    /// Every file on the release, by the name its manifest gives it. A
    /// release carries one program per platform, so this is a list rather
    /// than a field per program.
    files: Vec<(String, Vec<u8>)>,
}

/// Serves releases the way GitHub does, and returns the list's address.
fn a_feed(runtime: &tokio::runtime::Runtime, releases: Vec<Listed>) -> String {
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .expect("a socket");
    let base = format!("http://{}", listener.local_addr().unwrap());
    let releases = Arc::new(releases);

    let list = {
        let releases = Arc::clone(&releases);
        let base = base.clone();
        move || {
            let releases = Arc::clone(&releases);
            let base = base.clone();
            async move {
                let body: Vec<serde_json::Value> = releases
                    .iter()
                    .map(|r| {
                        let asset = |name: &str| {
                            serde_json::json!({
                                "name": name,
                                "browser_download_url": format!("{base}/files/{}/{name}", r.tag),
                            })
                        };
                        // Each program under the name its manifest gives it, as
                        // the publishing script uploads them.
                        let mut assets = vec![asset("release.json")];
                        assets.extend(r.files.iter().map(|(name, _)| asset(name)));
                        serde_json::json!({
                            "tag_name": r.tag,
                            "draft": false,
                            "prerelease": r.prerelease,
                            "assets": assets,
                        })
                    })
                    .collect();
                axum::Json(body)
            }
        }
    };
    let files = {
        let releases = Arc::clone(&releases);
        move |axum::extract::Path((tag, name)): axum::extract::Path<(String, String)>| {
            let releases = Arc::clone(&releases);
            async move {
                let Some(r) = releases.iter().find(|r| r.tag == tag) else {
                    return (axum::http::StatusCode::NOT_FOUND, Vec::new());
                };
                let bytes = if name == "release.json" {
                    serde_json::to_vec(&r.published).unwrap()
                } else if let Some((_, bytes)) = r.files.iter().find(|(n, _)| *n == name) {
                    bytes.clone()
                } else {
                    return (axum::http::StatusCode::NOT_FOUND, Vec::new());
                };
                (axum::http::StatusCode::OK, bytes)
            }
        }
    };
    let app = Router::new()
        .route("/releases", get(list))
        .route("/files/:tag/:name", get(files));
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("{base}/releases")
}

fn a_release(key: &SigningKey, version: &str, prerelease: bool) -> Listed {
    a_release_named(key, version, prerelease, "Hyperview.exe", "Hyperview-Server.exe")
}

/// A release whose programs are uploaded under these names.
fn a_release_named(key: &SigningKey, version: &str, prerelease: bool, viewer: &str, server_file: &str) -> Listed {
    let channel = if prerelease { Channel::Preview } else { Channel::Stable };
    let app = format!("MZ pretend viewer {version}").into_bytes();
    let server = format!("MZ pretend server {version}").into_bytes();
    Listed {
        tag: format!("v{version}"),
        prerelease,
        published: Published {
            app: sign(key, version, channel, APP_PLATFORM, viewer, &app),
            apps: Vec::new(),
            server: Some(sign(key, version, channel, SERVER_PLATFORM, server_file, &server)),
            servers: Vec::new(),
        },
        files: vec![(viewer.to_string(), app), (server_file.to_string(), server)],
    }
}

/// The names the two platforms are published under, spelled out rather than
/// taken from this build, because this test is about an office holding builds
/// for machines that are not the one it runs on.
const WINDOWS: &str = "windows-x64";
const MAC: &str = "macos-universal";

/// One release carrying both builds, the way the publishing script writes one
/// once there is a Mac version.
fn a_release_for_both_platforms(key: &SigningKey, version: &str) -> Listed {
    let windows = format!("MZ pretend viewer {version}").into_bytes();
    let mac = format!("PK pretend mac {version}").into_bytes();
    let channel = Channel::Stable;
    let for_windows = sign(key, version, channel, WINDOWS, "ExcaliburView.exe", &windows);
    let for_mac = sign(key, version, channel, MAC, "ExcaliburView-mac.zip", &mac);
    Listed {
        tag: format!("v{version}"),
        prerelease: false,
        published: Published {
            // `app` stays the Windows build, for every copy installed before
            // there was another kind.
            app: for_windows.clone(),
            apps: vec![for_windows, for_mac],
            server: None,
            servers: Vec::new(),
        },
        files: vec![
            ("ExcaliburView.exe".to_string(), windows),
            ("ExcaliburView-mac.zip".to_string(), mac),
        ],
    }
}

/// A company's server with an administrator and an estimator on it.
struct Office {
    server: Arc<Server>,
    base: String,
    runtime: tokio::runtime::Runtime,
    _scratch: Scratch,
}

fn an_office() -> Office {
    an_office_reading(None)
}

/// An office whose server reads `feed` for itself, the way an installed one
/// does, rather than only when a test asks it to.
fn an_office_reading(feed: Option<Vec<Listed>>) -> Office {
    an_office_with(feed, Vec::new())
}

fn an_office_with(feed: Option<Vec<Listed>>, plugin_keys: Vec<(String, [u8; 32])>) -> Office {
    let scratch = Scratch::new();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let release_feed = feed.map(|releases| a_feed(&runtime, releases));
    let config = Config {
        name: "Example Steel".into(),
        data: scratch.0.clone(),
        listen: "127.0.0.1:0".parse().unwrap(),
        check_for_updates: release_feed.is_some(),
        release_feed,
        plugin_keys_for_tests: plugin_keys,
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).expect("a store");
    for (name, email, password, role) in [
        ("Sam", "sam@example.com", "brace-gusset-purlin-shim-42", "admin"),
        ("Estimator", "est@example.com", "camber-weld-joist-plate-19", "estimator"),
    ] {
        let hashed = auth::hash_password(password).unwrap();
        store
            .with(|db| {
                db.execute(
                    "INSERT INTO people (id, name, email, password, role, created)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                    params![api::fresh_id("usr"), name, email, hashed, role, api::now()],
                )?;
                Ok(())
            })
            .unwrap();
    }
    let listener = runtime
        .block_on(tokio::net::TcpListener::bind(config.listen))
        .unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = Arc::new(Server::new(store, config));
    let app = api::router(Arc::clone(&server));
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Office {
        server,
        base,
        runtime,
        _scratch: scratch,
    }
}

fn signed_in(office: &Office, who: &str, password: &str) -> Client {
    let mut client = Client::new(&office.base);
    client.sign_in(who, password).expect("sign in");
    client
}

fn sam(office: &Office) -> Client {
    signed_in(office, "sam@example.com", "brace-gusset-purlin-shim-42")
}

#[test]
fn a_release_whose_files_carry_the_new_names_still_reaches_everyone() {
    // From 0.6.2 the programs go up as ExcaliburView.exe and
    // ExcaliburView-Server.exe. Every copy already installed finds its file by
    // the name in release.json, never by a name of its own, so nothing
    // installed has to know.
    let office = an_office();
    let (key, trusted) = publisher();
    let listed = a_release_named(&key, "9.4.0", false, "ExcaliburView.exe", "ExcaliburView-Server.exe");
    let feed = a_feed(&office.runtime, vec![listed]);

    let outcome = check_with(&office.server, &Feed::new(&feed), &trusted);
    assert_eq!(outcome, Outcome::Offered("9.4.0".into()));
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let release = seat
        .update_offer("0.6.1", Channel::Stable, APP_PLATFORM)
        .unwrap()
        .release
        .expect("the seat is offered it");
    let bytes = seat.download_update(&release.download).unwrap();
    assert_eq!(trusted.check(&release, &bytes), Ok(()));
    assert_eq!(bytes, b"MZ pretend viewer 9.4.0");

    // And a seat with no server, or a server, reading the feed itself.
    let found = Feed::new(&feed).newest(Channel::Stable).unwrap().expect("the release");
    for (part, expected) in [
        (&found.published.app, &b"MZ pretend viewer 9.4.0"[..]),
        (found.published.server.as_ref().unwrap(), &b"MZ pretend server 9.4.0"[..]),
    ] {
        let url = found.url(&part.download).expect("the file is on the release under its manifest name");
        let bytes = Feed::new(&feed).fetch(url).unwrap();
        assert_eq!(trusted.check(part, &bytes), Ok(()));
        assert_eq!(bytes, expected);
    }
}

#[test]
fn a_release_on_the_feed_reaches_every_seat_and_checks_out_on_arrival() {
    let office = an_office();
    let (key, trusted) = publisher();
    let feed = a_feed(&office.runtime, vec![a_release(&key, "9.1.0", false)]);

    let outcome = check_with(&office.server, &Feed::new(&feed), &trusted);
    assert_eq!(outcome, Outcome::Offered("9.1.0".into()));

    // A seat on an older version asks its server, as it does on start-up.
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let offer = seat.update_offer("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    let release = offer.release.expect("the seat is offered it");
    assert_eq!(release.version, "9.1.0");

    // And what it downloads is exactly what was signed.
    let bytes = seat.download_update(&release.download).unwrap();
    assert_eq!(trusted.check(&release, &bytes), Ok(()));
    assert_eq!(bytes, b"MZ pretend viewer 9.1.0");

    // Asking again changes nothing and downloads nothing.
    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::UpToDate
    );
}

#[test]
fn a_release_signed_by_anybody_else_never_reaches_a_seat() {
    let office = an_office();
    let (_, trusted) = publisher();
    let impostor = SigningKey::from_bytes(&[9u8; 32]);
    let feed = a_feed(&office.runtime, vec![a_release(&impostor, "9.1.0", false)]);

    match check_with(&office.server, &Feed::new(&feed), &trusted) {
        Outcome::Trouble(why) => assert!(why.contains("9.1.0"), "{why}"),
        other => panic!("it must be refused, not {other:?}"),
    }
    let seat = sam(&office);
    let offer = seat.update_offer("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    assert!(offer.release.is_none(), "nothing may be offered");
}

#[test]
fn an_office_takes_early_releases_only_when_its_administrator_says_so() {
    let office = an_office();
    let (key, trusted) = publisher();
    let feed = a_feed(
        &office.runtime,
        vec![a_release(&key, "9.1.0", false), a_release(&key, "9.2.0", true)],
    );

    // Stable, as every office starts: the early one is not taken.
    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::Offered("9.1.0".into())
    );

    // An estimator cannot change it.
    let estimator = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    assert!(estimator.change_update_channel("early").is_err());

    // The administrator can, and the office gets it on the next look — and
    // every seat is offered it, not just the one that asked for early.
    let admin = sam(&office);
    let settings = admin.change_update_channel("early").unwrap();
    assert_eq!(settings.channel, "early");
    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::Offered("9.2.0".into())
    );
    let offer = estimator.update_offer("9.1.0", Channel::Stable, APP_PLATFORM).unwrap();
    assert_eq!(offer.release.map(|r| r.version).as_deref(), Some("9.2.0"));

    let settings = admin.update_settings().unwrap();
    assert_eq!(settings.offering.as_deref(), Some("9.2.0"));
    assert_eq!(settings.running, env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_new_join_code_stops_the_old_one_working() {
    let office = an_office();
    let admin = sam(&office);
    let first = admin.change_joining(Some("code"), None, true).unwrap().code;
    let second = admin.change_joining(None, None, true).unwrap().code;
    assert!(!first.is_empty());
    assert_ne!(first, second);

    let mut late = Client::new(&office.base);
    let joined = late.join(&hub::Join {
        code: first,
        name: "Somebody".into(),
        email: "somebody@example.com".into(),
        password: "a-long-enough-password".into(),
    });
    assert!(joined.is_err(), "the old code must not work");
    let mut late = Client::new(&office.base);
    late.join(&hub::Join {
        code: second,
        name: "Somebody".into(),
        email: "somebody@example.com".into(),
        password: "a-long-enough-password".into(),
    })
    .expect("the new one does");
}

#[test]
fn changing_a_password_signs_you_out_everywhere_else() {
    let office = an_office();
    let here = sam(&office);
    let elsewhere = sam(&office);

    assert!(here.change_password("not it", "a-brand-new-password-1").is_err());
    assert!(here.change_password("brace-gusset-purlin-shim-42", "short").is_err());
    here.change_password("brace-gusset-purlin-shim-42", "a-brand-new-password-1")
        .expect("changed");

    assert!(here.me().is_ok(), "this session carries on");
    assert!(elsewhere.me().is_err(), "every other session is signed out");
    let mut again = Client::new(&office.base);
    assert!(again.sign_in("sam@example.com", "brace-gusset-purlin-shim-42").is_err());
    again
        .sign_in("sam@example.com", "a-brand-new-password-1")
        .expect("the new password works");
}

#[test]
fn a_drawing_set_put_on_the_server_from_a_seat_is_there_for_everybody() {
    let office = an_office();
    let estimator = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let project = estimator.create_project("2601", "Elm Theater").unwrap();
    let set = estimator
        .upload_set(&project.id, "Structural", "S-Structural.pdf", &a_drawing())
        .expect("uploaded");
    assert_eq!(set.sheets, 2);

    let admin = sam(&office);
    let sets = admin.sets(&project.id).unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].name, "Structural");
}

#[test]
fn a_tool_chest_is_shared_by_an_administrator_and_nobody_else() {
    let office = an_office();
    let estimator = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    assert!(estimator.upload_chest("Takeoff", "Takeoff.bpx", b"anything").is_err());
    // The administrator's upload reaches the server intact; a file with no
    // tools in it is refused for what it is, not for how it was sent.
    let admin = sam(&office);
    match admin.upload_chest("Takeoff", "Takeoff.bpx", b"not a chest") {
        Err(e) => assert!(e.to_string().contains("No tools"), "{e}"),
        Ok(_) => panic!("an empty chest must be refused"),
    }
}

#[test]
fn an_administrator_lets_the_fleet_watch_and_can_shut_it_out_again() {
    let office = an_office();
    let ops = |key: &str| {
        ureq::get(&format!("{}/api/admin/ops", office.base))
            .set("x-av-update-key", key)
            .call()
            .map(|r| r.status())
            .unwrap_or_else(|e| match e {
                ureq::Error::Status(code, _) => code,
                _ => 0,
            })
    };
    // Until somebody asks, there is no door at all.
    assert_eq!(ops("anything"), 404);

    let estimator = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    assert!(estimator.change_fleet_access(true).is_err());

    let admin = sam(&office);
    let access = admin.change_fleet_access(true).unwrap();
    let key = access.key.expect("the key, once");
    assert!(admin.fleet_access().unwrap().on);
    assert!(admin.fleet_access().unwrap().key.is_none(), "never shown twice");
    assert_eq!(ops(&key), 200);
    assert_eq!(ops(&"0".repeat(key.len())), 401);

    // What the Fleet's board reads first, with no key at all.
    let version: serde_json::Value = ureq::get(&format!("{}/api/version", office.base))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(version["product"], "hyperview");
    assert_eq!(version["version"], env!("CARGO_PKG_VERSION"));

    admin.change_fleet_access(false).unwrap();
    assert_eq!(ops(&key), 404);
}

/// A small but real PDF: two pages.
fn a_drawing() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n");
    let mut offsets = Vec::new();
    for body in [
        "1 0 obj\n<</Type/Catalog/Pages 2 0 R>>\nendobj\n",
        "2 0 obj\n<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>\nendobj\n",
        "3 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
        "4 0 obj\n<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>\nendobj\n",
    ] {
        offsets.push(out.len());
        out.extend_from_slice(body.as_bytes());
    }
    let xref = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len() + 1).as_bytes());
    for offset in &offsets {
        out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{xref}\n%%EOF\n", offsets.len() + 1)
            .as_bytes(),
    );
    out
}

#[test]
fn somebody_pressing_check_for_updates_has_the_server_look_there_and_then() {
    // Signed by a key only this test trusts, so the server — which trusts the
    // publisher's real key and nothing else — looks, finds it, and refuses
    // it. What matters here is that it looked, when asked, and said so.
    let (key, _) = publisher();
    let office = an_office_reading(Some(vec![a_release(&key, "9.1.0", false)]));
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");

    assert!(office.server.updates.since_last().is_none(), "nobody has looked yet");
    let offer = seat.update_offer_now("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    assert!(offer.release.is_none(), "a release nobody signed is never offered");
    assert!(!offer.looking, "the look finished before the answer");
    assert!(office.server.updates.since_last().is_some(), "the server looked");
    let note = office
        .server
        .store
        .setting(hyperview_server::updates::settings::NOTE)
        .unwrap()
        .unwrap_or_default();
    assert!(note.contains("9.1.0"), "what it found is written down: {note}");
}

#[test]
fn an_ordinary_question_sets_off_a_look_when_the_last_one_is_old() {
    let (key, _) = publisher();
    let office = an_office_reading(Some(vec![a_release(&key, "9.1.0", false)]));
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");

    // Answered at once, from what the server already has, which is nothing.
    let offer = seat.update_offer("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    assert!(offer.release.is_none());

    // And the look goes on behind the answer.
    let started = std::time::Instant::now();
    while office.server.updates.since_last().is_none() {
        assert!(
            started.elapsed() < std::time::Duration::from_secs(10),
            "asking never set off a look"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

#[test]
fn a_server_told_not_to_look_does_not_look_however_it_is_asked() {
    let office = an_office();
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let offer = seat.update_offer_now("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    assert!(offer.release.is_none() && !offer.looking);
    assert!(office.server.updates.since_last().is_none());
}

// ---- keys for programs, and the markup list another system reads ----------

#[test]
fn fabwire_gets_a_key_from_an_administrator_and_nobody_else() {
    let office = an_office();
    let estimator = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    assert!(estimator.make_key("integration", "FabWire").is_err(), "only an administrator");

    let admin = sam(&office);
    let made = admin.make_key("integration", "FabWire").expect("an administrator can");
    let key = made.key.clone().expect("the key comes back once");
    assert!(key.starts_with("hvk_"));

    // FabWire, with nothing but the key.
    let fabwire = Client::new(&office.base).with_token(&key);
    assert!(fabwire.projects().is_ok(), "the key works on the API");
    assert!(fabwire.make_key("integration", "Another").is_err(), "a key cannot make a key");

    // Listed without the key itself, and taken back.
    let listed = admin.keys().unwrap();
    assert!(listed.iter().any(|k| k.name == "FabWire" && k.key.is_none()));
    admin.revoke_key(&made.id).unwrap();
    assert!(fabwire.projects().is_err(), "a revoked key stops working at once");
}

#[test]
fn an_assistant_key_reads_the_takeoffs_and_nothing_else() {
    let office = an_office();
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let made = seat.make_key("assistant", "Assistant on the estimator's PC").unwrap();
    let key = made.key.unwrap();

    // Not the API.
    let assistant = Client::new(&office.base).with_token(&key);
    assert!(assistant.projects().is_err());

    // The MCP endpoint, yes.
    let answer: serde_json::Value = ureq::post(&format!("{}/mcp", office.base))
        .set("Authorization", &format!("Bearer {key}"))
        .send_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": { "name": "list_projects", "arguments": {} }
        }))
        .unwrap()
        .into_json()
        .unwrap();
    assert!(answer.get("result").is_some(), "{answer}");
}

#[test]
fn a_project_with_no_sets_has_an_empty_markup_list_not_an_error() {
    let office = an_office();
    let admin = sam(&office);
    let project = admin.create_project("2630", "Riverside Medical").unwrap();
    let key = admin.make_key("integration", "FabWire").unwrap().key.unwrap();
    let answer: serde_json::Value = ureq::get(&format!("{}/api/v1/projects/{}/markuplist", office.base, project.id))
        .set("Authorization", &format!("Bearer {key}"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(answer["results"].as_array().map(|a| a.len()), Some(0), "{answer}");
    assert!(admin.markup_list("no-such-set").is_err());
}

// ---- plugins the office hands out ------------------------------------------

fn a_plugin(key: &SigningKey, id: &str, version: &str, wasm: &[u8]) -> Vec<u8> {
    let sha = hex(&Sha256::digest(wasm));
    let signature = key.sign(&plugin_api::signing_payload(id, version, &sha));
    plugin_api::seal(
        &plugin_api::Header {
            id: id.into(),
            version: version.into(),
            name: "Shop Estimating".into(),
            sha256: sha,
            bytes: wasm.len() as u64,
            key: "test-2026".into(),
            signature: hex(&signature.to_bytes()),
        },
        wasm,
    )
}

#[test]
fn an_administrator_hands_a_signed_plugin_to_every_seat_and_can_take_it_back() {
    let (key, trusted) = publisher();
    let office = an_office_with(None, trusted.keys.clone());
    let admin = sam(&office);
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");

    let first = a_plugin(&key, "shop-estimating", "1.0.0", b"\0asm\x01\0\0\0 one");
    assert!(seat.upload_plugin(&first).is_err(), "only an administrator adds plugins");
    let added = admin.upload_plugin(&first).expect("an administrator can");
    assert_eq!(added.id, "shop-estimating");
    assert_eq!(added.uploaded_by, "Sam");

    // Every seat sees it, and gets exactly the bytes that were signed.
    let listed = seat.plugins().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(seat.plugin_file("shop-estimating").unwrap(), first);

    // A newer version replaces the older rather than sitting beside it.
    let second = a_plugin(&key, "shop-estimating", "1.1.0", b"\0asm\x01\0\0\0 two");
    admin.upload_plugin(&second).unwrap();
    let listed = seat.plugins().unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].version, "1.1.0");

    admin.remove_plugin("shop-estimating").unwrap();
    assert!(seat.plugins().unwrap().is_empty());
}

#[test]
fn a_server_will_not_hand_out_a_plugin_nobody_trusted_signed() {
    let (_, trusted) = publisher();
    let office = an_office_with(None, trusted.keys.clone());
    let admin = sam(&office);
    let stranger = SigningKey::from_bytes(&[43u8; 32]);
    let forged = a_plugin(&stranger, "shop-estimating", "1.0.0", b"\0asm\x01\0\0\0");
    let refused = admin.upload_plugin(&forged).unwrap_err().to_string();
    assert!(refused.contains("not signed"), "{refused}");
    // Nor a file that is not a plugin at all.
    assert!(admin.upload_plugin(b"MZ a program").is_err());
    assert!(admin.plugins().unwrap().is_empty());
}

/// A real plugin, small enough to write by hand: one command, "hello", that
/// finds one thing.
fn a_working_plugin() -> Vec<u8> {
    let manifest = r#"{"id":"shop-estimating","name":"Shop Estimating","version":"1.0.0","commands":[{"id":"hello","name":"Hello"}]}"#;
    let answer = r#"{"done":{"title":"Hello","findings":[{"level":"check","message":"Look here","page":0,"area":[1,2,3,4]}]}}"#;
    let escape = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let text = format!(
        r#"(module
            (memory (export "memory") 1)
            (global $next (mut i32) (i32.const 8192))
            (data (i32.const 0) "{m}")
            (data (i32.const 2048) "{a}")
            (func (export "hv_alloc") (param i32) (result i32) (local i32)
                global.get $next
                local.set 1
                global.get $next
                local.get 0
                i32.add
                global.set $next
                local.get 1)
            (func (export "hv_manifest") (result i64)
                i64.const {ml})
            (func (export "hv_run") (param i32 i32) (result i64)
                i64.const {packed}))"#,
        m = escape(manifest),
        a = escape(answer),
        ml = manifest.len(),
        packed = (2048i64 << 32) | i64::try_from(answer.len()).unwrap(), // WHAT: the answer's place and length in one number. WHY: how the plugin hands it back
    );
    wat::parse_str(text).unwrap()
}

#[test]
fn a_seat_that_cant_run_plugins_has_the_server_run_them() {
    let (key, trusted) = publisher();
    let office = an_office_with(None, trusted.keys.clone());
    let admin = sam(&office);
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    admin
        .upload_plugin(&a_plugin(&key, "shop-estimating", "1.0.0", &a_working_plugin()))
        .unwrap();

    // The list says what it can do, so a Mac App Store copy can build its menu
    // without ever holding the plugin.
    let listed = seat.plugins().unwrap();
    let manifest = listed[0].manifest.as_ref().expect("the server read the manifest");
    assert_eq!(manifest.commands[0].id, "hello");

    let input = plugin_api::Input {
        command: "hello".into(),
        sheets: vec![plugin_api::Sheet { page: 0, name: "S-101".into(), ..Default::default() }],
        ..Default::default()
    };
    match seat.run_plugin("shop-estimating", &input).unwrap() {
        plugin_api::Answer::Done(output) => {
            assert_eq!(output.title, "Hello");
            assert_eq!(output.findings[0].message, "Look here");
        }
        plugin_api::Answer::Failed(why) => panic!("it failed: {why}"),
    }
    assert!(seat.run_plugin("no-such-plugin", &input).is_err());
}

#[test]
fn a_plugin_that_isnt_really_one_says_so_rather_than_running() {
    let (key, trusted) = publisher();
    let office = an_office_with(None, trusted.keys.clone());
    let admin = sam(&office);
    // Signed properly, but not WebAssembly that does anything.
    admin
        .upload_plugin(&a_plugin(&key, "shop-estimating", "1.0.0", b"\0asm\x01\0\0\0 one"))
        .unwrap();
    let listed = admin.plugins().unwrap();
    assert!(listed[0].manifest.is_none());
    let input = plugin_api::Input { command: "hello".into(), ..Default::default() };
    match admin.run_plugin("shop-estimating", &input).unwrap() {
        plugin_api::Answer::Failed(why) => assert!(!why.is_empty()),
        plugin_api::Answer::Done(_) => panic!("that was never a plugin"),
    }
}

// ---- an office with Macs in it ----------------------------------------------

/// The whole point of the release carrying two builds: a Windows office holds
/// the Mac one for the Macs sitting in it, and each seat is handed its own.
#[test]
fn an_office_holds_both_builds_and_hands_each_seat_its_own() {
    let office = an_office();
    let (key, trusted) = publisher();
    let feed = a_feed(&office.runtime, vec![a_release_for_both_platforms(&key, "9.4.0")]);

    let outcome = check_with(&office.server, &Feed::new(&feed), &trusted);
    assert_eq!(outcome, Outcome::Offered("9.4.0".into()));

    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");

    let windows = seat
        .update_offer("0.6.3", Channel::Stable, WINDOWS)
        .unwrap()
        .release
        .expect("the Windows seat is offered it");
    assert_eq!(windows.platform, WINDOWS);
    let bytes = seat.download_update(&windows.download).unwrap();
    assert_eq!(trusted.check(&windows, &bytes), Ok(()));
    assert_eq!(bytes, b"MZ pretend viewer 9.4.0");

    let mac = seat
        .update_offer("0.6.3", Channel::Stable, MAC)
        .unwrap()
        .release
        .expect("the Mac seat is offered it");
    assert_eq!(mac.platform, MAC);
    let bytes = seat.download_update(&mac.download).unwrap();
    assert_eq!(trusted.check(&mac, &bytes), Ok(()));
    assert_eq!(bytes, b"PK pretend mac 9.4.0");

    // Asking again downloads nothing: the office already holds both.
    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::UpToDate
    );
}

/// A release with the Windows build in it and nothing else, whatever kind of
/// machine this test is running on.
///
/// `a_release` signs for `APP_PLATFORM`, and on a Mac that IS the Mac
/// platform — so an "office with no Mac build" built that way quietly has
/// one. The test below then passes on Linux and fails on a Mac, which is
/// exactly what happened the first time these tests ran on one.
fn a_windows_only_release(key: &SigningKey, version: &str) -> Listed {
    let windows = format!("MZ pretend viewer {version}").into_bytes();
    Listed {
        tag: format!("v{version}"),
        prerelease: false,
        published: Published {
            app: sign(key, version, Channel::Stable, WINDOWS, "ExcaliburView.exe", &windows),
            apps: Vec::new(),
            server: None,
            servers: Vec::new(),
        },
        files: vec![("ExcaliburView.exe".to_string(), windows)],
    }
}

/// A Mac that turns up in an office which has only ever held Windows builds
/// is told there is nothing for it — not handed the Windows one, and not told
/// it is up to date when it has never had a version at all.
#[test]
fn a_mac_in_an_office_with_no_mac_build_is_offered_nothing_rather_than_the_wrong_thing() {
    let office = an_office();
    let (key, trusted) = publisher();
    let feed = a_feed(&office.runtime, vec![a_windows_only_release(&key, "9.4.0")]);
    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::Offered("9.4.0".into())
    );

    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let offer = seat.update_offer("0.6.3", Channel::Stable, MAC).unwrap();
    assert!(offer.release.is_none(), "a Mac was offered {:?}", offer.release);

    // And the Windows seats in that same office are served as they always were.
    let windows = seat.update_offer("0.6.3", Channel::Stable, WINDOWS).unwrap();
    assert_eq!(windows.release.expect("the Windows seat").version, "9.4.0");
}

/// The release before the Mac existed, read by a server that knows about
/// Macs. It must still take the Windows build and still offer it.
#[test]
fn a_release_from_before_there_were_two_platforms_still_reaches_the_windows_seats() {
    let office = an_office();
    let (key, trusted) = publisher();
    // `apps` absent, `app` alone — exactly what 0.6.3 and everything before it
    // was published as.
    let old_shape = a_release_named(&key, "9.4.0", false, "Hyperview.exe", "Hyperview-Server.exe");
    assert!(old_shape.published.apps.is_empty());
    let feed = a_feed(&office.runtime, vec![old_shape]);

    assert_eq!(
        check_with(&office.server, &Feed::new(&feed), &trusted),
        Outcome::Offered("9.4.0".into())
    );
    let seat = signed_in(&office, "est@example.com", "camber-weld-joist-plate-19");
    let offer = seat.update_offer("0.4.1", Channel::Stable, APP_PLATFORM).unwrap();
    assert_eq!(offer.release.expect("still offered").version, "9.4.0");
}
