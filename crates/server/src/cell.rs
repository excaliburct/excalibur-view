//! The Citadel cell: Citadel Server, run inside the Office Server as a
//! supervised child process (docs/OFFICE-ENROLLMENT.md §6 and §6a).
//!
//! The Office Server starts `citadel\Citadel Server.exe --cell` from its own
//! program folder, under a restricted token, with its data in
//! `ProgramData\Excalibur Office\citadel`. It restarts the cell 5 s after it
//! stops, at most five times in ten minutes; after that it says so in the log
//! and on `/api/v1/office` ("not-answering").
//!
//! The cell keeps no device list of its own. It asks the Office Server over a
//! named pipe that only the system and the cell's own SID may open, one JSON
//! object per line, each answered by exactly one line:
//!
//! - `device`: is this device approved, and with what key?
//! - `hello`: who is the office, and which parts does its licence list?
//! - `migrate`: an old Citadel Server's paired computers, carried over once.
//!
//! What is answered is worked out by [`answer`], a pure function, and the
//! restart rule by [`Restarts`]; the Windows half only moves bytes and starts
//! processes, so the logic is tested on any machine.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The cell's HTTPS port: Citadel's own, so `LatCon Estimating`'s wire is unchanged.
pub const PORT: u16 = crate::citadel::PORT; // WHAT: 47110. WHY: §6

/// The longest line either side sends, its line end aside.
pub const LONGEST_LINE: usize = 4096; // WHAT: 4 KiB. WHY: §6a; a line past it is refused and the connection ended

/// How long after the cell stops it is started again.
pub const RESTART_AFTER: Duration = Duration::from_secs(5); // WHAT: 5 s. WHY: §6a

/// How many restarts are allowed inside [`RESTART_WINDOW`].
pub const MOST_RESTARTS: usize = 5; // WHAT: five. WHY: §6a

/// The window the restarts are counted in.
pub const RESTART_WINDOW: Duration = Duration::from_secs(10 * 60); // WHAT: ten minutes. WHY: §6a

/// The program, beside the Office Server's own.
pub const PROGRAM: &str = "Citadel Server.exe"; // WHAT: the cell's file name. WHY: §6a

/// The folder it is in, inside the Office Server's program folder.
pub const FOLDER: &str = "citadel"; // WHAT: `citadel\`. WHY: §6a

/// The named pipe for an installation: `excalibur-office-<install id>`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn pipe_name(install_id: &str) -> String {
    format!("excalibur-office-{install_id}") // WHAT: the pipe's name, without `\\.\pipe\`. WHY: §6a; the argument the cell is given
}

/// The cell's data folder: `ProgramData\Excalibur Office\citadel`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn data_folder(program_data: &Path) -> PathBuf {
    program_data.join("Excalibur Office").join(FOLDER) // WHAT: the folder. WHY: §6
}

/// The program to start and its arguments, exactly as §6a writes them.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn command(program_folder: &Path, install_id: &str, data: &Path) -> (PathBuf, Vec<String>) {
    let program = program_folder.join(FOLDER).join(PROGRAM); // WHAT: `<office program folder>\citadel\Citadel Server.exe`. WHY: §6a
    let args = vec![
        "--cell".to_string(), // WHAT: run as the cell. WHY: no window, no service, no pairing of its own
        "--pipe".to_string(),
        pipe_name(install_id), // WHAT: which pipe to ask. WHY: §6a
        "--data".to_string(),
        data.display().to_string(), // WHAT: its own data folder. WHY: §6a
        "--port".to_string(),
        PORT.to_string(), // WHAT: 47110. WHY: §6a
    ];
    (program, args)
}

