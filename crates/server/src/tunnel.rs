//! Reaching this server from a jobsite.
//!
//! On the shop network a seat finds its server by shouting on UDP and the
//! server answering. From a truck, a trailer or a hotel it cannot: a broadcast
//! does not cross a router. The answer is an outbound connection from this
//! server to Cloudflare, which then answers on a public hostname — nothing is
//! opened on the shop's firewall, because the connection is made from the
//! inside, the way a browser makes one.
//!
//! **The tunnel belongs to the shop, not to us.** It lives in their own
//! Cloudflare account, on their hostname, made with their token. We cannot
//! reach it, cannot see it and cannot take it away; if they stop paying us it
//! keeps working. That is not generosity, it is the product: this thing is
//! sold on the promise that a shop's drawings stay in the shop's hands, and
//! routing every customer's jobsite traffic through our account would make
//! that untrue in the one place it matters, which is the path the drawings
//! actually travel.
//!
//! **A sealed server never does any of this.** Every outward connection is
//! refused and that is the whole of the edition. The panel says so rather than
//! offering a button that cannot work.
//!
//! What this is not: a VPN. One service is reachable on one hostname — this
//! server's API. Nothing else on the shop's network is behind the tunnel, so
//! nothing else is exposed.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Where the token and hostname are kept, in the settings table beside
/// everything else this server remembers.
const TOKEN: &str = "tunnel.token";
const HOSTNAME: &str = "tunnel.hostname";

/// The version of `cloudflared` this build fetches, and what it must hash to.
///
/// Pinned, and checked before the file is ever run — the same way the PDF
/// engine is pinned, and for the same reason. A downloaded program that
/// matches its digest and nothing else proves only that whoever wrote the
/// digest also wrote the file; but this digest ships inside a signed build, so
/// anybody who could change it could already change everything.
///
/// These were taken from Cloudflare's own release and then checked by
/// downloading the file and hashing it, rather than believing a field in an
/// API answer. Updating the version means updating the digest in the same
/// commit, and there is a test below that fails if one is a placeholder.
pub const CLOUDFLARED_VERSION: &str = "2026.9.1";

/// The digest per platform. **Empty means this build has no checked copy for
/// this kind of machine and will not fetch one at all** — better no remote
/// access than a program nobody verified.
///
/// macOS is empty on purpose rather than by omission: there is no macOS build
/// of this server, so nothing would ever run it. When there is one, the
/// download for it is a `.tgz` and has to be unpacked before it is hashed,
/// which is why it cannot simply be pasted in beside the other two.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn expected_digest() -> &'static str {
    if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "2837888cc0f5d58f15b6dc478376de90b4d3ba5241c7947455d1e0a0df429712"
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "03f1f25d1cc93b9ad6c60569d44060bc4f17ed97075760ed8cfca4b12dcd68cc"
    } else {
        ""
    }
}

fn download_url() -> Option<String> {
    let file = if cfg!(target_os = "windows") {
        "cloudflared-windows-amd64.exe"
    } else if cfg!(target_os = "linux") {
        "cloudflared-linux-amd64"
    } else {
        return None;
    };
    Some(format!(
        "https://github.com/cloudflare/cloudflared/releases/download/{CLOUDFLARED_VERSION}/{file}"
    ))
}

/// What the administrator is shown.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum State {
    /// Nobody has turned it on. The ordinary state, and not a problem.
    Off,
    /// This server is sealed. It will not make an outward connection for this
    /// or for anything else, which is what was bought.
    Sealed,
    /// A token is held and the connector is being started.
    Starting,
    /// Up, and the address answered when this server asked the public
    /// internet for it. That last part matters: a connector that started is
    /// not the same as an address that works.
    On { address: String },
    /// Turned on, running, and the address did not answer. Said plainly,
    /// because an administrator who believes remote access works and finds
    /// out in a truck has been failed twice.
    Unreachable { address: String, why: String },
    /// It would not start at all.
    Trouble { why: String },
}

impl State {
    /// One line for the panel.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn in_words(&self) -> String {
        match self {
            State::Off => "Off. Seats find this server on the shop network only.".into(),
            State::Sealed => {
                "This server is sealed, so it makes no outward connections at all — \
                 including this one. That is what the Sealed edition is."
                    .into()
            }
            State::Starting => "Starting…".into(),
            State::On { address } => format!("On. Seats can reach this server at {address}."),
            State::Unreachable { address, why } => format!(
                "The connector is running but {address} did not answer: {why}. \
                 Check the hostname points at this tunnel in your Cloudflare dashboard."
            ),
            State::Trouble { why } => format!("It would not start: {why}"),
        }
    }
}

