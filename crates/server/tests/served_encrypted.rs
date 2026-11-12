//! The whole server started as an office runs it, and reached over HTTPS by
//! its certificate's pin, the way a seat will (docs/OFFICE-SERVER.md, step 4).

use std::io::Read;

use hyperview_server::Config;

/// A server started by `run::serve` with HTTPS on, its data folder, and the
/// plain-HTTP address it answers on.
fn start() -> (std::path::PathBuf, String, tokio::runtime::Runtime) {
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("served-encrypted-{n:016x}")); // WHAT: under the build's own folder. WHY: the server's key is never written where another account shares
    let config = Config {
        name: "Example Steel".into(),
        data: data.clone(),
        listen: "127.0.0.1:0".parse().unwrap(), // WHAT: any free port. WHY: tests run side by side
        tls: true, // WHAT: HTTPS on too, on the API's own port. WHY: the thing under test
        ..Config::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (told, heard) = std::sync::mpsc::channel(); // WHAT: where it landed. WHY: `ready` says so once it listens
    runtime.spawn(async move {
        let _ = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(serving.url());
        })
        .await;
    });
    let base = heard.recv_timeout(std::time::Duration::from_secs(30)).expect("the server started");
    (data, base, runtime)
}

fn health(base: &str) -> serde_json::Value {
    ureq::get(&format!("{base}/api/v1/health")).call().unwrap().into_json().unwrap() // WHAT: the open health answer. WHY: it says where HTTPS is
}

fn ask_encrypted(port: u16, pin: Option<&str>) -> (Result<String, String>, Option<String>) {
    let (agent, check) = hub::pinned::agent(pin).unwrap();
    let answer = agent
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .get(&format!("https://127.0.0.1:{port}/api/v1/health"))
        .call()
        .map_err(|e| e.to_string())
        .and_then(|r| {
            let mut text = String::new();
            r.into_reader().take(64 * 1024).read_to_string(&mut text).map(|_| text).map_err(|e| e.to_string())
        }); // WHAT: the same health answer, encrypted. WHY: the API is the same on both
    (answer, check.seen())
}

#[test]
fn the_api_answers_over_https_by_the_pin_of_the_certificate_it_made_and_keeps() {
    let (data, base, _runtime) = start();
    let said = health(&base);
    let port = u16::try_from(said["secure_port"].as_u64().expect("health says where HTTPS answers")).expect("a port"); // WHAT: the HTTPS port. WHY: a port number always fits
    assert_eq!(Some(port), base.rsplit(':').next().and_then(|p| p.parse().ok()), "HTTPS answers on the API's own port, so no new port is opened anywhere");

    let cert = std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).expect("the certificate was made on start");
    let pin = hyperview_server::tls::pin(&cert).unwrap();
    assert!(hub::pinned::is_pin(&pin));

    let (first, seen) = ask_encrypted(port, None); // WHAT: a seat's first HTTPS connection. WHY: it keeps the pin it sees
    let first: serde_json::Value = serde_json::from_str(&first.expect("answers over HTTPS")).unwrap();
    assert_eq!(first["name"], "Example Steel", "the same API, encrypted");
    assert_eq!(seen.as_deref(), Some(pin.as_str()), "the pin a seat keeps is the certificate the server made");

    let (pinned, _) = ask_encrypted(port, Some(&pin));
    assert!(pinned.is_ok(), "the pinned seat gets in: {pinned:?}");
    let (wrong, shown) = ask_encrypted(port, Some(&"0".repeat(64)));
    assert!(wrong.is_err(), "a seat pinned to another server is refused");
    assert_eq!(shown.as_deref(), Some(pin.as_str()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(data.join(hyperview_server::tls::KEY)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the key is the server's alone");
    }
}

#[test]
fn a_seat_moves_to_https_keeps_the_pin_and_never_falls_back() {
    let (data, base, runtime) = start();
    let n: u64 = rand::random();
    let book_file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("pins-{n:016x}.json")); // WHAT: this seat's pin book. WHY: kept between runs
    let book = hub::secure::PinBook::at(book_file.clone());
    let server = base.trim_start_matches("http://").to_string(); // WHAT: host:port. WHY: how the book names a server

    let first = hub::secure::open(&base, &book).expect("the first connection"); // WHAT: a seat's first time. WHY: it moves to HTTPS and keeps the pin
    assert!(first.base().starts_with("https://127.0.0.1:"), "moved to HTTPS: {}", first.base());
    let cert = std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).unwrap();
    assert_eq!(book.pin(&server), Some(hyperview_server::tls::pin(&cert).unwrap()), "the pin kept is the server's own");
    assert_eq!(first.health().unwrap().name, "Example Steel");

    let again = hub::secure::open(&base, &book).expect("the next connection"); // WHAT: later. WHY: by the kept pin
    assert!(again.base().starts_with("https://"));

    std::fs::write(&book_file, serde_json::to_string(&serde_json::json!({ server.clone(): "0".repeat(64) })).unwrap()).unwrap(); // WHAT: the seat knows another certificate. WHY: as if the server were swapped, or someone were in between
    match hub::secure::open(&base, &book) {
        Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "certificate_changed", "{}", p.message),
        other => panic!("a changed certificate must be refused, got {:?}", other.map(|c| c.base().to_string())),
    }

    drop(runtime); // WHAT: the server gone, HTTPS with it. WHY: a seat holding a pin must not drop to plain HTTP
    assert!(hub::secure::open(&base, &book).is_err(), "never back to plain HTTP once pinned");
}

#[test]
fn this_machine_can_never_take_the_slots_the_office_s_own_computers_need() {
    use std::io::Read;
    let (_data, base, _runtime) = start();
    let address = base.trim_start_matches("http://").to_string(); // WHAT: host:port on 127.0.0.1. WHY: every connection here comes from this machine, as the tunnel connector's do
    let held: Vec<std::net::TcpStream> = (0..448).map(|_| std::net::TcpStream::connect(&address).unwrap()).collect(); // WHAT: this machine's whole share, held open. WHY: a stranger through the tunnel trying to take every slot
    std::thread::sleep(std::time::Duration::from_millis(500)); // WHAT: a moment for the server to take them all. WHY: accepting is asynchronous
    let mut one_more = std::net::TcpStream::connect(&address).unwrap(); // WHAT: the 449th from this machine. WHY: past its share
    one_more.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
    let _ = std::io::Write::write_all(&mut one_more, format!("GET /api/v1/health HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n\r\n").as_bytes()); // WHAT: a real request. WHY: a served connection answers it; a turned-away one can't
    let mut answer = Vec::new();
    let _ = one_more.read_to_end(&mut answer); // WHAT: whatever comes back. WHY: nothing, when turned away
    assert!(!String::from_utf8_lossy(&answer).starts_with("HTTP/1.1"), "the 449th connection from this machine is turned away, not answered");
    drop(held); // WHAT: the share given back. WHY: then this machine is served again
    std::thread::sleep(std::time::Duration::from_millis(500));
    let health: serde_json::Value = ureq::get(&format!("{base}/api/v1/health")).call().unwrap().into_json().unwrap();
    assert_eq!(health["name"], "Example Steel", "served again once the share is free");
}