/// One argument quoted the way Windows programs read their command line
/// (the rules `CommandLineToArgvW` and .NET follow).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn quoted(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|c| c == ' ' || c == '\t' || c == '"') {
        return arg.to_string(); // WHAT: nothing to quote. WHY: as it is
    }
    let mut out = String::from('"'); // WHAT: opened. WHY: spaces, tabs or quotes inside
    let mut slashes = 0; // WHAT: backslashes waiting. WHY: they are doubled only before a quote
    for c in arg.chars() {
        match c {
            '\\' => slashes += 1, // WHAT: counted. WHY: written once the next character is known
            '"' => {
                out.push_str(&"\\".repeat(slashes * 2 + 1)); // WHAT: doubled, and one to escape the quote. WHY: the Windows rule
                out.push('"');
                slashes = 0;
            }
            other => {
                out.push_str(&"\\".repeat(slashes)); // WHAT: as they were. WHY: not before a quote
                out.push(other);
                slashes = 0;
            }
        }
    }
    out.push_str(&"\\".repeat(slashes * 2)); // WHAT: doubled before the closing quote. WHY: so it closes
    out.push('"');
    out
}

/// The whole command line, the program first.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn command_line(program: &Path, args: &[String]) -> String {
    let mut line = quoted(&program.display().to_string()); // WHAT: the program, quoted. WHY: its folder has a space in it
    for arg in args {
        line.push(' ');
        line.push_str(&quoted(arg)); // WHAT: each argument, quoted when it needs it. WHY: the data folder has spaces
    }
    line
}

/// Whether the cell should run: the licence lists `citadel`, and the program
/// is there.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn should_start(licensed: &[hub::license::Module], program_is_there: bool) -> bool {
    licensed.contains(&hub::license::Module::Citadel) && program_is_there // WHAT: both. WHY: §6: the cell runs only when the licence lists citadel; and only a build that carries it
}

// ---- the restart rule -------------------------------------------------------------

/// What to do once the cell has stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Next {
    RestartAfter(Duration), // WHAT: start it again after this long. WHY: §6a
    GiveUp, // WHAT: too many in ten minutes. WHY: §6a: logged and shown as not answering
}

/// The restarts made, inside the window.
#[derive(Debug, Default)]
pub struct Restarts {
    made: VecDeque<Instant>, // WHAT: when each restart inside the window was made. WHY: at most five in ten minutes
}

impl Restarts {
    /// The cell stopped at `at`: restart it, or give up.
    pub fn after_exit(&mut self, at: Instant) -> Next {
        while self.made.front().is_some_and(|first| at.saturating_duration_since(*first) >= RESTART_WINDOW) {
            self.made.pop_front(); // WHAT: restarts older than ten minutes forgotten. WHY: the window slides
        }
        if self.made.len() >= MOST_RESTARTS {
            return Next::GiveUp; // WHAT: five already. WHY: §6a
        }
        self.made.push_back(at); // WHAT: this restart counted. WHY: the next exit sees it
        Next::RestartAfter(RESTART_AFTER)
    }
}

// ---- how the cell is, for /api/v1/office ------------------------------------------

/// The cell as the supervisor last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Starting, // WHAT: started, not yet said hello. WHY: shown as looking
    Running { version: String }, // WHAT: said hello with its version. WHY: shown as running
    NotAnswering, // WHAT: given up after too many restarts, or couldn't be started. WHY: §6a
    PipeTaken, // WHAT: another program holds the cell's pipe name. WHY: security review L4: an alarm, never a quiet restart
}

/// The supervisor's last word on the cell. Empty when there is no cell to
/// supervise (no licence for it, no program, or not Windows).
#[derive(Default)]
pub struct Watch {
    state: Mutex<Option<State>>, // WHAT: the state, or none when not supervised. WHY: /office falls back to the look at Citadel's port then
}

impl Watch {
    /// Notes how the cell is.
    pub fn set(&self, state: State) {
        if let Ok(mut slot) = self.state.lock() {
            *slot = Some(state); // WHAT: kept. WHY: read by /office
        }
    }

    /// Whether a cell is supervised here at all.
    pub fn supervised(&self) -> bool {
        self.state.lock().is_ok_and(|s| s.is_some()) // WHAT: a state was ever set. WHY: set only by the supervisor
    }

