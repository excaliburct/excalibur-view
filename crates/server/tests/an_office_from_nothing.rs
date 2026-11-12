//! One office, from an empty data folder to two linked computers, the way
//! a real office's will go: Mark's computer claims the server, a second PC asks and
//! Mark approves it, it reads a project, Mark removes it and it is turned
//! away; the claiming window has closed a day on; the Citadel cell is
//! answered for both PCs; and an old Citadel Server's data moves in before
//! the cell's first start.

use std::sync::Arc;

use hub::device::{DeviceKey, Signer};
use hyperview_server::cell::Office as _; // WHAT: the cell's questions. WHY: hello and device, asked below
use hyperview_server::{api, Config, Store};

struct Office {
    data: std::path::PathBuf, // WHAT: its data folder. WHY: empty at the start
    port: u16, // WHAT: its port. WHY: every call goes there
    pin: String, // WHAT: its certificate's pin. WHY: proofs and signatures are bound to it
    _runtime: tokio::runtime::Runtime, // WHAT: the server's runtime. WHY: kept alive for the test
}

fn start() -> Office {
    let n: u64 = rand::random(); // WHAT: a random folder name. WHY: tests run side by side
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("from-nothing-{n:016x}")); // WHAT: a folder that doesn't exist yet. WHY: a fresh install
    assert!(!data.exists(), "an empty start"); // WHAT: nothing there. WHY: a fresh install
    let config = Config { name: "LatCon".into(), data: data.clone(), listen: "127.0.0.1:0".parse().unwrap(), tls: true, devices: hyperview_server::config::Devices::Required, ..Config::default() }; // WHAT: the office server as LatCon runs it: HTTPS, devices required. WHY: the shipped settings
    let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap(); // WHAT: the server's own runtime. WHY: the client calls block
    let (told, heard) = std::sync::mpsc::channel(); // WHAT: a channel for the port. WHY: told once it listens
    runtime.spawn(async move { // WHAT: the server, running. WHY: until the test ends
        let _ = hyperview_server::run::serve(config, std::future::pending(), move |serving| {
            let _ = told.send(serving.address.port());
        })
        .await; // WHAT: served. WHY: until the runtime is dropped
    });
    let port = heard.recv_timeout(std::time::Duration::from_secs(30)).expect("the server started"); // WHAT: the port it took. WHY: every call goes there
    let cert = std::fs::read_to_string(data.join(hyperview_server::tls::CERT)).expect("its certificate, made at its first start"); // WHAT: its certificate. WHY: made at the first start
    let pin = hyperview_server::tls::pin(&cert).unwrap(); // WHAT: its pin. WHY: what a computer pins
    Office { data, port, pin, _runtime: runtime } // WHAT: the running office. WHY: for the test
}

impl Office {
    fn store(&self) -> Store {
        let config = Config { data: self.data.clone(), ..Config::default() }; // WHAT: the same data folder. WHY: the same database
        Store::open(&config.database(), &config.blobs()).unwrap() // WHAT: the database. WHY: read and set beside the server
    }

    fn client(&self, key: &Arc<Signer>) -> hub::Client {
        hub::Client::pinned(&format!("https://127.0.0.1:{}", self.port), &self.pin).unwrap().with_device(key.clone()) // WHAT: a computer's client, by the pin. WHY: as the app connects
    }
}

fn computer(seed: u8) -> Arc<Signer> {
    Arc::new(Signer::new(DeviceKey::from_secret(&[seed; 32]))) // WHAT: a computer's key. WHY: one per PC
}

