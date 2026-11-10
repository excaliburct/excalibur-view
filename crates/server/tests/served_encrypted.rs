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
        secure_listen: Some("127.0.0.1:0".parse().unwrap()), // WHAT: HTTPS on too, on any free port. WHY: the thing under test
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
    let port = said["secure_port"].as_u64().expect("health says where HTTPS answers") as u16;

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