    /// How the cell is, if it is supervised.
    pub fn get(&self) -> Option<State> {
        self.state.lock().ok().and_then(|s| s.clone()) // WHAT: a copy. WHY: read by /office
    }

    /// The cell as `/api/v1/office` reports Citadel: `None` while it starts,
    /// or when it isn't supervised, so the last look at its port is used.
    pub fn seen(&self) -> Option<crate::citadel::Seen> {
        match self.get()? {
            State::Starting => None, // WHAT: no word yet. WHY: the look at its port, or "looking"
            State::Running { version } => Some(crate::citadel::Seen::Running { version }), // WHAT: running, its version. WHY: Fleet compares it with the shelf
            State::NotAnswering => Some(crate::citadel::Seen::NotAnswering), // WHAT: not answering. WHY: §6a
            State::PipeTaken => Some(crate::citadel::Seen::PipeTaken), // WHAT: the pipe's name taken. WHY: shown as an alarm
        }
    }
}

/// A process that can be started and waited for.
pub trait Launcher {
    /// Starts the cell.
    ///
    /// # Errors
    ///
    /// When it can't be started; counted as a stop.
    fn start(&mut self) -> std::io::Result<()>;

    /// Waits until the started cell stops.
    fn wait(&mut self);
}

/// Runs the cell until the restart rule gives up: started, waited for, and
/// started again 5 s after each stop. `sleep` and `now` are the clock, so a
/// test runs it in no time.
pub fn supervise(launcher: &mut dyn Launcher, watch: &Watch, sleep: &dyn Fn(Duration), now: &dyn Fn() -> Instant) {
    let mut restarts = Restarts::default(); // WHAT: none yet. WHY: counted from the first stop
    loop {
        watch.set(State::Starting); // WHAT: starting. WHY: shown until it says hello
        match launcher.start() {
            Ok(()) => launcher.wait(), // WHAT: run until it stops. WHY: then the rule decides
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::error!("ALARM: another program holds the Citadel cell's pipe ({e}); the cell is not started until the Office Server restarts"); // WHAT: written in the log as an alarm. WHY: something on this machine is pretending to be the Office Server to the cell
                watch.set(State::PipeTaken); // WHAT: shown on /api/v1/office. WHY: an admin and Fleet see it
                return;
            }
            Err(e) => tracing::warn!("the Citadel cell could not be started: {e}"), // WHAT: said. WHY: counted as a stop
        }
        match restarts.after_exit(now()) {
            Next::RestartAfter(wait) => {
                tracing::warn!("the Citadel cell stopped; starting it again in {} s", wait.as_secs()); // WHAT: said. WHY: an admin reading the log sees it
                sleep(wait); // WHAT: 5 s. WHY: §6a
            }
            Next::GiveUp => {
                tracing::error!("the Citadel cell stopped {MOST_RESTARTS} times in ten minutes; it is not being started again until the Office Server restarts"); // WHAT: written in the log. WHY: §6a
                watch.set(State::NotAnswering); // WHAT: shown on /api/v1/office. WHY: §6a
                return;
            }
        }
    }
}

// ---- the pipe's messages ---------------------------------------------------------------

/// What the cell may ask the Office Server.
pub trait Office {
    /// `{"op":"device","id":...}`'s answer.
    fn device(&self, id: &str) -> Value;
    /// `{"op":"hello","version":...}`'s answer.
    fn hello(&self, version: &str) -> Value;
    /// `{"op":"migrate","devices":[...]}`'s answer.
    fn migrate(&self, devices: &[crate::devices::Paired]) -> Value;
}

/// A refusal line.
fn refusal(error: &str) -> Value {
    json!({ "ok": false, "error": error }) // WHAT: ok false and why. WHY: each request gets exactly one answer line, even a bad one
}

