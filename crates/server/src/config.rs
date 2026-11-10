//! What the administrator sets.

use std::net::SocketAddr;
use std::path::PathBuf;

/// The port a Hyperview server serves on unless somebody says otherwise. Not
/// the port it is *found* on — that is [`hub::discover::PORT`], and the two are
/// deliberately different so a machine that cannot hold one can still do the
/// other.
pub const DEFAULT_PORT: u16 = 8714;

/// The port the same API is served on over HTTPS, with the server's own
/// certificate (`crate::tls`), beside the plain-HTTP one until every seat has
/// moved over.
pub const DEFAULT_SECURE_PORT: u16 = 8716; // WHAT: TCP 8716. WHY: beside 8714; not 8715, which is the UDP port seats find the server on, so a firewall list never reads two ways

#[derive(Clone, Debug)]
pub struct Config {
    /// What the company calls this installation. It shows in the client, so
    /// nobody marks up a drawing on the wrong server.
    pub name: String,
    pub listen: SocketAddr,
    /// Where the API is served over HTTPS too. None means plain HTTP only,
    /// which is how a test or a server embedded in something else starts.
    pub secure_listen: Option<SocketAddr>, // WHAT: HYPERVIEW_SECURE_LISTEN, or the listen address on 8716. WHY: sign-ins and drawings encrypted, the seats pinning the certificate
    /// Everything the server keeps: the database and the drawings.
    pub data: PathBuf,
    /// Largest drawing set it will accept, in bytes.
    pub largest_upload: u64,
    /// Hold every seat on this version. Empty means whatever is newest.
    pub pin_version: Option<String>,
    /// Where releases come from when this server mirrors rather than hosts.
    pub release_feed: Option<String>,
    /// A token for that feed, when it is a private repository.
    pub release_token: Option<String>,
    /// Browser origins allowed to call the API. Empty means none, which is the
    /// right default for a server whose clients are desktop programs.
    pub allow_origins: Vec<String>,
    /// The key whoever looks after this box uses to reach its maintenance
    /// routes. **None means those routes do not exist** — not that they are
    /// open. A shop that has not asked to be looked after should not quietly
    /// grow a remote channel into its drawings.
    pub maintenance_key: Option<String>,
    /// Which release channel this installation follows: `stable` or `preview`.
    /// An administrator's choice in the program wins over this.
    pub channel: String,
    /// Whether to read the publisher's feed at all. Off unless whatever
    /// started the server turns it on, so a test or a server embedded in
    /// something else never goes out to the internet by surprise.
    pub check_for_updates: bool,
    /// Whether this server may replace its own program file with a newer
    /// signed one and restart. Only the installed Windows service does: it
    /// is the one with a service manager to start it again.
    pub replace_self: bool,
    /// Keys a plugin may be signed with besides the ones compiled in. Empty
    /// in every server that is started as a program — nothing reads it from a
    /// file, the command line or the environment — and set only by a test,
    /// which cannot hold the publisher's private key.
    pub plugin_keys_for_tests: Vec<(String, [u8; 32])>,
    /// Keys a licence may be signed with besides the ones compiled in. The
    /// same rule as `plugin_keys_for_tests`: never read from a file, the
    /// command line or the environment, set only by a test, so a test can
    /// switch Office Server parts on with a licence it really signed.
    pub license_keys_for_tests: Vec<(String, [u8; 32])>, // WHAT: a test's own licence key. WHY: parts are switched on only by a signed licence, in tests too
    /// Where the nightly off-site backup goes: a folder, or
    /// `rclone:REMOTE:bucket/folder`. None means the backup is off.
    pub offsite_target: Option<String>, // WHAT: HYPERVIEW_OFFSITE_TARGET. WHY: a copy on the same machine is not a backup (crate::offsite).
    /// The backup's own folder, apart from the data: age and rclone in its
    /// `bin`, the public key, rclone's settings, the spool and the log.
    pub offsite_work: Option<String>, // WHAT: HYPERVIEW_OFFSITE_WORK. WHY: the programs and the bucket's settings live outside what is backed up.
    /// The time of night the server runs it, on its own clock, 24-hour.
    pub offsite_at: String, // WHAT: HYPERVIEW_OFFSITE_AT, "01:30" unless set. WHY: after the office has gone home, before anyone is back.
}

impl Default for Config {
    fn default() -> Config {
        Config {
            name: "Excalibur View".into(),
            listen: SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT)),
            secure_listen: None, // WHAT: off by default. WHY: only a server started for real turns it on (from_environment)
            data: beside_the_program(),
            // Sixty drawing sets the size of a stamped structural package.
            largest_upload: 512 * 1024 * 1024,
            pin_version: None,
            release_feed: None,
            release_token: None,
            allow_origins: Vec::new(),
            maintenance_key: None,
            channel: "stable".into(),
            check_for_updates: false,
            replace_self: false,
            plugin_keys_for_tests: Vec::new(),
            license_keys_for_tests: Vec::new(), // WHAT: none. WHY: only the compiled-in keys sign a licence in a real server
            offsite_target: None, // WHAT: off. WHY: nothing leaves the box until someone sets it up.
            offsite_work: None, // WHAT: none. WHY: the same.
            offsite_at: "01:30".into(), // WHAT: half past one at night. WHY: the office is shut and the night's sync is done.
        }
    }
}

