//! The one thing the whole six-seat plan rides on: a seat finding the server
//! without anybody typing an address.
//!
//! The instruction this was built to was plain — put one thing on the server,
//! run it, and the other computers connect. Everything else in this program
//! can be worked around by typing a URL; this cannot, because the whole point
//! is that nobody has to. It is also exactly the sort of thing that breaks
//! silently: a refactor moves a port, or stops spawning a thread, and every
//! other test still passes while the office quietly stops finding its own
//! server.
//!
//! # Why this is one long test rather than six short ones
//!
//! The announcing socket is a single fixed port held for the life of the
//! process, which is right for a server and awkward for a test binary: only
//! the first server in the process can have it, and the rest would be testing
//! the first one's answers. So there is one server here and one test, told in
//! order, and the ordering is the point — the middle of it is a server being
//! set up *while somebody is looking at it*, which is exactly what happens on
//! the first morning.

use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use hyperview_server::{run, Config, Store};

/// A scratch directory that cleans up after itself.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n: u64 = rand::random();
        let path = std::env::temp_dir().join(format!("hyperview-found-{n:016x}"));
        std::fs::create_dir_all(&path).expect("a scratch directory");
        Scratch(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Looks for a server by name the way a seat does, with a few goes at it —
/// this is a UDP broadcast on a loaded test machine, and one dropped packet
/// is not a failure.
fn look_for(name: &str) -> Option<hub::discover::Found> {
    for _ in 0..4 {
        let found = hub::discover::look(Duration::from_millis(1500));
        if let Some(hit) = found.into_iter().find(|f| f.answer.name == name) {
            return Some(hit);
        }
    }
    None
}

#[test]
fn a_seat_finds_the_office_server_without_being_told_where_it_is() {
    // WHAT: is the asking port free on this computer? WHY: the server only answers seats when it holds the port; something
    // else holding it (the HP's pen-test copies hold it on purpose, so they never announce themselves) leaves nothing to test.
    if let Err(e) = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], hub::discover::PORT))) {
        if e.kind() == std::io::ErrorKind::AddrInUse {
            eprintln!(
                "skipped: UDP port {} is held by another program on this computer, so no server here can answer seats",
                hub::discover::PORT
            ); // WHAT: said, not failed. WHY: the computer, not the code, stops it; anywhere the port is free it runs in full.
            return;
        }
    } // WHAT: the probe socket closed at once. WHY: the server under test binds the port next.
    let home = Scratch::new();
    let config = Config {
        name: "Example Steel".into(),
        data: home.0.clone(),
        // Port zero: the operating system picks one, so this never collides
        // with a real server or with another test run.
        listen: "127.0.0.1:0".parse().expect("an address"),
        ..Config::default()
    };
    let database = config.database();
    let blobs = config.blobs();

    let (stop, stopping) = tokio::sync::oneshot::channel();
    let (up, is_up) = std::sync::mpsc::channel();
    let serving = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a runtime");
        runtime.block_on(async move {
            let _ = run::serve(
                config,
                async {
                    let _ = stopping.await;
                },
                move |serving| {
                    let _ = up.send(serving.address.port());
                },
            )
            .await;
        });
    });
    let port = is_up
        .recv_timeout(Duration::from_secs(30))
        .expect("the server came up");
    // The announcing thread is spawned just before the server is declared
    // ready; give it the moment it needs to get hold of its socket.
    std::thread::sleep(Duration::from_millis(500));

    // ---- it is found at all, with everything a seat needs ------------------

    let found = look_for("Example Steel").expect("the server was not found on the network");
    assert!(
        found.answer.url.starts_with("http://"),
        "no address for a seat to talk to: {:?}",
        found.answer
    );
    assert!(
        found.answer.url.ends_with(&format!(":{port}")),
        "it gave out the wrong port: {:?} (it is serving on {port})",
        found.answer
    );
    assert!(
        !found.answer.id.trim().is_empty(),
        "nothing to tell two servers in one building apart by"
    );
    assert_eq!(
        found.answer.api_version,
        hub::API_VERSION,
        "a seat has to know whether it can talk to this at all"
    );
    assert!(
        !found.answer.version.trim().is_empty(),
        "a seat has to be able to see what the server is running"
    );

    // ---- with nobody on it, it says so -------------------------------------
    //
    // This is what lets the first person in the office set the thing up from
    // Hyperview itself: no address to type, no password to be handed over.
    assert!(
        !found.answer.ready,
        "a server with nobody on it must not claim to be set up: {:?}",
        found.answer
    );

    // ---- set up while somebody is looking at it ----------------------------

    {
        let store = Store::open(&database, &blobs).expect("the same store");
        run::first_administrator(&store).expect("an administrator");
    }
    let now = look_for("Example Steel").expect("still found after being set up");
    assert!(
        now.answer.ready,
        "it is set up now and has to stop advertising itself as free: {:?}",
        now.answer
    );
    assert_eq!(now.answer.id, found.answer.id, "it is the same server");

    // ---- it answers one question and ignores everything else ---------------
    //
    // The discovery socket is open to the whole local network. It is not a
    // service that can be talked into saying anything about the drawings.
    let socket = UdpSocket::bind("0.0.0.0:0").expect("a socket");
    socket
        .set_read_timeout(Some(Duration::from_millis(600)))
        .expect("a timeout");
    for rubbish in [
        &b"GET / HTTP/1.1"[..],
        &b"HYPERVIEW-FIND-9"[..],
        &b"hyperview-find-1"[..],
        &b""[..],
        &[0xff; 200][..],
    ] {
        let _ = socket.send_to(
            rubbish,
            SocketAddr::from(([127, 0, 0, 1], hub::discover::PORT)),
        );
        let mut back = [0u8; 1024];
        assert!(
            socket.recv_from(&mut back).is_err(),
            "it answered something that was not a seat asking: {rubbish:?}"
        );
    }

    // And it is still there afterwards, so none of that knocked it over.
    assert!(
        look_for("Example Steel").is_some(),
        "the announcer stopped working after being sent rubbish"
    );

    let _ = stop.send(());
    let _ = serving.join();
}