/// The one answer line for one request line (without its line end). Never
/// longer than [`LONGEST_LINE`].
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn answer(line: &[u8], office: &dyn Office) -> String {
    let value = if line.len() > LONGEST_LINE {
        refusal("too_long") // WHAT: refused. WHY: §6a: at most 4 KiB a line
    } else {
        match serde_json::from_slice::<Value>(line) {
            Ok(Value::Object(asked)) => match asked.get("op").and_then(Value::as_str) {
                Some("device") => match asked.get("id").and_then(Value::as_str) {
                    Some(id) => office.device(id), // WHAT: how this device stands. WHY: §6a
                    None => refusal("no_id"), // WHAT: no id. WHY: nothing to look up
                },
                Some("hello") => office.hello(asked.get("version").and_then(Value::as_str).unwrap_or_default()), // WHAT: the office and its parts. WHY: §6a
                Some("migrate") => match asked.get("devices").cloned().map(serde_json::from_value::<Vec<crate::devices::Paired>>) {
                    Some(Ok(devices)) => office.migrate(&devices), // WHAT: the paired computers carried over. WHY: §6a
                    _ => refusal("bad_devices"), // WHAT: a list that doesn't read. WHY: nothing is half-carried
                },
                _ => refusal("unknown_op"), // WHAT: anything else. WHY: answered, never ignored
            },
            _ => refusal("not_json"), // WHAT: not a JSON object. WHY: likewise
        }
    };
    let text = value.to_string(); // WHAT: one line of JSON. WHY: serde_json never writes a raw line break
    if text.len() > LONGEST_LINE {
        return refusal("answer_too_long").to_string(); // WHAT: never more than the cell reads. WHY: §6a's limit holds both ways
    }
    text
}

/// Serves one connection: each line read is answered with one line, until
/// the cell closes it, or sends a line longer than [`LONGEST_LINE`] (which
/// is answered, and the connection ended).
///
/// # Errors
///
/// When the pipe fails.
pub fn serve_lines(reader: &mut dyn BufRead, writer: &mut dyn Write, office: &dyn Office) -> std::io::Result<()> {
    loop {
        let mut line = Vec::new(); // WHAT: one request. WHY: read up to its line end
        let read = std::io::Read::take(&mut *reader, (LONGEST_LINE + 2) as u64).read_until(b'\n', &mut line)?; // WHAT: at most 4 KiB and its line end. WHY: a cell sending without end can't fill memory
        if read == 0 {
            return Ok(()); // WHAT: closed. WHY: the cell stopped
        }
        let ended = line.last() == Some(&b'\n'); // WHAT: whether the whole line came. WHY: one that didn't is too long, or cut off
        while matches!(line.last(), Some(b'\n' | b'\r')) {
            line.pop(); // WHAT: the line end taken off. WHY: not part of the JSON
        }
        let reply = if ended { answer(&line, office) } else { refusal("too_long").to_string() }; // WHAT: the answer, or a refusal for a line that never ended. WHY: §6a
        writer.write_all(reply.as_bytes())?; // WHAT: the answer line. WHY: exactly one per request
        writer.write_all(b"\n")?;
        writer.flush()?; // WHAT: sent now. WHY: the cell waits at most 2 s
        if !ended {
            return Ok(()); // WHAT: the connection ended. WHY: what follows a too-long line can't be trusted to start a new one
        }
    }
}

/// The Office Server answering the cell, for one connection.
pub struct Answering<'a> {
    pub server: &'a crate::api::Server, // WHAT: the server. WHY: its device list, its name and its licence
    pub migrate_open: std::sync::atomic::AtomicBool, // WHAT: whether this connection may migrate. WHY: only the first cell's first connection, after its first hello
}

impl<'a> Answering<'a> {
    /// The answers for a new connection from the cell.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn new(server: &'a crate::api::Server) -> Answering<'a> {
        Answering { server, migrate_open: std::sync::atomic::AtomicBool::new(false) } // WHAT: closed until the first hello ever opens it. WHY: security review M1
    }
}

