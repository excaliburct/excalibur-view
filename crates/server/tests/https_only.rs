//! The HTTPS-only switch (audit M5): with `HYPERVIEW_HTTPS_ONLY=on`, another computer's plain HTTP is turned away with
//! a reason and never read, HTTPS still answers, and this machine itself (the tunnel connector) may still use plain HTTP.

use std::io::{Read, Write};

use hyperview_server::Config;

/// A server started as an office runs it, on every address, with the switch as given; the address another computer
/// would use (this machine's network address, not loopback) and the port.
fn start(https_only: Result<bool, String>, tls: bool) -> (Result<(std::net::IpAddr, u16), String>, tokio::runtime::Runtime) {
    let n: u64 = rand::random();
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("https-only-{n:016x}")); // WHAT: under the build's own folder. WHY: the server's key is never written where another account shares
    let config = Config {
        name: "Example Steel".into(),
        data,
        listen: "0.0.0.0:0".parse().unwrap(), // WHAT: every address, any free port. WHY: reached from this machine's network address, as another computer would
        tls,
        https_only,
        ..Config::default()
    };
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    let (told, heard) = std::sync::mpsc::channel(); // WHAT: where it landed, or why it didn't start. WHY: read below
    let failed = told.clone();
    runtime.spawn(async move {
        let result = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(Ok(serving.address.port()));
        })
        .await;
        if let Err(e) = result {
            let _ = failed.send(Err(format!("{e:#}")));
        } // WHAT: a refusal to start. WHY: the switch's own checks
    });
    let landed = heard.recv_timeout(std::time::Duration::from_secs(30)).expect("the server started or said why not");
    let lan = hyperview_server::run::local_address().expect("this machine has a network address to test from"); // WHAT: not loopback. WHY: the server must see another computer
    (landed.map(|port| (lan, port)), runtime)
}

/// One plain-HTTP request; the whole answer as text.
fn plain(ip: std::net::IpAddr, port: u16) -> String {
    let mut tcp = std::net::TcpStream::connect((ip, port)).unwrap();
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    tcp.write_all(b"GET /api/v1/health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n").unwrap();
    let mut answer = String::new();
    let _ = tcp.take(64 * 1024).read_to_string(&mut answer); // WHAT: all it says. WHY: compared below
    answer
}

fn encrypted(ip: std::net::IpAddr, port: u16) -> Result<String, String> {
    let (agent, _) = hub::pinned::agent(None).unwrap();
    let host = match ip {
        std::net::IpAddr::V6(v6) => format!("[{v6}]"),
        std::net::IpAddr::V4(v4) => v4.to_string(),
    };
    agent
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .get(&format!("https://{host}:{port}/api/v1/health"))
        .call()
        .map_err(|e| e.to_string())
        .and_then(|r| r.into_string().map_err(|e| e.to_string()))
}

#[test]
fn with_the_switch_on_another_computers_plain_http_is_turned_away_and_https_answers() {
    let (started, _runtime) = start(Ok(true), true);
    let (lan, port) = started.expect("it starts");
    let refused = plain(lan, port);
    assert!(refused.starts_with("HTTP/1.1 426"), "plain HTTP from another address is refused: {refused}");
    assert!(refused.contains("HTTPS only") && !refused.contains("Example Steel"), "with a reason, and nothing of the server's: {refused}"); // WHAT: no answer from the API. WHY: nothing it sent was read
    assert!(encrypted(lan, port).is_ok_and(|a| a.contains("Example Steel")), "HTTPS still answers");
    let here = plain(std::net::IpAddr::from([127, 0, 0, 1]), port);
    assert!(here.starts_with("HTTP/1.1 200"), "this machine itself may still use plain HTTP: {here}"); // WHAT: the tunnel connector's way in. WHY: it never leaves the box
}

#[test]
fn with_the_switch_off_plain_http_still_answers_everyone() {
    let (started, _runtime) = start(Ok(false), true);
    let (lan, port) = started.expect("it starts");
    assert!(plain(lan, port).starts_with("HTTP/1.1 200"), "seats older than 0.7.0 still connect"); // WHAT: the default. WHY: nothing changes until the office turns it on
}

#[test]
fn a_switch_that_does_not_read_or_cannot_work_stops_the_start() {
    let (typo, _a) = start(Err("yes".into()), true);
    assert!(typo.is_err_and(|e| e.contains("HYPERVIEW_HTTPS_ONLY") && e.contains("on or off")), "a typo is refused, never guessed");
    let (no_tls, _b) = start(Ok(true), false);
    assert!(no_tls.is_err_and(|e| e.contains("HYPERVIEW_TLS")), "HTTPS-only with HTTPS off would turn everyone away");
    assert_eq!(hyperview_server::config::https_only_from(Some(" ON ")), Ok(true));
    assert_eq!(hyperview_server::config::https_only_from(None), Ok(false));
    assert_eq!(hyperview_server::config::https_only_from(Some("true")), Err("true".into()));
}
