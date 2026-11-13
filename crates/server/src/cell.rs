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

/// How long the supervisor rests after giving up before it tries again.
pub const GIVE_UP_REST: Duration = Duration::from_secs(10 * 60); // WHAT: ten minutes. WHY: review: a cell given up on recovers by itself, shown as not answering meanwhile

/// Runs the cell: started, waited for, and started again 5 s after each
/// stop; after five restarts in ten minutes it is shown as not answering,
/// rested ten minutes, and tried again, `rounds` times in all (a real server
/// passes `usize::MAX`). `sleep` and `now` are the clock, so a test runs it
/// in no time. A pipe someone else holds stops it for good, as an alarm.
pub fn supervise(launcher: &mut dyn Launcher, watch: &Watch, sleep: &dyn Fn(Duration), now: &dyn Fn() -> Instant, rounds: usize) {
    let mut restarts = Restarts::default(); // WHAT: none yet. WHY: counted from the first stop
    let mut round = 1; // WHAT: the first round. WHY: counted against `rounds`
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
                tracing::error!("ALARM: the Citadel cell stopped {MOST_RESTARTS} times in ten minutes; it is tried again in ten minutes"); // WHAT: written in the log. WHY: §6a
                watch.set(State::NotAnswering); // WHAT: shown on /api/v1/office. WHY: §6a
                if round >= rounds {
                    return; // WHAT: the rounds asked for are done. WHY: a test's end; a real server never gets here
                }
                round += 1; // WHAT: the next round. WHY: counted
                sleep(GIVE_UP_REST); // WHAT: ten minutes' rest. WHY: whatever stopped it may have passed
                restarts = Restarts::default(); // WHAT: a fresh count. WHY: a new round of five
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
    pub migrate_open: std::sync::atomic::AtomicBool, // WHAT: whether this connection may migrate. WHY: only after its hello, and only until a migrate has succeeded
    reader: Option<std::sync::Mutex<rusqlite::Connection>>, // WHAT: this connection's own read-only handle on the database. WHY: review: a device check never waits on the server's one shared connection, so it answers well inside the cell's 2 s
}

impl<'a> Answering<'a> {
    /// The answers for a new connection from the cell.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn new(server: &'a crate::api::Server) -> Answering<'a> {
        let reader = rusqlite::Connection::open_with_flags(server.config.database(), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .ok()
            .inspect(|c| {
                let _ = c.busy_timeout(Duration::from_millis(500)); // WHAT: at most half a second waiting on a writer. WHY: well inside the cell's 2 s
            })
            .map(std::sync::Mutex::new); // WHAT: a read-only handle of its own. WHY: the server's shared connection may be busy with a big write
        Answering { server, migrate_open: std::sync::atomic::AtomicBool::new(false), reader } // WHAT: closed until a hello opens it. WHY: security review M1
    }
}