impl Office for Answering<'_> {
    fn device(&self, id: &str) -> Value {
        crate::devices::for_the_cell(&self.server.store, id) // WHAT: the device's state, and its key when approved. WHY: §6a
    }

    fn hello(&self, version: &str) -> Value {
        let version = version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')).take(32).collect::<String>(); // WHAT: a plain version, at most 32 characters. WHY: shown on /api/v1/office as it is
        self.server.cell.set(State::Running { version }); // WHAT: running, its version. WHY: /api/v1/office reports it
        if crate::devices::first_hello(&self.server.store).unwrap_or(false) {
            self.migrate_open.store(true, std::sync::atomic::Ordering::SeqCst); // WHAT: this connection may migrate. WHY: it is the first cell's first, over an old Citadel Server's data
        }
        let modules: Vec<&str> = crate::office::licensed(self.server).iter().map(|m| m.name()).collect(); // WHAT: the licence's parts. WHY: §6a
        json!({ "ok": true, "office": crate::api::installation_name(self.server), "modules": modules })
    }

    fn migrate(&self, devices: &[crate::devices::Paired]) -> Value {
        if !self.migrate_open.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return refusal("migrate_sealed"); // WHAT: refused. WHY: security review M1: only the first connection after the first hello ever, and only once in it
        }
        match crate::devices::from_citadel(&self.server.store, devices) {
            Ok(Some(added)) => json!({ "ok": true, "added": added }), // WHAT: how many came in. WHY: §6a
            Ok(None) => refusal("already_migrated"), // WHAT: done before. WHY: §6a: sent once
            Err(e) => {
                tracing::error!("the Citadel cell's paired computers could not be carried over: {e}"); // WHAT: said. WHY: the detail is for the log
                refusal("server_error")
            }
        }
    }
}

// ---- the cell's identity and folders ----------------------------------------------------------

/// The cell's own SID: an `S-1-5-80` (service) SID made from this
/// installation's id, so no other account on the computer has it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn cell_sid(install_id: &str) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(format!("excalibur-office-citadel-cell/1\n{install_id}").as_bytes()); // WHAT: a digest of the label and the id. WHY: one SID per installation, the same every start
    let parts: Vec<String> = digest[..20]
        .chunks(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]).to_string())
        .collect(); // WHAT: five 32-bit sub-authorities. WHY: the shape of a service SID
    format!("S-1-5-80-{}", parts.join("-"))
}

/// The pipe's permissions, as SDDL: the system and the cell's SID, nobody
/// else, and nothing inherited.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn pipe_sddl(cell_sid: &str) -> String {
    format!("D:P(A;;GA;;;SY)(A;;0x12008b;;;{cell_sid})") // WHAT: SYSTEM full; the cell reads and writes data only (FILE_GENERIC_READ and FILE_WRITE_DATA), never FILE_CREATE_PIPE_INSTANCE. WHY: §6a: "SYSTEM, plus the cell's own SID. Nobody else may connect"; and the cell can never make an instance of the pipe itself
}

/// The `icacls` runs that make the cell's data folder its own: nothing
/// inherited, the system (which the cell's restricted token is made from,
/// and needs for the token's first check) and the cell's SID.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn data_steps(folder: &Path, cell_sid: &str) -> Vec<Vec<std::ffi::OsString>> {
    let whole = folder.as_os_str().to_os_string(); // WHAT: the folder. WHY: every step is on it
    let below = folder.join("*").into_os_string(); // WHAT: what is inside. WHY: reset to inherit
    let cell = format!("*{cell_sid}:(OI)(CI)M"); // WHAT: the cell may read, write and delete its own data. WHY: a blind store keeps its files here
    let os = |a: &str| std::ffi::OsString::from(a); // WHAT: one argument. WHY: icacls takes them as they are
    vec![
        vec![whole.clone(), os("/reset"), os("/C"), os("/Q")], // WHAT: its own entries cleared. WHY: as the service's folder
        vec![whole.clone(), os("/inheritance:r"), os("/grant:r"), os("*S-1-5-18:(OI)(CI)F"), os(&cell), os("/C"), os("/Q")], // WHAT: the system and the cell only. WHY: §6: ACL'd to the cell
        vec![below, os("/reset"), os("/T"), os("/C"), os("/Q")], // WHAT: everything inside inherits. WHY: an entry written on a file goes
        vec![whole, os("/setowner"), os("*S-1-5-18"), os("/T"), os("/C"), os("/Q")], // WHAT: the system owns it. WHY: nobody keeps an owner's right to change it back
    ]
}

