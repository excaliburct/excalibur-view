//! Running one.
//!
//! This is the whole server as one call, because there are two ways it gets
//! started and they must be the same server. A shop that runs
//! `hyperview-server` on a Linux box and a shop where somebody ticked "host the
//! drawings on this computer" in the program are running identical code; the
//! only difference is which process it is inside.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::api::{self, Server};
use crate::{Config, Store, VERSION};

/// A running server, and the things worth knowing about it.
pub struct Serving {
    /// Where it actually ended up listening, which is not always where it was
    /// asked to: port zero means the operating system picks one.
    pub address: SocketAddr,
    pub name: String,
    /// Whether anybody has set it up yet.
    pub claimed: bool,
}

impl Serving {
    /// The address to hand to somebody at another desk. Never `0.0.0.0`, which
    /// means "every address on this machine" to the machine and nothing at all
    /// to anybody typing it.
    #[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
    pub fn url(&self) -> String {
        if self.address.ip().is_unspecified() {
            match local_address() {
                Some(mine) => format!("http://{mine}:{}", self.address.port()),
                None => format!("http://localhost:{}", self.address.port()),
            }
        } else {
            format!("http://{}", self.address)
        }
    }
}

/// Starts a server and serves until told to stop.
///
/// `ready` is called once, as soon as it is listening, with where it landed.
/// That is how the program that started it knows what address to show somebody
/// without guessing or racing it.
///
/// # Errors
///
/// Fails when the address can't be listened on or serving stops with an error.
pub async fn serve(
    config: Config,
    stop: impl std::future::Future<Output = ()> + Send + 'static,
    ready: impl FnOnce(&Serving),
) -> Result<()> {
    std::fs::create_dir_all(&config.data)
        .with_context(|| format!("making {}", config.data.display()))?;
    crate::offsite::check_settings(&config.data, config.offsite_work.as_deref(), config.offsite_target.as_deref(), &config.offsite_at).context("the off-site backup's settings")?; // WHAT: refuse to start on a backup setting that doesn't read. WHY: a typo found now, not by a backup that never runs.
    #[cfg(windows)]
    if config.replace_self {
        if let Some(home) = config.data.parent() {
            crate::lockdown::lock_down(home); // WHAT: the installed service's folder, program and drawings made its own, every start. WHY: an office already installed is fixed by its next update, not only a new one.
        }
    }
    let store = Store::open(&config.database(), &config.blobs())?;

    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("listening on {}", config.listen))?;
    let address = listener.local_addr().unwrap_or(config.listen);
    let https_only = match &config.https_only {
        Ok(on) => *on,
        Err(v) => anyhow::bail!("HYPERVIEW_HTTPS_ONLY is {v:?}; it takes on or off"), // WHAT: refuse to start. WHY: a typo in a security switch is found now, not by a seat that was never turned away
    };
    if https_only && !config.tls {
        anyhow::bail!("HYPERVIEW_HTTPS_ONLY=on needs HTTPS, but HYPERVIEW_TLS is off; turn one of them back"); // WHAT: refuse to start. WHY: HTTPS-only with no HTTPS would turn every seat away
    }

    let secure = if config.tls {
        Some(secure_settings(&config.data, address)?) // WHAT: the certificate and TLS settings, before anything is served. WHY: a server that can't keep its certificate private doesn't start half-secure
    } else {
        None
    };
    let server = Arc::new(Server::new(store, config));
    let secure = secure.map(|(settings, pin)| secured(&server, address.port(), pin, settings)); // WHAT: the port and pin noted, the TLS settings kept. WHY: served with below
    let claimed = !server.store.is_empty().unwrap_or(false);
    let name = api::installation_name(&server);

    // Remote access comes back by itself after a restart. A shop that turned
    // it on once should not find their jobsite access gone because the box
    // rebooted overnight; and a sealed server refuses here exactly as it
    // refuses everywhere else.
    if let Some((token, hostname)) = crate::tunnel::held(&server.store) {
        tracing::info!("remote access is on at {hostname}");
        crate::tunnel::start(&server.tunnel, &server.config.data, token, hostname);
    }

    // Answering for itself on the local network, so nobody in the office has to
    // be told an IP address. It is its own thread rather than a task because it
    // is a blocking socket that lives as long as the process, and it is not
    // worth failing a server over: a machine where something else already holds
    // the port simply will not be found automatically.
    let announcing = Arc::clone(&server);
    let port = address.port();
    // Settled before the thread starts, so the first question does not race
    // two of them into making one each.
    let id = api::installation_id(&server);
    std::thread::Builder::new()
        .name("hyperview-announce".into())
        .spawn(move || {
            hub::discover::answer_for(|asker| {
                let host = hub::discover::facing(asker.ip()).map_or_else(|| "localhost".into(), |ip| ip.to_string()); // WHAT: the address this computer faces the asker on, else localhost. WHY: the answer it is told to use
                hub::discover::Answer {
                    name: api::installation_name(&announcing),
                    url: format!("http://{host}:{port}"),
                    version: VERSION.to_string(),
                    api_version: hub::API_VERSION,
                    // Read fresh every time, so a server that is set up while
                    // somebody is looking stops advertising itself as free.
                    ready: !announcing.store.is_empty().unwrap_or(false),
                    id: id.clone(),
                }
            });
        })
        .ok();

    let serving = Serving {
        address,
        name: name.clone(),
        claimed,
    };
    if server.config.check_for_updates {
        tokio::spawn(crate::updates::keep_up_to_date(Arc::clone(&server)));
    }
    #[cfg(windows)]
    if server.config.replace_self {
        tokio::spawn(keep_the_version_before(Arc::clone(&server))); // WHAT: the version before kept a day, then let go. WHY: critical 4: a rollback needs it, and it was deleted at every start
    }
    tokio::spawn(crate::citadel::watch(Arc::clone(&server))); // WHAT: looks at the Citadel cell's port every five minutes. WHY: only while the licence switches the citadel part on; its version and pin
    crate::cell::start(&server); // WHAT: the Citadel cell started and watched, when the licence lists it and its program is here. WHY: Citadel runs inside the Office Server (docs/OFFICE-ENROLLMENT.md §6a)
    if server.config.offsite_target.is_some() {
        if let Some(work) = server.config.offsite_work.as_deref() { crate::offsite::lock_work(std::path::Path::new(work)); } // WHAT: the work folder made the system's, administrators' and this server's only (Windows). WHY: it holds the programs run on the data and the bucket's key.
        tokio::spawn(crate::offsite::nightly(Arc::clone(&server))); // WHAT: tonight's off-site backup, once a night. WHY: off until HYPERVIEW_OFFSITE_TARGET is set.
    }

    let app = api::router(server)
        .layer(axum::middleware::from_fn(api::small_bodies_read_first)) // WHAT: a small body read whole before any door looks at it. WHY: a refusal sent with the body unread is lost to a reset on a Mac (GitHub's macOS tests)
        .layer(axum::middleware::map_request(|request: axum::extract::Request| async move {
            request.map(|body| axum::body::Body::new(tower_http::timeout::TimeoutBody::new(crate::tls::BODY_IDLE, body))) // WHAT: every request body with an idle limit. WHY: a body that stops arriving gives its connection back
        }))
        .layer(tower_http::trace::TraceLayer::new_for_http());
    ready(&serving);
    if let Some(settings) = secure {
        if https_only {
            tracing::info!("{name} is serving the Excalibur View API on {}/api/v1, HTTPS only (plain HTTP from this machine alone)", serving.url());
        } else {
            tracing::info!("{name} is serving the Excalibur View API on {}/api/v1, HTTPS and plain HTTP on the one port", serving.url());
        }
        let plain = if https_only { crate::tls::Plain::ThisMachineOnly } else { crate::tls::Plain::Everyone }; // WHAT: who may still use plain HTTP. WHY: the switch above
        crate::tls::serve(listener, settings, app, stop, plain).await; // WHAT: HTTPS and plain HTTP on the API's port. WHY: an update needs no new firewall rule or tunnel rule
        tracing::info!("stopped cleanly");
        return Ok(());
    }

    tracing::info!("{name} is serving the Excalibur View API on {}/api/v1", serving.url());
    // With connect info, so the presence ledger can say which machine a seat
    // is — "who is connected and from where" needs the address, and on a shop
    // LAN there is no proxy header to read it from.
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
        .with_graceful_shutdown(stop)
        .await
        .context("serving")?;
    tracing::info!("stopped cleanly");
    Ok(())
}