impl Office for Answering<'_> {
    fn device(&self, id: &str) -> Value {
        match self.reader.as_ref().and_then(|r| r.lock().ok()) {
            Some(db) => crate::devices::for_the_cell_from(&db, id), // WHAT: from this connection's own read-only handle. WHY: never behind the server's shared connection
            None => crate::devices::for_the_cell(&self.server.store, id), // WHAT: the shared one, when no handle of its own could be opened. WHY: still answered, failing closed
        }
    }

    fn hello(&self, version: &str) -> Value {
        let version = version.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')).take(32).collect::<String>(); // WHAT: a plain version, at most 32 characters. WHY: shown on /api/v1/office as it is
        self.server.cell.set(State::Running { version }); // WHAT: running, its version. WHY: /api/v1/office reports it
        if crate::devices::migration_pending(&self.server.store).unwrap_or(false) {
            self.migrate_open.store(true, std::sync::atomic::Ordering::SeqCst); // WHAT: this connection may migrate, after its hello. WHY: no migrate has succeeded yet; a successful one, empty or not, seals it for good
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

/// The cell's data folder's permissions, as SDDL: nothing inherited, the
/// system in full (the cell's restricted token is made from it, and needs it
/// for the token's first check) and the cell's SID to read, write and
/// delete, both passed on to everything inside. Set through Windows' own
/// call, never icacls: the cell's SID names no account, and icacls refused
/// every grant to it on a real Windows server (the end-to-end test, 2026-11-12).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn data_sddl(cell_sid: &str) -> String {
    format!("D:P(A;OICI;FA;;;SY)(A;OICI;0x1301bf;;;{cell_sid})") // WHAT: protected; SYSTEM full; the cell's SID "modify" (0x1301bf). WHY: §6: ACL'd to the cell
}

/// The `icacls` runs after [`data_sddl`] is set: everything inside made to
/// inherit it, and the system made the owner. Neither names the cell's SID.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn data_steps(folder: &Path) -> Vec<Vec<std::ffi::OsString>> {
    let whole = folder.as_os_str().to_os_string(); // WHAT: the folder. WHY: the owner is set on it
    let below = folder.join("*").into_os_string(); // WHAT: what is inside. WHY: reset to inherit
    let os = |a: &str| std::ffi::OsString::from(a); // WHAT: one argument. WHY: icacls takes them as they are
    vec![
        vec![below, os("/reset"), os("/T"), os("/C"), os("/Q")], // WHAT: everything inside inherits. WHY: an entry written on a moved file goes
        vec![whole, os("/setowner"), os("*S-1-5-18"), os("/T"), os("/C"), os("/Q")], // WHAT: the system owns it. WHY: nobody keeps an owner's right to change it back
    ]
}

/// The cell's program folder's own entry, as SDDL: the cell's SID may read
/// and run (0x1200a9), added to what the folder inherits from the locked
/// service folder (the system and administrators). Set through Windows' own
/// call, as [`data_sddl`].
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn program_sddl(cell_sid: &str) -> String {
    format!("D:(A;OICI;0x1200a9;;;{cell_sid})") // WHAT: not protected, so the inherited entries stay; the cell reads and runs. WHY: never change its own program
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

/// The cell's own inbound firewall rule's name.
pub const FIREWALL_RULE: &str = "Excalibur Office Server (Citadel cell)"; // WHAT: the rule's name. WHY: found, replaced and removed by it

/// Citadel Server 1.0's firewall rule, taken away after a good move.
pub const OLD_FIREWALL_RULE: &str = "Citadel Server"; // WHAT: the name 1.0's installer gave it (WindowsSetup.FirewallRule). WHY: it opened 47110 for 1.0's program, which is gone

/// Citadel Server 1.0's nightly update task, taken away after a good move.
pub const OLD_UPDATE_TASK: &str = "Citadel Server updates"; // WHAT: the task 1.0 made, 03:30 as the system (ServerUpdateCheck.TaskName). WHY: it would try to update a program that no longer runs

/// The `netsh` arguments that let the office network reach the cell on
/// 47110: TCP, for the cell's own program only, on every profile and from
/// the local subnet only, as the Office Server's own 8714 rule.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn firewall_rule(program: &Path) -> Vec<String> {
    vec![
        "advfirewall".into(),
        "firewall".into(),
        "add".into(),
        "rule".into(),
        format!("name={FIREWALL_RULE}"), // WHAT: its name. WHY: found again by it
        "dir=in".into(), // WHAT: inbound. WHY: the estimating computers connect to it
        "action=allow".into(), // WHAT: let in. WHY: nothing else opens 47110
        "protocol=TCP".into(), // WHAT: TCP. WHY: the cell's HTTPS
        format!("localport={PORT}"), // WHAT: 47110. WHY: §6
        format!("program={}", program.display()), // WHAT: the cell's own program only. WHY: no other program may answer on the port through this rule
        "profile=any".into(), // WHAT: every network profile. WHY: as 8714: a new network is "Public" until someone says otherwise
        "remoteip=localsubnet".into(), // WHAT: the office network only. WHY: as 8714
    ]
}

/// The `netsh` arguments that ask whether the cell's rule is there.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn firewall_rule_shown() -> Vec<String> {
    vec!["advfirewall".into(), "firewall".into(), "show".into(), "rule".into(), format!("name={FIREWALL_RULE}")] // WHAT: shown by name. WHY: netsh exits 1 when there is no such rule
}