/// The `icacls` runs that let the cell read and run its own program folder:
/// the system and administrators as for the service, and the cell's SID to
/// read and run. Run after the service's folder is locked, which resets it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn program_steps(folder: &Path, cell_sid: &str) -> Vec<Vec<std::ffi::OsString>> {
    let whole = folder.as_os_str().to_os_string(); // WHAT: `citadel\`. WHY: the cell's program
    let os = |a: &str| std::ffi::OsString::from(a); // WHAT: one argument. WHY: as above
    let cell = format!("*{cell_sid}:(OI)(CI)RX"); // WHAT: read and run. WHY: never change its own program
    vec![vec![whole, os("/grant:r"), os(&cell), os("/C"), os("/Q")]] // WHAT: added to what it inherits from the locked service folder. WHY: the system and administrators keep theirs
}

/// The SIDs the cell's restricted token is checked against a second time:
/// its own, and the groups every program needs to load Windows' own files.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn restricting_sids(cell_sid: &str) -> Vec<String> {
    vec![
        cell_sid.to_string(), // WHAT: its own SID. WHY: its pipe, its data and its program grant it
        "S-1-1-0".to_string(), // WHAT: Everyone. WHY: Windows' files and the .NET runtime grant it
        "S-1-5-32-545".to_string(), // WHAT: Users. WHY: likewise
        "S-1-5-11".to_string(), // WHAT: Authenticated Users. WHY: likewise
    ] // WHAT: never Administrators or SYSTEM. WHY: the Office Server's own data grants only those, so the cell can't reach it
}

#[cfg(windows)]
pub mod windows;