/// Where a server keeps its things when nobody has said.
///
/// Beside the program itself, not in whatever folder it happened to be
/// started from. Somebody who drops `hyperview-server.exe` in a folder on the
/// office server and double-clicks it should find the drawings in that same
/// folder afterwards — not in `C:\\Windows\\System32` because a shortcut had
/// a different "start in". When the program's own folder cannot be written
/// to, which is the ordinary case for `/usr/local/bin`, it falls back to the
/// working directory and says so by simply using it.
pub fn beside_the_program() -> PathBuf {
    let here = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from));
    if let Some(here) = here {
        let scratch = here.join(".hyperview-write-test");
        if std::fs::write(&scratch, b"x").is_ok() {
            let _ = std::fs::remove_file(&scratch);
            return here.join("hyperview-data");
        }
    }
    PathBuf::from("./hyperview-data")
}

impl Config {
    /// Reads the environment. Everything has a working default, so the first
    /// run needs no configuration at all.
    pub fn from_environment() -> Config {
        let mut config = Config::default();
        if let Ok(name) = std::env::var("HYPERVIEW_NAME") {
            if !name.trim().is_empty() {
                config.name = name;
            }
        }
        if let Ok(listen) = std::env::var("HYPERVIEW_LISTEN") {
            match listen.parse() {
                Ok(address) => config.listen = address,
                Err(_) => eprintln!(
                    "HYPERVIEW_LISTEN is not an address ({listen}); using {}",
                    config.listen
                ),
            }
        }
        config.secure_listen = Some(SocketAddr::new(config.listen.ip(), DEFAULT_SECURE_PORT)); // WHAT: HTTPS on 8715 beside the API's address. WHY: every real server serves it
        if let Ok(secure) = std::env::var("HYPERVIEW_SECURE_LISTEN") {
            match secure.trim() {
                "off" => config.secure_listen = None, // WHAT: turned off. WHY: a machine where 8716 is taken can still serve
                text => match text.parse() {
                    Ok(address) => config.secure_listen = Some(address), // WHAT: another address. WHY: the administrator's choice
                    Err(_) => eprintln!("HYPERVIEW_SECURE_LISTEN is not an address or off ({text}); using {:?}", config.secure_listen), // WHAT: said, and the default kept. WHY: the same as HYPERVIEW_LISTEN
                },
            }
        }
        if let Ok(data) = std::env::var("HYPERVIEW_DATA") {
            config.data = PathBuf::from(data);
        }
        if let Ok(size) = std::env::var("HYPERVIEW_MAX_UPLOAD_MB") {
            if let Ok(mb) = size.parse::<u64>() {
                config.largest_upload = mb * 1024 * 1024;
            }
        }
        config.maintenance_key = non_empty("HYPERVIEW_MAINTENANCE_KEY");
        if let Some(channel) = non_empty("HYPERVIEW_CHANNEL") {
            config.channel = channel;
        }
        config.pin_version = non_empty("HYPERVIEW_PIN_VERSION");
        config.release_feed = non_empty("HYPERVIEW_RELEASE_FEED");
        config.release_token = non_empty("HYPERVIEW_RELEASE_TOKEN");
        if let Some(origins) = non_empty("HYPERVIEW_ALLOW_ORIGINS") {
            config.allow_origins = origins
                .split(',')
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect();
        }
        config.offsite_target = non_empty("HYPERVIEW_OFFSITE_TARGET"); // WHAT: where the nightly backup goes. WHY: off when empty.
        config.offsite_work = non_empty("HYPERVIEW_OFFSITE_WORK"); // WHAT: the backup's own folder. WHY: its programs, key and log.
        if let Some(at) = non_empty("HYPERVIEW_OFFSITE_AT") {
            config.offsite_at = at; // WHAT: the time of night. WHY: checked as the server starts, so a typo is found then.
        }
        config
    }

    pub fn database(&self) -> PathBuf {
        self.data.join("hyperview.sqlite")
    }

    /// Drawings and chests, kept by the digest of their contents so the same
    /// file uploaded twice is stored once.
    pub fn blobs(&self) -> PathBuf {
        self.data.join("files")
    }

    pub fn releases(&self) -> PathBuf {
        self.data.join("releases")
    }
}

fn non_empty(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_with_nothing_configured_still_knows_where_to_listen() {
        let config = Config::default();
        assert_eq!(config.listen.port(), 8714);
        assert!(config.pin_version.is_none());
        // No browser may call it until somebody says which one, because the
        // clients are desktop programs and a wide-open origin list on a server
        // holding a company's drawings is nobody's idea of a default.
        assert!(config.allow_origins.is_empty());
        // And no maintenance channel until somebody sets a key for one.
        assert!(config.maintenance_key.is_none());
    }

    #[test]
    fn the_database_and_the_drawings_live_under_the_data_folder() {
        let config = Config {
            data: PathBuf::from("/srv/hyperview"),
            ..Config::default()
        };
        assert_eq!(config.database(), PathBuf::from("/srv/hyperview/hyperview.sqlite"));
        assert_eq!(config.blobs(), PathBuf::from("/srv/hyperview/files"));
    }
}