/// The steps that take away 1.0's leftovers after a good move, each as a
/// program in `System32` and its arguments: its update task and its firewall
/// rule. The service itself is uninstalled through the service manager.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn old_leftover_steps() -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("schtasks.exe", vec!["/Delete".into(), "/F".into(), "/TN".into(), OLD_UPDATE_TASK.into()]), // WHAT: the nightly update task gone. WHY: high 5
        ("netsh.exe", vec!["advfirewall".into(), "firewall".into(), "delete".into(), "rule".into(), format!("name={OLD_FIREWALL_RULE}")]), // WHAT: 1.0's firewall rule gone. WHY: critical 2: only the cell's own rule opens 47110
    ]
}

/// The cell's settings file, inside its data folder (Citadel's
/// `ServerPaths.Settings`).
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn settings_file(data: &Path) -> PathBuf {
    data.join("state").join("settings.json") // WHAT: `state\settings.json`. WHY: where Citadel keeps its port, its copy-to folder and Fleet's key
}

/// The cell's settings, as JSON, with Fleet's maintenance key made the
/// Office Server's own: its SHA-256 (lower-case hex, as Citadel's
/// `ServerState.HashKey` makes it), or none when the Office Server has no
/// key. Every other setting is kept as it was. `text` is the file as it is,
/// if it reads as an object.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn settings_with_office_key(text: Option<&str>, key: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    let mut settings = text.and_then(|t| serde_json::from_str::<serde_json::Map<String, Value>>(t).ok()).unwrap_or_default(); // WHAT: the settings as they are, or none. WHY: only the key is changed
    match key {
        Some(key) => {
            settings.insert("fleetKeySha256".into(), Value::String(hub::update::hex(&Sha256::digest(key.as_bytes())))); // WHAT: the Office Server's key, as Citadel keeps one. WHY: only the Office Server's maintenance key opens the cell's admin routes
        }
        None => {
            settings.remove("fleetKeySha256"); // WHAT: no key at all. WHY: a key set on 1.0 never opens the cell; Citadel then answers 503 not configured
        }
    }
    serde_json::to_string_pretty(&Value::Object(settings)).unwrap_or_else(|_| "{}".into()) // WHAT: written as Citadel writes it. WHY: it reads it strictly
}

/// Writes the Office Server's maintenance key into the cell's settings,
/// before every start of the cell, the file and Citadel's `.previous` copy of
/// it alike, so a key carried over from 1.0 never opens the cell's admin
/// routes on 47110.
///
/// The cell can change its own data folder, and this runs as the system, so
/// nothing here follows a link: the data folder and `state\` must be plain
/// folders (no symbolic link, no junction or other reparse point), each held
/// open while the files are written so it can't be swapped (Windows: without
/// delete sharing, checked on that handle); the two settings files must be
/// plain files or absent; each new copy is made beside them with
/// `create_new` (never through anything already there) and renamed over the
/// old one, which replaces the name itself and never writes through a link.
/// The .NET cell doesn't read the key over its pipe, so the file is the way.
///
/// # Errors
///
/// When anything on the way is a link (the cell must then not be started),
/// or a file can't be written.
pub fn give_the_cell_the_office_key(data: &Path, key: Option<&str>) -> std::io::Result<()> {
    let state = data.join("state"); // WHAT: `state\`. WHY: the settings are in it
    let _data_held = plain_folder(data)?; // WHAT: the data folder, checked and held. WHY: never written through a link the cell made
    match std::fs::symlink_metadata(&state) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::create_dir(&state)?, // WHAT: made, not followed. WHY: a fresh cell has none yet
        _ => {} // WHAT: there already. WHY: checked next
    }
    let _state_held = plain_folder(&state)?; // WHAT: `state\`, checked and held. WHY: review: the cell could swap it for a junction
    let file = settings_file(data); // WHAT: the settings. WHY: rewritten
    let previous = state.join("settings.json.previous"); // WHAT: Citadel's fallback copy (SafeFile.Previous). WHY: read when the file itself doesn't; it must never hold the old key
    let mut there = Vec::new(); // WHAT: which of the two exist. WHY: only those are rewritten, the file always
    for path in [&file, &previous] {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if !plain_file(&meta) => return Err(refused(path)), // WHAT: a link or anything but a plain file. WHY: never read or written through
            Ok(_) => there.push(path.clone()), // WHAT: a plain file. WHY: read and rewritten
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // WHAT: absent. WHY: the file is written new; no fallback copy is made
            Err(e) => return Err(e), // WHAT: can't be looked at. WHY: never guessed
        }
    }
    let read = |p: &Path| std::fs::read_to_string(p).ok().filter(|t| serde_json::from_str::<serde_json::Map<String, Value>>(t).is_ok()); // WHAT: a copy that reads as settings. WHY: the base for the new one
    let base = there.iter().find_map(|p| read(p)); // WHAT: the settings as they are, the file first. WHY: everything but the key kept
    let text = settings_with_office_key(base.as_deref(), key); // WHAT: with the office's key. WHY: as above
    for path in std::iter::once(&file).chain(there.iter().filter(|p| **p == previous)) {
        let part = state.join(format!("settings.office-{}-{}", std::process::id(), rand::random::<u32>())); // WHAT: a new name beside it, this run's own. WHY: create_new never opens what is already there
        write_new(&part, text.as_bytes())?; // WHAT: written whole, new. WHY: never through a link
        if let Err(e) = std::fs::rename(&part, path) {
            let _ = std::fs::remove_file(&part); // WHAT: the copy gone. WHY: never a stray file
            return Err(e); // WHAT: said. WHY: the cell isn't started
        }
    }
    Ok(())
}

