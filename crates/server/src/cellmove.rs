//! Moving an office's Citadel Server 1.0 data into the Citadel cell, once,
//! before the cell's first start.
//!
//! Citadel Server 1.0 ran as its own Windows service (`CitadelServer`) and
//! kept its data in `ProgramData\Citadel Server`, locked to the system,
//! administrators and that service. The cell runs under a restricted token
//! and can't read it, so the Office Server does the move for it:
//!
//! 1. the 1.0 service is stopped and uninstalled, so it lets go of port
//!    47110 and its files, and can never start again;
//! 2. `Citadel Server.exe --move-from "<old folder>" --data "<cell data>"` is
//!    run once, with the Office Server's own rights (the system), with a time
//!    limit;
//! 3. the result is kept in the store and shown on `/api/v1/office`;
//! 4. after a failure the cell is not started, because a cell started empty
//!    would make a new certificate and every estimating computer would have
//!    to pair again; the old data is left exactly as it was.
//!
//! After "moved" or "nothing to move" it never runs again.
//! `hyperview-server --retry-citadel-move` clears a failure so the next start
//! tries once more.
//!
//! The program prints one JSON line, `{"result","exit","files","reason",
//! "kept_as"}`, and exits with the same code. Only exit 0 with `moved`,
//! `nothing` or `already` is a success; `usage` (2), `refused` (3, the trust
//! check), `unreadable` (4, never "nothing"), `failed` (5), any other code, a
//! missing or unreadable line, or a timeout all hold the cell.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Where the result is kept.
pub const SETTING: &str = "citadel_move"; // WHAT: the setting's name. WHY: read on every start and by /api/v1/office

/// The longest the move may take.
pub const TIME_LIMIT: Duration = Duration::from_secs(30 * 60); // WHAT: half an hour. WHY: an office's whole estimating history, copied once; past it, something is stuck

/// The names the 1.0 service may have: its display name, and the name its
/// SID (`NT SERVICE\CitadelServer`) is made from.
pub const OLD_SERVICES: &[&str] = &["Citadel Server", "CitadelServer"]; // WHAT: both spellings, tried in turn. WHY: the one holding port 47110 and the old folder

/// The move program's one line, as it prints it.
#[derive(Deserialize, Debug, Default)]
struct Said {
    #[serde(default)]
    result: String, // WHAT: moved, nothing, already, usage, refused, unreadable or failed. WHY: decided with the exit code
    #[serde(default)]
    exit: Option<i32>, // WHAT: the code it says it exits with. WHY: must agree with the real one
    #[serde(default)]
    files: Option<u64>, // WHAT: how many files it moved. WHY: "moved N files"
    #[serde(default)]
    reason: Option<String>, // WHAT: why it failed, when it did. WHY: shown in the alarm
    #[serde(default)]
    kept_as: Option<String>, // WHAT: where the old folder was kept, renamed. WHY: said, so nobody hunts for it
}

/// What came of the move, as kept in the store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Moved {
    Moved { files: u64, kept_as: Option<String> }, // WHAT: moved, with how many files, and where the old folder was kept. WHY: "moved N files"
    Nothing, // WHAT: nothing to move. WHY: a fresh office, or no 1.0 data
    Already, // WHAT: the move program says it moved this folder before. WHY: never moved twice
    Failed { reason: String }, // WHAT: failed, and why. WHY: the cell isn't started; an admin is told
}

impl Moved {
    /// How `/api/v1/office` says it.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn words(&self) -> String {
        match self {
            Moved::Moved { files, kept_as: Some(kept) } => format!("moved {files} files; the old folder is kept as {kept}"), // WHAT: the count, and where the old folder went. WHY: the person checks it, and knows where the original is
            Moved::Moved { files, kept_as: None } => format!("moved {files} files"), // WHAT: the count. WHY: the person checks it against the old server
            Moved::Nothing => "nothing to move".to_string(), // WHAT: said. WHY: plain
            Moved::Already => "moved before".to_string(), // WHAT: said. WHY: plain
            Moved::Failed { reason } => format!("move failed: {reason}. The old data is untouched and the cell was not started; run hyperview-server --retry-citadel-move, then restart the Office Server"), // WHAT: the alarm, with what to do. WHY: a cell started empty would make a new certificate
        }
    }

    /// Whether the cell may start after this.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn lets_the_cell_start(&self) -> bool {
        !matches!(self, Moved::Failed { .. }) // WHAT: only a failure holds it. WHY: never start empty over data that didn't move
    }
}

