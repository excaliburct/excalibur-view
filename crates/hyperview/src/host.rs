//! Hosting the company's server on this computer.
//!
//! Somebody has to be the server, and in a six-person shop that somebody is a
//! computer already sitting in the office. Asking them to find a Linux box,
//! install a runtime and run a command is asking them not to bother.
//!
//! So this program is the server. It is the same binary either way: started
//! normally it is the viewer, started with `--serve` it is the server, and the
//! "host the drawings on this computer" button does both at once — serves right
//! now from inside the running program, and arranges for it to keep serving
//! after the machine is rebooted.
//!
//! One thing this deliberately does **not** claim to do: install itself onto
//! somebody else's machine. Nothing can put software on a server it has no
//! account on, and a program that offered to would be lying. What it does
//! instead is be the thing that needed installing — and once it is running,
//! every other seat in the building finds it without anybody typing an address.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::mpsc;

use hyperview_server::{run, Config};

/// A server running inside this program.
pub struct Hosting {
    /// Where it ended up listening, ready to hand to somebody at another desk.
    pub url: String,
    pub address: SocketAddr,
    pub name: String,
    pub data: PathBuf,
    /// Dropping this stops it, which is what closing the program does.
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Hosting {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Hosting {
    /// What to tell everybody else in the office.
    pub fn tell_the_office(&self) -> String {
        format!(
            "{} is now serving at {}. Everybody else can open Excalibur View and it should \
             already be in the list — nothing to type.",
            self.name, self.url
        )
    }
}

/// Where a server on this machine keeps the company's drawings.
///
/// Beside the program rather than in one person's own folder, because it is the
/// company's data and not theirs: an account being deleted should not take the
/// drawings with it, and the next person to sit at that machine is looking at
/// the same server.
pub fn default_data_folder() -> PathBuf {
    // The same folder the installed Hyperview-Server.exe uses, so a shop that
    // starts by hosting from here and later installs the server properly
    // finds its drawings already where the service looks for them.
    #[cfg(windows)]
    {
        return hyperview_server::winsvc::data_folder();
    }
    #[allow(unreachable_code)]
    if let Some(dirs) = directories::ProjectDirs::from("com", "Excalibur", "Hyperview") {
        return dirs.data_dir().join("server");
    }
    PathBuf::from("hyperview-data")
}

/// Starts serving from inside this program, and does not return until it is
/// either listening or has failed.
///
/// Waiting for it is the point. The person who pressed the button is looking at
/// a dialog that needs to say either "here is the address" or why not, and a
/// server that reports success and then fails to bind is how somebody spends an
/// afternoon wondering why nobody else can see it.
pub fn start_here(name: &str, folder: PathBuf, port: u16) -> Result<Hosting, String> {
    let config = Config {
        name: name.trim().to_string(),
        listen: SocketAddr::from(([0, 0, 0, 0], port)),
        data: folder.clone(),
        tls: true, // WHAT: HTTPS on, with its own certificate, as the Office Server. WHY: review: the other seats never send a password over plain HTTP to a 0.7.0 server, so this one serves HTTPS too
        ..Config::default()
    };

    let (tell, told) = mpsc::channel::<Result<(String, SocketAddr, String), String>>();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();

    std::thread::Builder::new()
        .name("hyperview-server".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = tell.send(Err(format!("Could not start the server: {e}")));
                    return;
                }
            };
            let said = std::sync::Mutex::new(Some(tell));
            let result = runtime.block_on(run::serve(
                config,
                async {
                    let _ = stopped.await;
                },
                |serving| {
                    if let Some(tell) = said.lock().ok().and_then(|mut s| s.take()) {
                        let _ = tell.send(Ok((
                            serving.url(),
                            serving.address,
                            serving.name.clone(),
                        )));
                    }
                },
            ));
            // Failing to bind happens before `ready` is ever called, so the
            // person waiting is told why rather than left on a spinner.
            if let Err(e) = result {
                if let Some(tell) = said.lock().ok().and_then(|mut s| s.take()) {
                    let _ = tell.send(Err(explain(&e.to_string())));
                }
            }
        })
        .map_err(|e| format!("Could not start the server: {e}"))?;

    match told.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok((url, address, name))) => Ok(Hosting {
            url,
            address,
            name,
            data: folder,
            stop: Some(stop),
        }),
        Ok(Err(why)) => Err(why),
        Err(_) => Err("The server did not start within twenty seconds.".into()),
    }
}