/// The refusal for a link where a plain folder or file must be.
fn refused(path: &Path) -> std::io::Error {
    std::io::Error::other(format!("{} is a link or junction, not a plain folder or file, so the cell's settings were not written and the cell was not started", path.display())) // WHAT: said. WHY: the alarm names it
}

/// Whether metadata (never followed) is a plain file: not a link, and on
/// Windows not any reparse point.
fn plain_file(meta: &std::fs::Metadata) -> bool {
    meta.file_type().is_file() && !reparse(meta) // WHAT: a regular file only. WHY: a link is never read or written as the system
}

/// Whether metadata carries a reparse point (a junction, a mount point or a
/// link of any kind). Never on Unix, where links are their own file type.
fn reparse(meta: &std::fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _; // WHAT: Windows' file attributes. WHY: a junction isn't always a "symlink" to std
        meta.file_attributes() & 0x400 != 0 // WHAT: FILE_ATTRIBUTE_REPARSE_POINT. WHY: any kind of link
    }
    #[cfg(not(windows))]
    {
        let _ = meta; // WHAT: unused here. WHY: a Unix link is its own file type
        false
    }
}

/// Checks that `folder` is a plain folder and holds it open while the caller
/// writes in it. On Windows it is opened without following a reparse point,
/// checked on that same handle, and held without delete sharing, so it can't
/// be renamed or swapped until the handle is dropped.
///
/// # Errors
///
/// When it is a link or not a folder.
fn plain_folder(folder: &Path) -> std::io::Result<Option<std::fs::File>> {
    let meta = std::fs::symlink_metadata(folder)?; // WHAT: never followed. WHY: a link is refused, not used
    if !meta.file_type().is_dir() || reparse(&meta) {
        return Err(refused(folder)); // WHAT: refused. WHY: review: SYSTEM must never write where a cell's link points
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _; // WHAT: Windows' open options. WHY: flags and sharing
        let held = std::fs::OpenOptions::new()
            .access_mode(0x0080) // WHAT: FILE_READ_ATTRIBUTES. WHY: enough to hold and check it
            .share_mode(0x0000_0001 | 0x0000_0002) // WHAT: read and write sharing, never delete. WHY: nobody can rename or remove it while held
            .custom_flags(0x0200_0000 | 0x0020_0000) // WHAT: FILE_FLAG_BACKUP_SEMANTICS and FILE_FLAG_OPEN_REPARSE_POINT. WHY: a folder, and the link itself if it is one, never its target
            .open(folder)?; // WHAT: opened. WHY: checked and held
        let on_handle = held.metadata()?; // WHAT: the attributes of what was opened. WHY: no gap between the check and the write
        if !on_handle.is_dir() || reparse(&on_handle) {
            return Err(refused(folder)); // WHAT: refused. WHY: swapped between the two looks
        }
        Ok(Some(held))
    }
    #[cfg(not(windows))]
    {
        Ok(None) // WHAT: checked; nothing held. WHY: the cell runs only on Windows
    }
}