/// What was recorded, if anything.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn recorded(store: &crate::Store) -> Option<Moved> {
    store.setting(SETTING).ok().flatten().and_then(|text| serde_json::from_str(&text).ok()) // WHAT: the kept result, read. WHY: one that doesn't read counts as none, so the move runs, never the cell over unmoved data
}

/// Keeps a result, and writes it in the audit log.
fn record(store: &crate::Store, moved: &Moved) {
    if let Ok(text) = serde_json::to_string(moved) {
        let _ = store.set_setting(SETTING, &text); // WHAT: kept. WHY: never run again after a success; read by /office
    }
    let _ = store.audit(&crate::audit::Entry::new("moved Citadel Server's data into the cell", "the Office Server").saying(moved.words())); // WHAT: written. WHY: whoever looks after the server can find when and what
}

/// The 1.0 service, as the move sees it.
pub trait OldService {
    /// Whether it is installed.
    fn present(&self) -> bool;
    /// Stops it and uninstalls it.
    ///
    /// # Errors
    ///
    /// Why it couldn't.
    fn stop_and_remove(&self) -> Result<(), String>;
}

/// Running the move program.
pub trait Mover {
    /// Runs it with these arguments, within `limit`: its exit code (none
    /// when it was stopped for taking too long) and what it printed.
    ///
    /// # Errors
    ///
    /// When it couldn't be started.
    fn run(&self, program: &Path, args: &[String], limit: Duration) -> Result<(Option<i32>, String), String>;
}

/// The move program's arguments, as the .NET side defines them.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn arguments(old: &Path, data: &Path) -> Vec<String> {
    vec!["--move-from".to_string(), old.display().to_string(), "--data".to_string(), data.display().to_string()] // WHAT: the two folders. WHY: the one-shot move
}

/// What the move program's exit code and its one JSON line mean. Only exit
/// 0, said as 0, with `moved`, `nothing` or `already`, is a success.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn reading(code: Option<i32>, out: &str) -> Moved {
    let Some(code) = code else {
        return Moved::Failed { reason: "the move took longer than half an hour and was stopped".to_string() }; // WHAT: timed out. WHY: never a success
    };
    let said = out.lines().rev().map(str::trim).find(|l| l.starts_with('{')).and_then(|l| serde_json::from_str::<Said>(l).ok()); // WHAT: its JSON line, read. WHY: decided with the code
    let Some(said) = said else {
        return Moved::Failed { reason: format!("the move program gave no answer it could read (code {code})") }; // WHAT: no line, or one that doesn't read. WHY: never taken as a success
    };
    if code == 0 && said.exit == Some(0) {
        match said.result.as_str() {
            "moved" => return Moved::Moved { files: said.files.unwrap_or(0), kept_as: said.kept_as.map(|k| k.chars().take(200).collect()) }, // WHAT: moved, its file count. WHY: read from `files`, never the first number in the output
            "nothing" => return Moved::Nothing, // WHAT: no old folder. WHY: a fresh office
            "already" => return Moved::Already, // WHAT: marked before. WHY: never run again
            _ => {} // WHAT: any other word with code 0. WHY: never a success
        }
    }
    let why = said.reason.filter(|r| !r.trim().is_empty()).unwrap_or_else(|| if said.result.is_empty() { "no reason given".to_string() } else { said.result.clone() }); // WHAT: its reason, or its result word. WHY: what the person reads
    Moved::Failed { reason: format!("{} ({}, code {code})", why.chars().take(200).collect::<String>(), if said.result.is_empty() { "unknown" } else { said.result.as_str() }) } // WHAT: refused, unreadable, failed, usage, or anything else. WHY: the cell never starts over data that didn't move
}