/// The connector, and what it is doing.
#[derive(Clone, Default)]
pub struct Tunnel {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Default)]
struct Inner {
    state: Option<State>,
    /// Whether an administrator wants it on. Turning it off has to stop the
    /// restart loop as well as the process, or it comes straight back.
    wanted: bool,
    /// The running connector, so it can be stopped and so a restart knows
    /// whether one is already up.
    child: Option<std::process::Child>,
}

impl std::fmt::Debug for Tunnel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Tunnel({:?})", self.state_now())
    }
}

impl Tunnel {
    /// What to show, without starting or stopping anything.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn state_now(&self) -> State {
        if hub::sealed::is_sealed() {
            return State::Sealed;
        }
        self.inner
            .lock()
            .ok()
            .and_then(|i| i.state.clone())
            .unwrap_or(State::Off)
    }

    fn set(&self, state: State) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.state = Some(state);
        }
    }

    /// Keeps hold of the running connector, so stopping actually stops it.
    fn hold(&self, child: std::process::Child) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.child = Some(child);
        }
    }

    fn want(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.wanted = true;
        }
    }

    /// False once somebody has turned it off, which is how the restart loop
    /// knows to stop rather than fighting them.
    fn wanted(&self) -> bool {
        self.inner.lock().map(|i| i.wanted).unwrap_or(false)
    }

    /// True when a connector of ours is running right now.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn running(&self) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        match inner.child.as_mut() {
            Some(child) => matches!(child.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Stops the connector. Safe to call when nothing is running.
    pub fn stop(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            inner.wanted = false;
            if let Some(mut child) = inner.child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
            inner.state = Some(State::Off);
        }
    }
}

/// Starts the connector, in the background, and keeps it up.
///
/// Fetching can take a while on a slow line, so none of this happens on the
/// request that turned it on: that returns `Starting` and the panel watches.
///
/// The restart loop is deliberately patient rather than eager. A connector
/// that cannot start -- a revoked token, a tunnel deleted in the dashboard --
/// must not turn into a machine hammering Cloudflare all night, so the wait
/// between goes grows and the reason stays on screen.
pub fn start(tunnel: &Tunnel, data: &Path, token: String, hostname: String) {
    if hub::sealed::is_sealed() {
        tunnel.set(State::Sealed);
        return;
    }
    tunnel.set(State::Starting);
    tunnel.want();
    let tunnel = tunnel.clone();
    let data = data.to_path_buf();
    std::thread::spawn(move || {
        let connector = match fetch_connector(&data, download) {
            Ok(path) => path,
            Err(why) => {
                tunnel.set(State::Trouble { why });
                return;
            }
        };
        let mut waited = 2u64;
        while tunnel.wanted() {
            match run_once(&connector, &token) {
                Ok(child) => {
                    tunnel.hold(child);
                    // A moment before telling anybody it is up: a revoked
                    // token fails immediately, and "On" followed by "Trouble"
                    // two seconds later is worse than a slightly slower "On".
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    if tunnel.running() {
                        tunnel.set(match reachable(&hostname) {
                            Ok(()) => State::On { address: hostname.clone() },
                            Err(why) => State::Unreachable {
                                address: hostname.clone(),
                                why,
                            },
                        });
                        waited = 2;
                        // Sit here while it runs. Checked rather than waited
                        // on, so turning it off does not have to wait for the
                        // connector to feel like stopping.
                        while tunnel.running() && tunnel.wanted() {
                            std::thread::sleep(std::time::Duration::from_secs(2));
                        }
                    } else {
                        tunnel.set(State::Trouble {
                            why: "the connector stopped as soon as it started. \
                                  The token may have been revoked in Cloudflare."
                                .into(),
                        });
                    }
                }
                Err(why) => tunnel.set(State::Trouble { why }),
            }
            if !tunnel.wanted() {
                return;
            }
            // Patient rather than eager. A token that has been revoked must
            // not turn this into a machine hammering Cloudflare all night.
            std::thread::sleep(std::time::Duration::from_secs(waited));
            waited = (waited * 2).min(300);
        }
    });
}