/// Notes that HTTPS answers on `port` with the certificate whose pin is
/// `pin`, and hands back the TLS settings to serve with.
fn secured(server: &Server, port: u16, pin: String, settings: Arc<rustls::ServerConfig>) -> Arc<rustls::ServerConfig> {
    let _ = server.secure_port.set(port); // WHAT: HTTPS answers on the API's own port. WHY: /health offers it; a seat moves there
    let _ = server.pin.set(pin); // WHAT: the certificate's pin. WHY: device requests are bound to it (`crate::devices`)
    settings
}

/// The TLS settings, with the certificate made the first time. Its pin goes
/// in the log so a person can compare it with a seat's.
fn secure_settings(data: &std::path::Path, at: SocketAddr) -> Result<(Arc<rustls::ServerConfig>, String)> {
    let (cert, key) = crate::tls::load_or_make(data).map_err(anyhow::Error::msg)?; // WHAT: the server's own certificate. WHY: made once, kept after
    let config = crate::tls::server_config(&cert, &key).map_err(anyhow::Error::msg)?; // WHAT: the TLS settings. WHY: a damaged pair stops the start, saying so
    let pin = crate::tls::pin(&cert).map_err(anyhow::Error::msg)?; // WHAT: its pin. WHY: logged for a person to compare
    tracing::info!("HTTPS on {at}; certificate pin {} (View); fingerprint {} (Excalibur CAD)", hub::pinned::short(&pin), crate::tls::fingerprint(&cert).unwrap_or_default()); // WHAT: said once at start, both ways the programs show it. WHY: a person compares what their program shows with this
    Ok((config, pin)) // WHAT: the settings and the pin. WHY: the pin binds device requests to this server
}