/// Before the cell's first start: stops and uninstalls the 1.0 service, and
/// moves its data in once. Whether the cell may start.
///
/// Runs only when nothing is recorded yet; after a success or "nothing to
/// move" it never runs again, and after a failure it doesn't run until
/// `--retry-citadel-move` clears it.
pub fn before_first_start(store: &crate::Store, old: &dyn OldService, mover: &dyn Mover, program: &Path, old_folder: &Path, data: &Path) -> bool {
    if let Some(done) = recorded(store) {
        return done.lets_the_cell_start(); // WHAT: decided before. WHY: never run again after a success; a failure holds the cell until retried
    }
    if old.present() {
        match old.stop_and_remove() {
            Ok(()) => tracing::warn!("Citadel Server 1.0's service was stopped and uninstalled; the Office Server's Citadel cell takes over"), // WHAT: written in the log. WHY: step 1
            Err(why) => {
                let failed = Moved::Failed { reason: format!("Citadel Server 1.0's service couldn't be stopped ({why})") }; // WHAT: failed. WHY: it holds the port and the files
                record(store, &failed);
                return false;
            }
        }
    }
    if std::fs::symlink_metadata(old_folder).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        record(store, &Moved::Nothing); // WHAT: no 1.0 folder at all. WHY: a fresh office; anything else there, even unreadable, is the move program's to judge
        return true;
    }
    let outcome = match mover.run(program, &arguments(old_folder, data), TIME_LIMIT) {
        Ok((code, said)) => reading(code, &said), // WHAT: what it said. WHY: kept
        Err(why) => Moved::Failed { reason: format!("the move program couldn't be started ({why})") }, // WHAT: failed. WHY: said
    };
    record(store, &outcome); // WHAT: kept, whatever it was. WHY: shown on /api/v1/office, and never run twice after a success
    if !outcome.lets_the_cell_start() {
        tracing::error!("ALARM: Citadel Server 1.0's data couldn't be moved into the cell ({}); the old data is untouched and the cell is not started", outcome.words()); // WHAT: the alarm. WHY: the cell must never start empty and make a new certificate
    }
    outcome.lets_the_cell_start()
}

/// Clears a recorded failure so the next start tries once more
/// (`hyperview-server --retry-citadel-move`). A success is never cleared.
///
/// # Errors
///
/// When nothing failed, or the store can't be written.
pub fn retry(store: &crate::Store) -> anyhow::Result<()> {
    match recorded(store) {
        Some(Moved::Failed { .. }) => {
            store.with(|db| Ok(db.execute("DELETE FROM settings WHERE key = ?1", rusqlite::params![SETTING])?))?; // WHAT: the failure forgotten. WHY: the next start runs the move again
            let _ = store.audit(&crate::audit::Entry::new("cleared a failed Citadel move to try again", "the server console")); // WHAT: written. WHY: every change is in the audit log
            Ok(())
        }
        Some(done) => anyhow::bail!("Citadel's data was already dealt with ({}); it is never moved twice", done.words()), // WHAT: refused. WHY: a second move could overwrite the cell's own data
        None => anyhow::bail!("no Citadel move has been tried yet; the next start of the Office Server tries it"), // WHAT: nothing to clear. WHY: said
    }
}

/// Runs a program with a time limit, for real: its exit code and output.
pub struct Run;

impl Mover for Run {
    fn run(&self, program: &Path, args: &[String], limit: Duration) -> Result<(Option<i32>, String), String> {
        let mut child = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| e.to_string())?; // WHAT: started with this service's own rights. WHY: the old folder is locked to the system; no restricted token here
        let (mut out, mut err) = (child.stdout.take(), child.stderr.take()); // WHAT: its output. WHY: read on threads so a full pipe never stalls it
        let reading_out = std::thread::spawn(move || read_all(out.as_mut())); // WHAT: what it prints. WHY: its count, or its reason
        let reading_err = std::thread::spawn(move || read_all(err.as_mut())); // WHAT: its errors. WHY: the reason, when it fails
        let until = std::time::Instant::now() + limit; // WHAT: the deadline. WHY: never waited for without end
        let code = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status.code(), // WHAT: it finished. WHY: its code
                Ok(None) if std::time::Instant::now() < until => std::thread::sleep(Duration::from_millis(200)), // WHAT: still running. WHY: checked again shortly
                _ => {
                    let _ = child.kill(); // WHAT: stopped. WHY: past its time
                    let _ = child.wait(); // WHAT: reaped. WHY: no process left behind
                    break None;
                }
            }
        };
        let said = format!("{}\n{}", reading_out.join().unwrap_or_default(), reading_err.join().unwrap_or_default()); // WHAT: all it said. WHY: read by `reading`
        Ok((code, said))
    }
}