#[test]
fn an_office_from_an_empty_folder_to_two_linked_computers_and_the_citadel_cell() {
    let office = start(); // WHAT: a fresh office. WHY: from nothing

    // Mark's computer is the first to ask: it claims the server as its admin.
    let mark = computer(1); // WHAT: Mark's computer. WHY: the first to ask
    let mut marks = office.client(&mark); // WHAT: its client. WHY: signs every call
    assert_eq!(marks.ask_to_be_let_in("view", "Mark").unwrap().unwrap().state, "approved", "the first local computer is the admin"); // WHAT: approved at once. WHY: the first local computer claims the server
    assert!(marks.device_me().unwrap().admin); // WHAT: and an admin. WHY: it decides the rest
    let claimed = marks.claim(&hub::Setup { company: "LatCon".into(), name: "Mark".into(), email: "mark@latcon.test".into(), password: ["girder", "plate", "weld", "shim"].join("-") }).unwrap(); // WHAT: the office's first account. WHY: from Mark's approved computer
    marks.create_project("2661", "Akron Awning").unwrap(); // WHAT: a project. WHY: for the second PC to read

    // A second PC asks, waits, and Mark approves it from his computer.
    let dana = computer(2); // WHAT: a second PC. WHY: Dana's
    let mut danas = office.client(&dana); // WHAT: its client. WHY: likewise
    assert_eq!(danas.ask_to_be_let_in("view", "Dana").unwrap().unwrap().state, "waiting", "an admin decides now"); // WHAT: waiting. WHY: an admin now decides
    assert_eq!(danas.device_me().unwrap().state, "waiting"); // WHAT: told it is waiting. WHY: the waiting screen
    let waiting = marks.devices().unwrap(); // WHAT: the device list, as Mark sees it. WHY: the Computers page
    assert!(waiting.iter().any(|d| d.id == dana.key().id() && d.state == "waiting"), "Mark sees Dana waiting"); // WHAT: Dana waiting there. WHY: Mark sees who asks
    marks.approve_device(&dana.key().id(), false).unwrap(); // WHAT: approved by Mark. WHY: from his computer
    assert_eq!(danas.device_me().unwrap().state, "approved"); // WHAT: told it is approved. WHY: the waiting screen moves on

    // The second PC joins with the office's code and reads the project.
    danas.join(&hub::Join { code: claimed.join_code.clone(), name: "Dana".into(), email: "dana@latcon.test".into(), password: ["camber", "weld", "joist", "plate"].join("-") }).unwrap(); // WHAT: Dana's account, by the office's code. WHY: a person on the approved PC
    assert_eq!(danas.projects().unwrap().iter().map(|p| p.name.clone()).collect::<Vec<_>>(), vec!["Akron Awning".to_string()], "the second PC reads the project"); // WHAT: the project read. WHY: an approved PC reaches the office's work

    // Mark removes it: it is turned away at once.
    marks.remove_device(&dana.key().id()).unwrap(); // WHAT: removed by Mark. WHY: the PC is gone
    match danas.projects() { // WHAT: its next call. WHY: turned away at once
        Err(hub::client::Trouble::Refused(p)) => assert_eq!(p.error, "device_unknown", "401 device_unknown"), // WHAT: 401 device_unknown. WHY: §4's one answer
        other => panic!("a removed computer still read the project: {:?}", other.is_ok()), // WHAT: anything else fails. WHY: a removed PC reads nothing
    }

    // The Citadel cell asks about both PCs over its pipe.
    let config = Config { data: office.data.clone(), ..Config::default() }; // WHAT: the same data folder. WHY: the cell reads the same device list
    let server = api::Server::new(office.store(), config); // WHAT: a server on the same data. WHY: the cell's answers come from it
    let cell = hyperview_server::cell::Answering::new(&server); // WHAT: a fake cell's connection. WHY: the pipe's answers, without Windows
    assert_eq!(cell.hello("1.1.0")["ok"], true); // WHAT: hello answered. WHY: the cell's first line
    assert_eq!(cell.device(&mark.key().id())["state"], "approved", "Mark's computer, approved, with its key"); // WHAT: Mark's PC approved. WHY: the cell lets it in
    assert_eq!(cell.device(&mark.key().id())["public_key"], mark.key().public_key_text()); // WHAT: with its key. WHY: the cell checks its signatures
    assert_eq!(cell.device(&dana.key().id())["state"], "removed", "the removed PC is refused by the cell"); // WHAT: Dana's PC removed. WHY: the cell turns it away

    // A day on, the claiming window is shut: a fresh office's first ask waits.
    let later = start(); // WHAT: a second fresh office. WHY: its window untouched
    let until = later.store().setting("device_claim_until").unwrap().expect("the window is kept"); // WHAT: its claiming window. WHY: set at the first start
    let closes = time::OffsetDateTime::parse(&until, &time::format_description::well_known::Rfc3339).unwrap(); // WHAT: read as a time. WHY: compared below
    let in_a_day = time::OffsetDateTime::now_utc() + time::Duration::hours(24); // WHAT: a day from now. WHY: the window's length
    assert!((closes - in_a_day).whole_seconds().abs() < 120, "the window is 24 hours from the first start: {until}"); // WHAT: 24 hours, give or take. WHY: §3
    later.store().set_setting("device_claim_until", &(time::OffsetDateTime::now_utc() - time::Duration::seconds(1)).format(&time::format_description::well_known::Rfc3339).unwrap()).unwrap(); // WHAT: the clock moved past the window. WHY: 24 hours later
    let stranger = computer(9); // WHAT: a stranger's computer. WHY: the first to ask after the window
    assert_eq!(later.client(&stranger).ask_to_be_let_in("view", "Someone").unwrap().unwrap().state, "waiting", "after 24 hours nobody claims the server"); // WHAT: waiting, not admin. WHY: nobody claims the server after 24 hours
}

