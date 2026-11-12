//! A Sealed licence, run for real: added over the wire, it seals the server
//! there and then, every seat that asks is told it is Sealed, and the server
//! stops looking for its own renewal (a sealed office gets its licences by
//! hand). Its own test program, because sealing is held for the whole
//! process and would seal any other test running beside it.

use std::sync::Arc; // WHAT: shared ownership of the server. WHY: the router takes it as an Arc.

use ed25519_dalek::{Signer, SigningKey}; // WHAT: Ed25519 signing. WHY: the test signs its own licence.
use hub::license::{Edition, License}; // WHAT: the licence file. WHY: made here as the publishing tools make one.
use hyperview_server::api::{self, Server}; // WHAT: the router and the server state. WHY: the real thing.
use hyperview_server::{auth, Config, Store}; // WHAT: hashing, configuration and storage. WHY: an office with a person in it.
use rusqlite::params; // WHAT: bound SQL parameters. WHY: the administrator goes in directly.
use serde_json::{json, Value}; // WHAT: JSON values. WHY: requests and answers.

#[test]
fn a_sealed_licence_seals_the_server_at_once_and_it_stops_reaching_out_for_renewals() {
    let n: u64 = rand::random(); // WHAT: a random number. WHY: a folder no other test uses.
    let folder = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("hyperview-sealed-{n:016x}")); // WHAT: the office's data folder. WHY: its own, under the build's scratch folder, not one shared with other users
    std::fs::create_dir_all(&folder).unwrap();
    let signing = SigningKey::from_bytes(&[13u8; 32]); // WHAT: a made-up licence key. WHY: never a real one.
    let config = Config {
        data: folder.clone(),                   // WHAT: the folder. WHY: everything goes there.
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port. WHY: tests run side by side.
        license_keys_for_tests: vec![("sealed-test".into(), signing.verifying_key().to_bytes())], // WHAT: the test key. WHY: as a real server trusts its built-in ones
        ..Config::default()
    };
    let store = Store::open(&config.database(), &config.blobs()).unwrap(); // WHAT: the database. WHY: the administrator goes in.
    let hashed = auth::hash_password("brace-gusset-purlin-shim-42").unwrap(); // WHAT: a password. WHY: to sign in.
    store
        .with(|db| {
            db.execute(
                "INSERT INTO people (id, name, email, password, role, created) VALUES ('usr_a', 'Sam', 'sam@example.com', ?1, 'admin', '2026-11-10T00:00:00Z')",
                params![hashed],
            )?; // WHAT: the administrator. WHY: adds the licence.
            Ok(())
        })
        .unwrap();
    let probe = Server::new(Store::open(&config.database(), &config.blobs()).unwrap(), config.clone()); // WHAT: a second handle on the same office. WHY: to ask the server's own licence code what it would fetch
    hub::sealed::set_for_testing(hub::sealed::Sealed::No); // WHAT: an ordinary machine to start with. WHY: no Group Policy here
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap(); // WHAT: a runtime. WHY: to serve.
    let listener = runtime.block_on(tokio::net::TcpListener::bind(config.listen)).unwrap(); // WHAT: a socket. WHY: the address.
    let base = format!("http://{}", listener.local_addr().unwrap()); // WHAT: its address. WHY: every request goes there.
    let app = api::router(Arc::new(Server::new(store, config))); // WHAT: the real server. WHY: under test.
    runtime.spawn(async move {
        let _ = axum::serve(listener, app).await; // WHAT: serving. WHY: over the socket.
    });
    let mut client = hub::Client::new(&base); // WHAT: the program's own client. WHY: signs in as a seat does.
    client.sign_in("sam@example.com", "brace-gusset-purlin-shim-42").unwrap();
    let token = client.token().unwrap().to_string(); // WHAT: the session. WHY: the bearer below.

    let mut license = License {
        id: "evl_sealed01".into(),
        company: "Example Defense Fab".into(),
        edition: Edition::Sealed, // WHAT: the Sealed edition. WHY: under test.
        users: 0,
        updates_through: "2099-01-01".into(),
        issued: "2026-11-10".into(),
        note: String::new(),
        modules: Vec::new(), // WHAT: no Office Server parts. WHY: View alone
        key: "sealed-test".into(),
        signature: String::new(),
    };
    license.signature = hyperview_server::store::hex(&signing.sign(&license.signing_payload()).to_bytes()); // WHAT: signed. WHY: a real licence.
    assert!(!hub::sealed::is_sealed(), "not sealed before the licence");
    let added: Value = ureq::post(&format!("{base}/api/v1/license"))
        .set("Authorization", &format!("Bearer {token}"))
        .send_json(json!({ "license": serde_json::to_string(&license).unwrap() }))
        .expect("the administrator adds it")
        .into_json()
        .unwrap(); // WHAT: added from a seat. WHY: the usual way
    assert_eq!((added["state"].as_str(), added["edition"].as_str()), (Some("licensed"), Some("sealed")), "{added}");
    let seat: hub::license::Standing = serde_json::from_value(added).unwrap(); // WHAT: what a seat reads. WHY: it seals itself from the edition
    assert!(seat.about_line().starts_with("Excalibur View Sealed, licensed to Example Defense Fab."), "{}", seat.about_line());
    assert!(hub::sealed::is_sealed(), "sealed the moment it is added, not at the next restart");
    assert_eq!(hyperview_server::license::renewal_url(&probe), None, "a sealed office never reaches out for its renewal");
    hub::sealed::set_for_testing(hub::sealed::Sealed::No); // WHAT: unsealed by hand, the same licence held. WHY: shows it is the seal, not something else, that stops the look
    assert!(hyperview_server::license::renewal_url(&probe).is_some_and(|u| u.starts_with("https://")), "an Office licence would look for its renewal");
    drop(runtime); // WHAT: the server stopped. WHY: before its folder goes
    let _ = std::fs::remove_dir_all(&folder); // WHAT: the folder removed. WHY: tests leave nothing behind.
}