/// Writes `bytes` to a file that must not exist yet, never through a link.
fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write as _; // WHAT: the file's write. WHY: below
    let mut options = std::fs::OpenOptions::new(); // WHAT: how it is opened. WHY: new only
    options.write(true).create_new(true); // WHAT: a new file, never one already there. WHY: a planted link fails here
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt as _; // WHAT: Windows' open options. WHY: the flag below
        options.custom_flags(0x0020_0000); // WHAT: FILE_FLAG_OPEN_REPARSE_POINT. WHY: never through a link, even one made in the moment between
    }
    let mut file = options.open(path)?; // WHAT: made. WHY: written next
    if reparse(&file.metadata()?) {
        drop(file); // WHAT: closed. WHY: removed next
        let _ = std::fs::remove_file(path); // WHAT: gone. WHY: never left
        return Err(refused(path)); // WHAT: refused. WHY: checked on the handle written through
    }
    file.write_all(bytes)?; // WHAT: the settings. WHY: the new copy
    file.sync_all() // WHAT: on the disk. WHY: before it is renamed into place
}

/// Where a self-extracting .NET program unpacks itself, inside the
/// service's own folder (locked to the system and administrators): one
/// folder for the move, run as the system, and one for the cell, which the
/// cell's SID may also change. Never `C:\Windows\Temp`, where any account can
/// plant a DLL for it to load.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn extract_folder(home: &Path, for_the_cell: bool) -> PathBuf {
    home.join("dotnet-extract").join(if for_the_cell { "cell" } else { "move" }) // WHAT: the two folders. WHY: review low 3
}

/// The variable that points .NET's single-file bundle at that folder.
pub const EXTRACT_VARIABLE: &str = "DOTNET_BUNDLE_EXTRACT_BASE_DIR"; // WHAT: .NET's own setting. WHY: harmless when the bundle doesn't extract

/// The cell's unpacking folder's own entry, as SDDL: the cell's SID may
/// change it (0x1301bf), added to what it inherits from the locked service
/// folder. Set through Windows' own call, as [`data_sddl`].
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn extract_sddl(cell_sid: &str) -> String {
    format!("D:(A;OICI;0x1301bf;;;{cell_sid})") // WHAT: not protected; the cell unpacks there. WHY: nobody else but the system and administrators can
}

/// A Windows environment block (`name=value` pairs, each ended by a zero,
/// and a zero after the last) from this process's own environment, with
/// `name` set to `value` in place of any it had.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn environment_block(vars: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>, name: &str, value: &std::ffi::OsStr) -> Vec<std::ffi::OsString> {
    let mut out: Vec<std::ffi::OsString> = vars.filter(|(k, _)| !k.to_string_lossy().eq_ignore_ascii_case(name)).map(|(mut k, v)| {
        k.push("="); // WHAT: name=. WHY: the block's form
        k.push(v); // WHAT: its value. WHY: likewise
        k
    }).collect(); // WHAT: every variable but `name`. WHY: Windows names are matched without case, so an older one never wins
    let mut set = std::ffi::OsString::from(name); // WHAT: the variable. WHY: added
    set.push("="); // WHAT: name=. WHY: the block's form
    set.push(value); // WHAT: its value. WHY: the folder
    out.push(set); // WHAT: added. WHY: the cell sees it
    out // WHAT: one entry each; the caller ends each and the block with zeros. WHY: as CreateProcess takes it
}