/// A 1.0 service that is there, and stops.
struct OldService;

impl hyperview_server::cellmove::OldService for OldService {
    fn present(&self) -> bool {
        true // WHAT: installed. WHY: LatCon's 1.0 server
    }
    fn stop_and_remove(&self) -> Result<(), String> {
        Ok(()) // WHAT: stopped and uninstalled. WHY: before the move
    }
}

#[cfg(unix)]
#[test]
fn an_old_citadel_server_s_data_moves_in_before_the_cell_s_first_start() {
    use std::os::unix::fs::PermissionsExt; // WHAT: file permissions. WHY: the fake program runs
    let office = start(); // WHAT: a fresh office. WHY: its data folder
    let old = office.data.join("Citadel Server"); // WHAT: the 1.0 folder's place. WHY: made below
    std::fs::create_dir_all(&old).unwrap(); // WHAT: a 1.0 data folder. WHY: there to move
    let program = office.data.join("move.sh"); // WHAT: the fake program's place. WHY: written below
    std::fs::write(&program, "#!/bin/sh\necho '{\"result\":\"moved\",\"exit\":0,\"files\":37,\"kept_as\":\"Citadel Server.moved\"}'\nexit 0\n").unwrap(); // WHAT: a fake --move-from. WHY: the .NET side's answer for a good move
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap(); // WHAT: runnable. WHY: it is run as the move
    let store = office.store(); // WHAT: the database. WHY: the move's record is kept there
    assert!(hyperview_server::cellmove::before_first_start(&store, &OldService, &hyperview_server::cellmove::Run, &program, &old, &office.data.join("cell")), "moved: the cell may start"); // WHAT: moved, so the cell may start. WHY: the move's own answer
    assert_eq!(hyperview_server::cellmove::recorded(&store).unwrap().words(), "moved 37 files; the old folder is kept as Citadel Server.moved"); // WHAT: its words, from the JSON line. WHY: shown on the office page
    let unused = office.data.join("never.sh"); // WHAT: a program that isn't there. WHY: it must never be run again
    assert!(hyperview_server::cellmove::before_first_start(&store, &OldService, &hyperview_server::cellmove::Run, &unused, &old, &office.data.join("cell")), "never run again after a success"); // WHAT: not run again. WHY: a recorded success holds
}