fn run_once(connector: &Path, token: &str) -> Result<std::process::Child, String> {
    // WHAT: the file checked again, every time, just before it is run. WHY: it was checked only when fetched; restarts ran whatever was at that path since, and on a server whose folder others can write, that could be anything, run as the service.
    if want_matches(connector).is_none() {
        return Err("the connector on this server is not the one that was checked, so it was not run. Turn remote access off and on again to fetch it afresh.".into());
    }
    connector_command(connector, token)
        .spawn()
        .map_err(|e| format!("the connector would not start: {e}"))
}

/// Whether the file at `connector` is the checked connector: `Some` only
/// when this build has a digest for this machine and the file hashes to it.
fn want_matches(connector: &Path) -> Option<()> {
    let want = expected_digest(); // WHAT: the digest built in. WHY: what the file must hash to.
    (!want.is_empty() && digest_of(connector).as_deref() == Some(want)).then_some(()) // WHAT: matched, or not. WHY: an empty digest matches nothing.
}

/// How the connector is started: its token handed over in its environment,
/// as `TUNNEL_TOKEN`, never on its command line, where anybody on the
/// computer who can list programs could read it.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn connector_command(connector: &Path, token: &str) -> std::process::Command {
    let mut command = std::process::Command::new(connector); // WHAT: the connector. WHY: run as a child of the server.
    command
        .args(["tunnel", "--no-autoupdate", "run"]) // WHAT: run the tunnel the token names. WHY: the token alone says which.
        .env("TUNNEL_TOKEN", token) // WHAT: the token, in its environment. WHY: cloudflared reads it from there; a command line is visible to every account.
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// Asks the public internet for this server, the way a seat in a truck will.
///
/// Worth doing because a connector that started is not the same as an address
/// that works: the hostname has to point at this tunnel, and that is set in
/// Cloudflare's dashboard rather than here. An administrator who believes
/// remote access works and finds out otherwise in a truck has been failed
/// twice.
fn reachable(hostname: &str) -> Result<(), String> {
    let url = format!("https://{hostname}{}/health", hub::API_ROOT); // WHAT: the real health route. WHY: /health has never existed, so the check always said unreachable
    hub::web::outbound("Checking the tunnel answers", &url)?;
    let agent = hub::web::agent()
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let response = agent.get(&url).call().map_err(|e| format!("{e}"))?;
    let health: serde_json::Value = response
        .into_json()
        .map_err(|e| format!("it answered, but not the way this server would ({e})"))?;
    if health.get("api_version").is_some() {
        Ok(())
    } else {
        Err("something answered at that address, but it is not this server".into())
    }
}