/// This machine's address on the network it is actually on. Asked of the
/// operating system rather than guessed from a list of interfaces, because a
/// shop machine has four of those and only one of them is the answer.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn local_address() -> Option<std::net::IpAddr> {
    // Nothing is sent: connecting a UDP socket only picks a route.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("192.168.1.1:9").ok()?;
    let mine = socket.local_addr().ok()?.ip();
    (!mine.is_unspecified() && !mine.is_loopback()).then_some(mine)
}

/// Ctrl-C, or the service manager asking it to stop.
pub async fn on_a_signal() {
    let interrupt = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        () = interrupt => {}, // WHAT: Ctrl+C. WHY: stops the server
        () = terminate => {}, // WHAT: the service being stopped. WHY: likewise
    }
    tracing::info!("stopping");
}

/// Makes the first account when a server is started with nothing on it and
/// nobody is going to be sitting in front of the program to set it up — the
/// headless case, where somebody installed this on a Linux box.
///
/// # Errors
///
/// Fails when the database can't be read or written.
pub fn first_administrator(store: &Store) -> Result<String> {
    let password = crate::auth::readable_secret();
    let hashed = crate::auth::hash_password(&password)?;
    let now = api::now();
    store.with(|db| {
        db.execute(
            "INSERT INTO people (id, name, email, password, role, created)
             VALUES (?1, ?2, ?3, ?4, 'admin', ?5)",
            rusqlite::params![
                api::fresh_id("usr"),
                "Administrator",
                "admin@localhost",
                hashed,
                now
            ],
        )?;
        Ok(())
    })?;
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_to_hand_somebody_is_never_the_anywhere_one() {
        let serving = Serving {
            address: "0.0.0.0:8714".parse().unwrap(),
            name: "Example Steel".into(),
            claimed: true,
        };
        let url = serving.url();
        assert!(!url.contains("0.0.0.0"), "{url} is not something to type");
        assert!(url.ends_with(":8714"));
    }

    #[test]
    fn a_server_told_exactly_where_to_listen_says_exactly_that() {
        let serving = Serving {
            address: "192.168.1.40:8714".parse().unwrap(),
            name: "Example Steel".into(),
            claimed: true,
        };
        assert_eq!(serving.url(), "http://192.168.1.40:8714");
    }
}

/// Lets the version before the last update go once this one has run a day,
/// looking at the start and every hour after. Windows only: only the
/// Windows service replaces itself.
#[cfg(windows)]
async fn keep_the_version_before(server: Arc<crate::api::Server>) {
    loop {
        let program = crate::winsvc::installed_program(); // WHAT: the installed program, by its known place. WHY: the kept copy is beside it; never a path the running program reports about itself
        crate::kept::tidy(&server.store, &program, crate::VERSION, time::OffsetDateTime::now_utc()); // WHAT: kept, or let go after a day. WHY: crate::kept
        tokio::time::sleep(std::time::Duration::from_secs(60 * 60)).await; // WHAT: an hour. WHY: the day ends while the server runs
    }
}