/// Starts supervising the cell, when the licence lists it and its program is
/// there. Windows only; elsewhere there is no cell.
pub fn start(server: &crate::api::Shared) {
    #[cfg(windows)]
    windows::start(server.clone()); // WHAT: the supervisor, with its own handle on the server. WHY: it runs on its own thread
    #[cfg(not(windows))]
    let _ = server; // WHAT: nothing to start. WHY: the cell is a Windows program
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// An office that answers from fixed values, and notes what it was asked.
    struct Fixed {
        asked: RefCell<Vec<String>>,
    }

    impl Office for Fixed {
        fn device(&self, id: &str) -> Value {
            self.asked.borrow_mut().push(format!("device {id}"));
            if id == "a".repeat(32) {
                json!({"state":"approved","public_key":"k","admin":false,"name":"Dana","machine":"DANA-LAPTOP","app":"latcon-estimating"})
            } else {
                json!({"state":"unknown"})
            }
        }
        fn hello(&self, version: &str) -> Value {
            self.asked.borrow_mut().push(format!("hello {version}"));
            json!({"ok":true,"office":"LatCon","modules":["view","citadel"]})
        }
        fn migrate(&self, devices: &[crate::devices::Paired]) -> Value {
            self.asked.borrow_mut().push(format!("migrate {}", devices.len()));
            json!({"ok":true,"added":devices.len()})
        }
    }

    fn fixed() -> Fixed {
        Fixed { asked: RefCell::new(Vec::new()) }
    }

    #[test]
    fn each_message_is_answered_as_the_contract_says() {
        let office = fixed();
        let id = "a".repeat(32);
        let approved: Value = serde_json::from_str(&answer(format!(r#"{{"op":"device","id":"{id}"}}"#).as_bytes(), &office)).unwrap();
        assert_eq!(approved["state"], "approved");
        assert_eq!(approved["machine"], "DANA-LAPTOP");
        let unknown: Value = serde_json::from_str(&answer(br#"{"op":"device","id":"bbbb"}"#, &office)).unwrap();
        assert_eq!(unknown, json!({"state":"unknown"}));
        let hello: Value = serde_json::from_str(&answer(br#"{"op":"hello","version":"1.1.0"}"#, &office)).unwrap();
        assert_eq!(hello["ok"], true);
        assert_eq!(hello["modules"], json!(["view", "citadel"]));
        let migrated: Value = serde_json::from_str(&answer(br#"{"op":"migrate","devices":[{"public_key":"x","name":"Dana","machine":"PC","paired_at":"2026-01-01T00:00:00Z","paired_by":"Mark"}]}"#, &office)).unwrap();
        assert_eq!(migrated, json!({"ok":true,"added":1}));
        assert_eq!(*office.asked.borrow(), vec![format!("device {id}"), "device bbbb".to_string(), "hello 1.1.0".to_string(), "migrate 1".to_string()]);
    }

    #[test]
    fn anything_else_gets_one_refusal_line() {
        let office = fixed();
        for bad in [&b"not json"[..], b"[1,2]", br#"{"op":"delete"}"#, br#"{"op":"device"}"#, br#"{"op":"migrate","devices":7}"#] {
            let said: Value = serde_json::from_str(&answer(bad, &office)).unwrap();
            assert_eq!(said["ok"], false, "{}", String::from_utf8_lossy(bad));
        }
        let long = format!(r#"{{"op":"hello","version":"{}"}}"#, "1".repeat(LONGEST_LINE));
        let said: Value = serde_json::from_str(&answer(long.as_bytes(), &office)).unwrap();
        assert_eq!(said["error"], "too_long");
        assert!(office.asked.borrow().is_empty(), "nothing bad reached the office");
    }

    #[test]
    fn a_connection_answers_every_line_once_and_ends_on_a_line_too_long() {
        let office = fixed();
        let id = "a".repeat(32);
        let sent = format!("{{\"op\":\"device\",\"id\":\"{id}\"}}\r\n{{\"op\":\"hello\",\"version\":\"1.1.0\"}}\n{}\n{{\"op\":\"hello\"}}\n", "x".repeat(LONGEST_LINE + 10));
        let mut reader = std::io::Cursor::new(sent.into_bytes());
        let mut written = Vec::new();
        serve_lines(&mut reader, &mut written, &office).unwrap();
        let lines: Vec<&str> = std::str::from_utf8(&written).unwrap().lines().collect();
        assert_eq!(lines.len(), 3, "two answers and the refusal, then the connection ended: {lines:?}");
        assert!(lines[0].contains("approved"));
        assert!(lines[1].contains("\"ok\":true"));
        assert!(lines[2].contains("too_long"));
        assert_eq!(office.asked.borrow().len(), 2, "the line after the long one was never read");
    }

    #[test]
    fn the_cell_is_started_exactly_as_the_contract_writes_it() {
        let (program, args) = command(Path::new(r"C:\Program Files\Excalibur Office"), "inst_1", Path::new(r"C:\ProgramData\Excalibur Office\citadel"));
        let line = command_line(&program, &args);
        assert!(line.starts_with('"') && line.contains("Citadel Server.exe\""), "{line}");
        assert!(line.ends_with(r#" --cell --pipe excalibur-office-inst_1 --data "C:\ProgramData\Excalibur Office\citadel" --port 47110"#), "{line}");
        assert_eq!(quoted(r"a\b"), r"a\b");
        assert_eq!(quoted(r#"a "b""#), r#""a \"b\"""#);
        assert_eq!(quoted(r"C:\a b\"), r#""C:\a b\\""#);
        assert_eq!(quoted(""), r#""""#);
    }

    #[test]
    fn the_cell_runs_only_with_the_licence_and_the_program() {
        use hub::license::Module;
        assert!(should_start(&[Module::View, Module::Citadel], true));
        assert!(!should_start(&[Module::View], true), "not licensed");
        assert!(!should_start(&[Module::View, Module::Citadel], false), "no program");
    }

    #[test]
    fn restarts_after_five_seconds_at_most_five_times_in_ten_minutes() {
        let mut restarts = Restarts::default();
        let start = Instant::now();
        for i in 0..5 {
            assert_eq!(restarts.after_exit(start + Duration::from_secs(i * 10)), Next::RestartAfter(RESTART_AFTER), "restart {i}");
        }
        assert_eq!(restarts.after_exit(start + Duration::from_secs(60)), Next::GiveUp, "the sixth stop inside ten minutes");
        let mut later = Restarts::default();
        for i in 0..5 {
            assert_eq!(later.after_exit(start + Duration::from_secs(i * 200)), Next::RestartAfter(RESTART_AFTER));
        }
        assert_eq!(later.after_exit(start + Duration::from_secs(1000)), Next::RestartAfter(RESTART_AFTER), "the first restart has left the window");
    }

    /// A cell that stops as soon as it starts.
    struct Stopping {
        starts: usize,
    }

    impl Launcher for Stopping {
        fn start(&mut self) -> std::io::Result<()> {
            self.starts += 1;
            Ok(())
        }
        fn wait(&mut self) {}
    }

    /// A pipe name somebody else holds.
    struct Squatted {
        starts: usize,
    }

    impl Launcher for Squatted {
        fn start(&mut self) -> std::io::Result<()> {
            self.starts += 1;
            Err(std::io::Error::new(std::io::ErrorKind::AddrInUse, "taken"))
        }
        fn wait(&mut self) {}
    }

    #[test]
    fn a_taken_pipe_name_is_an_alarm_and_never_a_restart() {
        let watch = Watch::default();
        let mut cell = Squatted { starts: 0 };
        let start = Instant::now();
        supervise(&mut cell, &watch, &|_| {}, &|| start);
        assert_eq!(cell.starts, 1, "never tried again on a name someone else holds");
        assert_eq!(watch.get(), Some(State::PipeTaken));
        assert_eq!(watch.seen().map(|s| s.state()), Some("pipe-taken"));
    }

    #[test]
    fn a_cell_that_keeps_stopping_is_given_up_on_and_shown_not_answering() {
        let watch = Watch::default();
        let mut cell = Stopping { starts: 0 };
        let slept = RefCell::new(Vec::new());
        let start = Instant::now();
        supervise(&mut cell, &watch, &|d| slept.borrow_mut().push(d), &|| start);
        assert_eq!(cell.starts, 1 + MOST_RESTARTS, "the first start and five restarts");
        assert!(slept.borrow().iter().all(|d| *d == RESTART_AFTER));
        assert_eq!(watch.get(), Some(State::NotAnswering));
        assert_eq!(watch.seen().map(|s| s.state()), Some("not-answering"));
    }

    #[test]
    fn the_pipe_and_the_data_are_the_system_and_the_cell_alone() {
        let sid = cell_sid("inst_1");
        assert!(sid.starts_with("S-1-5-80-") && sid.split('-').count() == 9, "S, 1, 5, 80 and five sub-authorities: {sid}");
        assert_eq!(sid, cell_sid("inst_1"), "the same every start");
        assert_ne!(sid, cell_sid("inst_2"), "another installation, another SID");
        assert_eq!(pipe_sddl(&sid), format!("D:P(A;;GA;;;SY)(A;;0x12008b;;;{sid})"), "the cell may read and write, never make an instance");
        let steps = data_steps(Path::new(r"C:\ProgramData\Excalibur Office\citadel"), &sid);
        let grant: Vec<String> = steps[1].iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(grant.contains(&"/inheritance:r".to_string()));
        assert_eq!(grant.iter().filter(|a| a.starts_with('*')).count(), 2, "the system and the cell, nobody else: {grant:?}");
        assert!(!restricting_sids(&sid).iter().any(|s| s == "S-1-5-18" || s == "S-1-5-32-544"), "never the system or administrators");
    }
}