/// Turns what the operating system said into what the person needs to do.
fn explain(trouble: &str) -> String {
    let lower = trouble.to_lowercase();
    if lower.contains("in use") || lower.contains("addrinuse") {
        return format!(
            "Something is already using port {}. That is usually an Excalibur View server \
             already running on this computer — if it is, it is the one everybody should be \
             using, and there is nothing to set up. Otherwise pick another port.",
            hyperview_server::config::DEFAULT_PORT
        );
    }
    if lower.contains("permission") || lower.contains("denied") {
        return "Windows would not let this program listen for other computers. Allow \
                Excalibur View through the firewall when it asks, or ask whoever looks after \
                the computers to."
            .into();
    }
    format!("The server could not start: {trouble}")
}

// ---- staying up after a reboot --------------------------------------------

/// What setting a machine up as the office server takes, beyond running it now.
///
/// Both of these need administrator rights on Windows, which means one prompt.
/// If the person says no, the server still works — it just stops when they
/// close Hyperview, which is a real answer for a shop where the same computer
/// is on all day anyway, and is said plainly rather than hidden.
#[derive(Clone, Debug, PartialEq)]
pub enum Standing {
    /// Installed as a Windows service and allowed through the firewall. It
    /// serves whether or not anybody is logged in.
    Always,
    /// It serves while Hyperview is open on this computer.
    WhileOpen(String),
}

impl Standing {
    pub fn says(&self) -> String {
        match self {
            Standing::Always => "This computer now serves the drawings whether or not \
                                 anybody is signed in to it."
                .into(),
            Standing::WhileOpen(why) => format!(
                "This computer is serving the drawings while Excalibur View is open on it. \
                 {why} Leaving Excalibur View open is enough; nobody will notice the difference \
                 until this computer is restarted."
            ),
        }
    }
}

/// Staying up after a reboot is what Hyperview-Server.exe is for.
///
/// This program serves from inside itself for as long as it is open, which is
/// a real answer for a shop where the same computer is on all day. Serving
/// with nobody signed in needs a proper Windows service, and that is one
/// double-click of the server program on this same computer — it finds these
/// same drawings, because it keeps them in the same folder.
#[cfg(windows)]
pub fn keep_serving(_folder: &std::path::Path, _port: u16, _name: &str) -> Standing {
    Standing::WhileOpen(
        "To keep serving after a restart with nobody signed in, double-click \
         Hyperview-Server.exe on this computer once; it takes over these same drawings."
            .into(),
    )
}

#[cfg(not(windows))]
pub fn keep_serving(_folder: &std::path::Path, _port: u16, _name: &str) -> Standing {
    Standing::WhileOpen(
        "Installing it as a service is a Windows thing; on this system run \
         `hyperview --serve` from whatever starts programs at boot."
            .into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_port_already_taken_is_explained_as_the_thing_it_usually_is() {
        let said = explain("listening on 0.0.0.0:8714: Address already in use (os error 98)");
        assert!(said.contains("already running"), "{said}");
    }

    #[test]
    fn being_up_only_while_the_program_is_open_is_said_out_loud() {
        let said = Standing::WhileOpen("Windows said no.".into()).says();
        assert!(said.contains("while Excalibur View is open"), "{said}");
        // And it says what that actually costs, rather than leaving somebody to
        // find out when the machine reboots.
        assert!(said.contains("restarted"), "{said}");
    }

    #[test]
    fn the_company_drawings_do_not_live_in_one_persons_folder() {
        let folder = default_data_folder();
        let text = folder.display().to_string();
        assert!(!text.is_empty());
        assert!(text.to_lowercase().contains("hyperview"), "{text}");
    }
}