fn download(url: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    /// Bigger than the connector has ever been, and small enough that a
    /// server pointed at something enormous does not fill its disk.
    const LARGEST: u64 = 200 * 1024 * 1024;
    hub::web::outbound("Fetching the tunnel connector", url)?;
    let agent = hub::web::agent()
        .timeout(std::time::Duration::from_secs(600))
        .build();
    let response = agent.get(url).call().map_err(|e| format!("{e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(LARGEST)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("the download stopped part way: {e}"))?;
    Ok(bytes)
}

/// Whether a hostname is one we are willing to hand to a seat.
///
/// Checked because it is typed by a person and then handed to seats as an
/// address to connect to. A hostname with a scheme, a path or a port in it is
/// somebody misunderstanding the box rather than an attack, and saying so at
/// the point of typing is worth more than a connector that starts and never
/// works.
///
/// # Errors
///
/// A sentence saying what is wrong with what was typed.
pub fn a_hostname(typed: &str) -> Result<String, String> {
    let typed = typed.trim().trim_end_matches('.').to_ascii_lowercase();
    if typed.is_empty() {
        return Err("Type the hostname you gave the tunnel in Cloudflare.".into());
    }
    if typed.contains("://") || typed.contains('/') {
        return Err("Just the hostname — no https:// and no path after it.".into());
    }
    if typed.contains(':') {
        return Err("Just the hostname — no port after it.".into());
    }
    if typed.contains(' ') {
        return Err("A hostname has no spaces in it.".into());
    }
    if !typed.contains('.') {
        return Err("That is not a whole hostname: it needs a domain, like drawings.yourshop.com.".into());
    }
    if typed.starts_with('-') || typed.ends_with('-') {
        return Err("A hostname does not start or end with a dash.".into());
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || c == '.' || c == '-';
    if !typed.chars().all(allowed) {
        return Err("A hostname is letters, digits, dots and dashes.".into());
    }
    if typed.len() > 253 {
        return Err("That hostname is too long to be one.".into());
    }
    Ok(typed)
}

/// Whether a token is the shape Cloudflare hands out.
///
/// Deliberately loose: the exact shape is Cloudflare's to change and refusing
/// a token that would have worked is worse than passing one that will not.
/// What this catches is the common mistake, which is pasting the whole
/// `cloudflared service install <token>` command line.
///
/// # Errors
///
/// A sentence saying what is wrong with what was typed.
pub fn a_token(typed: &str) -> Result<String, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Err("Paste the token Cloudflare gave you when you made the tunnel.".into());
    }
    if typed.contains(' ') || typed.contains('\n') {
        return Err(
            "That looks like the whole command line. Paste only the token at the end of it — \
             the long run of letters and digits."
                .into(),
        );
    }
    if typed.len() < 40 {
        return Err("That is too short to be a tunnel token.".into());
    }
    Ok(typed.to_string())
}

/// Where the connector is kept, beside the server's own data.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn connector_path(data: &Path) -> PathBuf {
    let name = if cfg!(target_os = "windows") {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    data.join("connector").join(name)
}

/// Fetches the connector if it is not already there, and refuses to keep one
/// whose digest is wrong.
///
/// A program downloaded onto a customer's server and run as a child process is
/// exactly the thing that has to be checked, and a build with no digest for
/// its own platform does not fetch at all. Better no remote access than a
/// program nobody verified.
///
/// # Errors
///
/// A sentence saying why, when it can't be fetched or its digest is wrong.
pub fn fetch_connector(data: &Path, get: impl Fn(&str) -> Result<Vec<u8>, String>) -> Result<PathBuf, String> {
    let want = expected_digest();
    if want.is_empty() {
        return Err(
            "This build has no checked copy of the connector for this kind of machine, \
             so it will not fetch one."
                .into(),
        );
    }
    let path = connector_path(data);
    if path.exists() && digest_of(&path).as_deref() == Some(want) {
        return Ok(path);
    }
    let url = download_url().ok_or("There is no connector for this kind of machine.")?;
    let bytes = get(&url)?;
    let got = hex(&{
        use sha2::Digest;
        sha2::Sha256::digest(&bytes)
    });
    if got != want {
        return Err(format!(
            "The connector that came down is not the one expected \
             (wanted {want}, got {got}). Nothing has been kept."
        ));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
    }
    Ok(path)
}

fn digest_of(path: &Path) -> Option<String> {
    use sha2::Digest;
    let bytes = std::fs::read(path).ok()?;
    Some(hex(&sha2::Sha256::digest(&bytes)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// What is stored, if anything.
pub fn held(store: &crate::store::Store) -> Option<(String, String)> {
    let token = store.setting(TOKEN).ok().flatten()?;
    let hostname = store.setting(HOSTNAME).ok().flatten()?;
    if token.is_empty() || hostname.is_empty() {
        return None;
    }
    Some((token, hostname))
}

/// Keeps the token and hostname for next time.
///
/// # Errors
///
/// A sentence saying why, when it can't be kept.
pub fn remember(store: &crate::store::Store, token: &str, hostname: &str) -> Result<(), String> {
    store.set_setting(TOKEN, token).map_err(|e| e.to_string())?;
    store.set_setting(HOSTNAME, hostname).map_err(|e| e.to_string())?;
    Ok(())
}

/// Forgets it, which is what turning it off means.
///
/// # Errors
///
/// A sentence saying why, when it can't be forgotten.
pub fn forget(store: &crate::store::Store) -> Result<(), String> {
    store.set_setting(TOKEN, "").map_err(|e| e.to_string())?;
    store.set_setting(HOSTNAME, "").map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder for one test, inside the build folder.
    fn scratch(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-scratch").join(name) // WHAT: under target. WHY: a connector a test writes is never somewhere another account shares.
    }

    #[test]
    fn a_hostname_is_a_hostname() {
        assert_eq!(a_hostname("drawings.example.com").unwrap(), "drawings.example.com");
        assert_eq!(a_hostname("  Drawings.Example.com  ").unwrap(), "drawings.example.com");
        // A trailing dot is legal in DNS and confusing everywhere else.
        assert_eq!(a_hostname("drawings.example.com.").unwrap(), "drawings.example.com");
    }

    #[test]
    fn the_mistakes_people_actually_make_are_caught_at_the_box() {
        for (typed, hint) in [
            ("https://drawings.example.com", "https://"),
            ("drawings.example.com/api", "path"),
            ("drawings.example.com:8714", "port"),
            ("drawings example com", "spaces"),
            ("localhost", "whole hostname"),
            ("", "Type the hostname"),
        ] {
            let said = a_hostname(typed).unwrap_err();
            assert!(
                said.to_lowercase().contains(&hint.to_lowercase()),
                "{typed:?} said {said:?}, which does not mention {hint:?}"
            );
        }
    }

    #[test]
    fn a_token_that_is_really_a_command_line_is_refused_with_the_reason() {
        let said = a_token("cloudflared service install eyJhIjoiabc123456789012345678901234567890")
            .unwrap_err();
        assert!(said.contains("command line"), "{said}");
    }

    #[test]
    fn a_real_looking_token_is_kept_as_it_is() {
        let token = "eyJhIjoiYWJjMTIzNDU2Nzg5MCIsInQiOiJkZWYiLCJzIjoiZ2hpIn0=";
        assert_eq!(a_token(&format!("  {token}  ")).unwrap(), token);
    }

    #[test]
    fn the_pinned_digest_is_a_real_one() {
        // Cheap, and it guards the thing most likely to go wrong when
        // somebody bumps the version: changing CLOUDFLARED_VERSION and
        // leaving the digest, or pasting a digest with a typo in it. A wrong
        // digest fails safe -- nothing is run -- but it fails at a customer's
        // server rather than here.
        let want = expected_digest();
        if want.is_empty() {
            // Only legal where there is no download either.
            assert!(
                download_url().is_none(),
                "this platform has a download but no digest to check it with"
            );
            return;
        }
        assert_eq!(want.len(), 64, "a SHA-256 is 64 hex characters");
        assert!(
            want.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
            "a digest is lower-case hex"
        );
        assert!(
            want.chars().collect::<std::collections::HashSet<_>>().len() > 8,
            "that looks like a placeholder rather than a digest"
        );
        assert!(download_url().is_some(), "a digest with nothing to check");
    }

    #[test]
    fn a_sealed_server_says_no_before_it_says_anything_else() {
        // Not a button that fails: a sentence saying it will never work here,
        // because that is what the edition was bought for.
        let state = State::Sealed;
        assert!(state.in_words().contains("sealed"));
        assert!(state.in_words().contains("Sealed edition"));
    }

    #[test]
    fn the_connector_is_never_fetched_without_a_digest_to_check_it_against() {
        // The failure that matters: a build with no pinned digest for its own
        // platform must refuse rather than run whatever comes down the wire.
        let dir = scratch("excalibur-tunnel-test-nodigest"); // WHAT: a folder of the test's own. WHY: inside the build folder, not the shared temporary one.
        let asked = std::cell::Cell::new(false);
        let get = |_: &str| -> Result<Vec<u8>, String> {
            asked.set(true);
            Ok(b"anything at all".to_vec())
        };
        if expected_digest().is_empty() {
            let said = fetch_connector(&dir, get).unwrap_err();
            assert!(said.contains("will not fetch"), "{said}");
            assert!(!asked.get(), "it should not have gone out at all");
        }
    }

    #[test]
    fn a_connector_whose_digest_is_wrong_is_not_kept() {
        let dir = scratch("excalibur-tunnel-test-baddigest"); // WHAT: a folder of the test's own. WHY: as above.
        let _ = std::fs::remove_dir_all(&dir);
        if expected_digest().is_empty() {
            return; // covered by the test above on this platform
        }
        let said = fetch_connector(&dir, |_| Ok(b"not the connector".to_vec())).unwrap_err();
        assert!(said.contains("not the one expected"), "{said}");
        assert!(
            !connector_path(&dir).exists(),
            "a connector that did not check out was written to disk anyway"
        );
    }

    #[test]
    fn the_token_never_goes_on_the_connectors_command_line() {
        let token = "eyJhIjoiYWJjIiwidCI6ImRlZiIsInMiOiJnaGkifQ-a-real-looking-token-0123456789"; // WHAT: a token. WHY: looked for below.
        let command = connector_command(std::path::Path::new("cloudflared"), token); // WHAT: how the connector would be started. WHY: inspected, never run.
        let args: Vec<String> = command.get_args().map(|a| a.to_string_lossy().into_owned()).collect(); // WHAT: its command line. WHY: anybody on the computer can read it.
        assert!(!args.iter().any(|a| a.contains(token)), "the token is on the command line: {args:?}");
        assert!(!args.iter().any(|a| a == "--token"), "{args:?}");
        let env: Vec<(String, String)> = command
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
            .collect(); // WHAT: what it is handed in its environment. WHY: where the token goes instead.
        assert!(env.contains(&("TUNNEL_TOKEN".to_string(), token.to_string())), "{env:?}");
    }

    #[test]
    fn a_connector_changed_after_it_was_checked_is_never_run() {
        let dir = scratch(&format!("excalibur-tunnel-test-swapped-{}", std::process::id())); // WHAT: a folder of the test's own. WHY: as above.
        let _ = std::fs::create_dir_all(&dir); // WHAT: made. WHY: the stand-in goes in it.
        let path = dir.join("cloudflared"); // WHAT: where the connector would be. WHY: swapped below.
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap(); // WHAT: something that is not the checked connector. WHY: what a swap leaves.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt; // WHAT: Unix file modes. WHY: the stand-in must be runnable, or refusing it proves nothing.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap(); // WHAT: made runnable. WHY: as above.
        }
        let ran = run_once(&path, "a-token-long-enough-to-be-one-0123456789-abcdef"); // WHAT: an attempt to run it. WHY: must be refused before starting.
        let _ = std::fs::remove_dir_all(&dir); // WHAT: cleaned up. WHY: tests leave nothing.
        let started = match ran {
            Ok(mut child) => {
                let _ = child.wait(); // WHAT: the stand-in left to finish. WHY: no stray process after the test.
                true
            }
            Err(_) => false,
        }; // WHAT: whether it was started. WHY: checked below.
        assert!(!started, "a file that isn't the checked connector was started");
    }

    #[test]
    fn turning_it_off_leaves_nothing_behind() {
        let tunnel = Tunnel::default();
        assert_eq!(tunnel.state_now(), State::Off);
        tunnel.set(State::On { address: "drawings.example.com".into() });
        assert!(matches!(tunnel.state_now(), State::On { .. }));
        tunnel.stop();
        assert_eq!(tunnel.state_now(), State::Off);
        assert!(!tunnel.running());
    }

    #[test]
    fn every_state_says_something_a_person_can_act_on() {
        for state in [
            State::Off,
            State::Sealed,
            State::Starting,
            State::On { address: "a.b.com".into() },
            State::Unreachable { address: "a.b.com".into(), why: "timed out".into() },
            State::Trouble { why: "it would not start".into() },
        ] {
            let said = state.in_words();
            assert!(!said.is_empty());
            assert!(said.len() > 10, "{state:?} says too little: {said:?}");
        }
    }
}

#[cfg(test)]
mod what_the_panel_is_told {
    //! The shape the Office panel reads, rather than the state machine
    //! underneath it. A panel built against an older server must not fall
    //! over because a newer one invented a state it has never heard of, which
    //! is why these are flat booleans and not a tagged enum.

    use super::State;

    fn as_panel(state: &State) -> (bool, bool, bool, bool) {
        (
            matches!(state, State::Off),
            matches!(state, State::Sealed),
            matches!(state, State::Starting),
            matches!(state, State::On { .. }),
        )
    }

    #[test]
    fn exactly_one_of_them_is_ever_true() {
        for state in [
            State::Off,
            State::Sealed,
            State::Starting,
            State::On { address: "a.b.com".into() },
        ] {
            let (off, sealed, starting, on) = as_panel(&state);
            let how_many = [off, sealed, starting, on].iter().filter(|x| **x).count();
            assert_eq!(how_many, 1, "{state:?} lit {how_many} lamps");
        }
    }

    #[test]
    fn trouble_and_unreachable_light_none_of_them() {
        // Deliberate: both are "not working", and the panel shows the reason
        // rather than a lamp. A panel that treated either as On would tell
        // somebody remote access works when it does not.
        for state in [
            State::Trouble { why: "it would not start".into() },
            State::Unreachable { address: "a.b.com".into(), why: "timed out".into() },
        ] {
            let (off, sealed, starting, on) = as_panel(&state);
            assert!(!on, "{state:?} must never read as working");
            assert!(!off && !sealed && !starting, "{state:?}");
            assert!(!state.in_words().is_empty(), "and it must say why");
        }
    }

    #[test]
    fn unreachable_says_where_to_go_and_look() {
        // The failure that actually happens: the connector runs, and the
        // hostname in Cloudflare points somewhere else. Nothing in this
        // program can fix that, so the message has to send them to the place
        // that can.
        let said = State::Unreachable {
            address: "drawings.example.com".into(),
            why: "timed out".into(),
        }
        .in_words();
        assert!(said.contains("drawings.example.com"));
        assert!(said.contains("Cloudflare"), "{said}");
    }
}