/// Everything a pipe gives, at most 64 KiB of it.
fn read_all(from: Option<&mut impl std::io::Read>) -> String {
    let Some(from) = from else { return String::new() };
    let mut text = String::new();
    let _ = std::io::Read::read_to_string(&mut std::io::Read::take(from, 64 * 1024), &mut text); // WHAT: read, bounded. WHY: a program printing without end can't fill memory
    text
}

/// Citadel Server 1.0's data folder: `ProgramData\Citadel Server`.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn old_folder(program_data: &Path) -> PathBuf {
    program_data.join("Citadel Server") // WHAT: the folder 1.0 kept. WHY: where the move reads from
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Service {
        there: bool,
        stops: Cell<u32>,
        fails: bool,
    }

    impl OldService for Service {
        fn present(&self) -> bool {
            self.there
        }
        fn stop_and_remove(&self) -> Result<(), String> {
            self.stops.set(self.stops.get() + 1);
            if self.fails { Err("access denied".into()) } else { Ok(()) }
        }
    }

    fn store() -> (crate::Store, PathBuf) {
        let n: u64 = rand::random();
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(format!("cellmove-{n:016x}")); // WHAT: a fresh folder under the build's own. WHY: never a shared temporary folder
        std::fs::create_dir_all(&dir).unwrap();
        let config = crate::Config { data: dir.clone(), ..crate::Config::default() };
        (crate::Store::open(&config.database(), &config.blobs()).unwrap(), dir)
    }

    /// A fake move program: a shell script that prints `said` and exits
    /// with `code` (or sleeps, when `code` is none).
    #[cfg(unix)]
    fn fake(dir: &Path, said: &str, code: Option<i32>) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(format!("move-{}.sh", rand::random::<u32>()));
        let body = match code {
            Some(c) => format!("#!/bin/sh\necho \"$@\" > \"{}/args\"\necho '{said}'\nexit {c}\n", dir.display()),
            None => "#!/bin/sh\nsleep 30\n".to_string(),
        };
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The one line the move program prints for `result`.
    fn line(result: &str, exit: i32, extra: &str) -> String {
        format!("{{\"result\":\"{result}\",\"exit\":{exit}{extra}}}") // WHAT: its JSON line. WHY: as the .NET side prints it
    }

    /// Runs the move for one fake outcome on a fresh store: whether the cell
    /// may start, and what was recorded.
    #[cfg(unix)]
    fn outcome(said: &str, code: Option<i32>) -> (bool, Moved) {
        let (store, dir) = store();
        let old = dir.join("Citadel Server");
        std::fs::create_dir_all(&old).unwrap(); // WHAT: a 1.0 folder. WHY: so the program is run
        let program = fake(&dir, said, code);
        let service = Service { there: false, stops: Cell::new(0), fails: false };
        let starts = before_first_start(&store, &service, &Run, &program, &old, &dir.join("cell"));
        (starts, recorded(&store).expect("recorded"))
    }

    #[cfg(unix)]
    #[test]
    fn only_moved_nothing_and_already_start_the_cell() {
        let (starts, moved) = outcome(&line("moved", 0, ",\"files\":12,\"kept_as\":\"Citadel Server.moved\""), Some(0));
        assert!(starts, "moved");
        assert_eq!(moved, Moved::Moved { files: 12, kept_as: Some("Citadel Server.moved".into()) }, "the count is read from files, never the first number");
        assert!(outcome(&line("nothing", 0, ""), Some(0)).0, "nothing");
        assert_eq!(outcome(&line("already", 0, ""), Some(0)).1, Moved::Already);
        for (result, exit) in [("usage", 2), ("refused", 3), ("unreadable", 4), ("failed", 5), ("odd", 9)] {
            let (starts, moved) = outcome(&line(result, exit, ",\"reason\":\"the folder is not a 1.0 folder\""), Some(exit));
            assert!(!starts, "{result}: the cell never starts");
            assert!(moved.words().starts_with("move failed: the folder is not a 1.0 folder") && moved.words().contains(result), "{}", moved.words());
        }
        let (starts, moved) = outcome("Moved everything", Some(0));
        assert!(!starts, "a line that isn't JSON is never a success: {}", moved.words());
        let (starts, _) = outcome(&line("moved", 3, ""), Some(0));
        assert!(!starts, "the line and the code must agree");
        let (starts, _) = outcome(&line("refused", 3, ""), Some(3));
        assert!(!starts, "refused, the .NET side's 3");
    }

    #[cfg(unix)]
    #[test]
    fn after_a_failure_the_cell_is_not_started_and_the_move_is_not_run_again_until_retried() {
        let (store, dir) = store();
        let old = dir.join("Citadel Server");
        std::fs::create_dir_all(&old).unwrap();
        let service = Service { there: true, stops: Cell::new(0), fails: false };
        let failing = fake(&dir, &line("failed", 5, ",\"reason\":\"a file could not be copied\""), Some(5));
        assert!(!before_first_start(&store, &service, &Run, &failing, &old, &dir.join("cell")), "the cell never starts over data that didn't move");
        let words = recorded(&store).unwrap().words();
        assert!(words.starts_with("move failed: a file could not be copied (failed, code 5)") && words.contains("untouched"), "{words}");
        assert_eq!(service.stops.get(), 1, "the 1.0 service was stopped and uninstalled first");
        assert!(std::fs::read_to_string(dir.join("args")).unwrap().contains("--move-from"), "run with --move-from");
        let good = fake(&dir, &line("moved", 0, ",\"files\":4"), Some(0));
        assert!(!before_first_start(&store, &service, &Run, &good, &old, &dir.join("cell")), "not tried again on its own");
        assert_eq!(service.stops.get(), 1);
        retry(&store).unwrap();
        assert!(before_first_start(&store, &service, &Run, &good, &old, &dir.join("cell")), "once retried, it runs");
        assert_eq!(recorded(&store), Some(Moved::Moved { files: 4, kept_as: None }));
        assert!(retry(&store).is_err(), "a success is never cleared");
        assert!(before_first_start(&store, &service, &Run, &failing, &old, &dir.join("cell")), "and the move never runs again after a success");
        assert_eq!(recorded(&store), Some(Moved::Moved { files: 4, kept_as: None }));
    }

    #[cfg(unix)]
    #[test]
    fn a_move_that_takes_too_long_is_stopped_and_fails() {
        let (_, dir) = store();
        let slow = fake(&dir, "", None);
        let (code, _) = Run.run(&slow, &[], Duration::from_millis(300)).unwrap();
        assert_eq!(code, None, "stopped");
        assert!(!reading(code, "").lets_the_cell_start());
    }

    #[test]
    fn an_old_service_that_won_t_stop_holds_the_cell() {
        let (store, dir) = store();
        let service = Service { there: true, stops: Cell::new(0), fails: true };
        assert!(!before_first_start(&store, &service, &Run, Path::new("/nonexistent"), &dir.join("none"), &dir.join("cell")));
        assert!(recorded(&store).unwrap().words().contains("couldn't be stopped"));
    }

    #[test]
    fn no_old_folder_is_nothing_to_move_without_running_anything() {
        let (store, dir) = store();
        let service = Service { there: false, stops: Cell::new(0), fails: false };
        assert!(before_first_start(&store, &service, &Run, Path::new("/nonexistent"), &dir.join("none"), &dir.join("cell")));
        assert_eq!(recorded(&store), Some(Moved::Nothing));
    }
}