/// Ends the running cell before an update, waiting up to ten seconds;
/// whether it is gone. True where there is no cell.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn end_for_update() -> bool {
    #[cfg(windows)]
    {
        windows::end_the_cell(Duration::from_secs(10)) // WHAT: the cell's job ended and waited for. WHY: review low 1: its folder is swapped at the next start
    }
    #[cfg(not(windows))]
    {
        true // WHAT: no cell. WHY: the cell is a Windows program
    }
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

    #[test]
    fn the_cell_unpacks_into_its_own_folder_never_windows_temp() {
        let home = Path::new(r"C:\ProgramData\Excalibur Hyperview\Server"); // WHAT: the service's folder. WHY: locked to the system and administrators
        assert!(super::extract_folder(home, true).starts_with(home) && super::extract_folder(home, false).starts_with(home), "inside the service's folder"); // WHAT: inside. WHY: review low 3
        assert_ne!(super::extract_folder(home, true), super::extract_folder(home, false), "the cell's and the move's apart"); // WHAT: apart. WHY: only the cell's grants the cell
        let vars = vec![("Path".into(), "C:\\Windows".into()), ("dotnet_bundle_extract_base_dir".into(), r"C:\Windows\Temp".into())].into_iter(); // WHAT: an environment that already names Windows' temp, in other case. WHY: never kept
        let block = super::environment_block(vars, super::EXTRACT_VARIABLE, std::ffi::OsStr::new(r"C:\x\cell")); // WHAT: the block. WHY: passed to CreateProcess
        let texts: Vec<String> = block.iter().map(|v| v.to_string_lossy().into_owned()).collect(); // WHAT: as text. WHY: compared
        assert!(texts.contains(&format!(r"{}=C:\x\cell", super::EXTRACT_VARIABLE)), "{texts:?}"); // WHAT: set. WHY: the folder
        assert!(!texts.iter().any(|t| t.to_ascii_lowercase().contains("windows\\temp")), "the old value is gone: {texts:?}"); // WHAT: replaced. WHY: never Windows' temp
        assert!(texts.iter().any(|t| t.starts_with("Path=")), "everything else kept"); // WHAT: kept. WHY: the cell needs its environment
        assert_eq!(super::extract_sddl("S-1-5-80-1"), "D:(A;OICI;0x1301bf;;;S-1-5-80-1)", "the cell may use its own, beside what it inherits"); // WHAT: granted. WHY: it unpacks there
    }

    #[test]
    fn a_maintenance_key_from_1_0_never_opens_the_cell() {
        let old = r#"{"port":47110,"copyTo":"D:\\Backups","fleetKeySha256":"1111111111111111111111111111111111111111111111111111111111111111","minFreeBytes":5}"#; // WHAT: 1.0's settings, moved in, with 1.0's key. WHY: the case asked about
        let mine: serde_json::Value = serde_json::from_str(&super::settings_with_office_key(Some(old), Some("office-key"))).unwrap(); // WHAT: rewritten with the Office Server's key. WHY: the cell's admin routes open only to it
        let want = { use sha2::Digest; hub::update::hex(&sha2::Sha256::digest(b"office-key")) }; // WHAT: the SHA-256 of the office's key. WHY: as Citadel keeps one
        assert_eq!(mine["fleetKeySha256"], want.as_str(), "the office's key"); // WHAT: the office's. WHY: never 1.0's
        assert_eq!(mine["port"], 47110); // WHAT: kept. WHY: only the key changes
        assert_eq!(mine["copyTo"], r"D:\Backups"); // WHAT: kept. WHY: likewise
        let none: serde_json::Value = serde_json::from_str(&super::settings_with_office_key(Some(old), None)).unwrap(); // WHAT: an Office Server with no key. WHY: then the cell has none
        assert!(none.get("fleetKeySha256").is_none(), "1.0's key removed"); // WHAT: gone. WHY: never 1.0's
    }

    #[test]
    fn the_office_key_is_written_to_the_cell_s_settings_and_its_fallback_copy() {
        let n: u64 = rand::random(); // WHAT: a fresh name. WHY: tests run side by side
        let data = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("cellkey-{n:016x}")); // WHAT: a cell's data folder. WHY: never a shared temporary folder
        std::fs::create_dir_all(data.join("state")).unwrap(); // WHAT: its state folder. WHY: the settings are there
        let old = r#"{"port":47110,"fleetKeySha256":"1111111111111111111111111111111111111111111111111111111111111111"}"#; // WHAT: 1.0's settings. WHY: moved in
        std::fs::write(super::settings_file(&data), old).unwrap(); // WHAT: the file. WHY: rewritten
        std::fs::write(data.join("state").join("settings.json.previous"), old).unwrap(); // WHAT: its fallback copy. WHY: must not keep 1.0's key either
        super::give_the_cell_the_office_key(&data, Some("office-key")).unwrap(); // WHAT: rewritten. WHY: before the cell starts
        for name in ["settings.json", "settings.json.previous"] {
            let text = std::fs::read_to_string(data.join("state").join(name)).unwrap(); // WHAT: each copy. WHY: both are read by Citadel
            assert!(!text.contains("1111111111"), "{name} no longer holds 1.0's key"); // WHAT: gone. WHY: only the office's key
        }
        let _ = std::fs::remove_dir_all(&data); // WHAT: tidied. WHY: a test leaves nothing
    }

    #[test]
    fn the_cell_s_firewall_rule_is_tcp_47110_for_its_program_on_the_office_network_only() {
        let rule = super::firewall_rule(Path::new(r"C:\ProgramData\Excalibur Hyperview\Server\citadel\Citadel Server.exe")); // WHAT: the rule for the installed cell. WHY: as added at install and at the cell's first start
        for word in ["dir=in", "action=allow", "protocol=TCP", "localport=47110", "profile=any", "remoteip=localsubnet", r"program=C:\ProgramData\Excalibur Hyperview\Server\citadel\Citadel Server.exe"] {
            assert!(rule.iter().any(|a| a == word), "{word} in {rule:?}"); // WHAT: each part. WHY: the same scope as 8714, and for the cell's program only
        }
        assert!(super::firewall_rule_shown().last().unwrap().ends_with(super::FIREWALL_RULE)); // WHAT: asked by the same name. WHY: added only when missing
    }

    #[test]
    fn the_1_0_leftovers_are_its_update_task_and_its_firewall_rule() {
        let steps = super::old_leftover_steps(); // WHAT: the steps. WHY: run after a good move
        assert!(steps.iter().any(|(tool, args)| *tool == "schtasks.exe" && args.iter().any(|a| a == "Citadel Server updates")), "the 03:30 task"); // WHAT: the task. WHY: high 5
        assert!(steps.iter().any(|(tool, args)| *tool == "netsh.exe" && args.iter().any(|a| a == "name=Citadel Server") && args.iter().any(|a| a == "delete")), "1.0's rule"); // WHAT: the rule. WHY: likewise
    }
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
        supervise(&mut cell, &watch, &|_| {}, &|| start, 3);
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
        supervise(&mut cell, &watch, &|d| slept.borrow_mut().push(d), &|| start, 2);
        assert_eq!(cell.starts, 2 * (1 + MOST_RESTARTS), "the first start and five restarts, then again after a rest");
        assert_eq!(slept.borrow().iter().filter(|d| **d == GIVE_UP_REST).count(), 1, "rested ten minutes between the rounds");
        assert!(slept.borrow().iter().all(|d| *d == RESTART_AFTER || *d == GIVE_UP_REST));
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
        assert_eq!(data_sddl(&sid), format!("D:P(A;OICI;FA;;;SY)(A;OICI;0x1301bf;;;{sid})"), "protected: the system and the cell, nobody else"); // WHAT: the data folder's entries. WHY: §6
        assert_eq!(program_sddl(&sid), format!("D:(A;OICI;0x1200a9;;;{sid})"), "the cell reads and runs its program, beside what the folder inherits"); // WHAT: the program folder's entry. WHY: never change its own program
        let steps = data_steps(Path::new(r"C:\ProgramData\Excalibur Office\citadel")); // WHAT: icacls after the entries are set. WHY: checked below
        assert!(steps.iter().flatten().all(|a| !a.to_string_lossy().contains(&sid)), "icacls never names the cell's SID, which it refuses: {steps:?}"); // WHAT: no grant to the cell through icacls. WHY: the end-to-end test found icacls refusing it
        assert!(!restricting_sids(&sid).iter().any(|s| s == "S-1-5-18" || s == "S-1-5-32-544"), "never the system or administrators");
    }
}
