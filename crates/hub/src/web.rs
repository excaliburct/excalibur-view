//! How this program reaches anything over HTTPS.
//!
//! A connection trusts two lists of certificate authorities at once: the one
//! built into the program, and the one Windows keeps.
//!
//! The built-in list alone fails on a PC whose antivirus or office firewall
//! looks inside secure connections. Avast, ESET, Kaspersky, Bitdefender and
//! most office firewalls do it by presenting a certificate of their own,
//! which they added to Windows' list and nobody else's — so a program that
//! only knows the built-in list refuses every connection, the update check
//! included, and says nothing a person could act on.
//!
//! Windows' list alone fails on a freshly installed server, which fetches the
//! authorities it needs only when something asks it to and may not have
//! GitHub's yet.
//!
//! Both lists together, and neither of those happens. What is trusted is still
//! only a certificate authority someone chose to trust; and an update is
//! checked against the publisher's key regardless, so the connection was never
//! what made a download safe to run.

use std::sync::{Arc, OnceLock};

/// The HTTP client itself, for callers that want its error type.
pub use ureq;

/// A client builder that trusts both lists.
///
/// This does not know whether the installation is sealed — a builder is not a
/// connection. `outbound` is what a caller asks before reaching the outside
/// world, and every such caller must ask.
pub fn agent() -> ureq::AgentBuilder {
    ureq::AgentBuilder::new().tls_config(tls())
}

/// The one gate between this program and the outside world.
///
/// Anything leaving the company's own network asks first, names itself, and
/// takes no for an answer. Addresses inside the network — the office server
/// above all — are not the outside world and are never refused: a sealed seat
/// is a seat with the doors to the internet shut, not a seat that cannot work.
pub fn outbound(what: &str, url: &str) -> Result<(), String> {
    if crate::sealed::is_sealed() && !crate::sealed::is_inside(url) {
        return Err(crate::sealed::refusal(what));
    }
    Ok(())
}

/// Fetches a small signed file: a license renewal, and nothing larger.
///
/// Goes through [`outbound`] like everything else, so a sealed office never
/// makes the request at all. Nothing about the company goes out with it: the
/// address is the whole of the message, and the file that comes back is
/// checked against the program's own keys before it is believed.
pub fn fetch_small(what: &str, url: &str) -> Result<String, String> {
    use std::io::Read;
    /// Larger than any license will ever be, and small enough that a server
    /// pointed at something enormous does not fill its disk finding out.
    const LARGEST: u64 = 64 * 1024;

    outbound(what, url)?;
    let response = agent()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(30))
        .user_agent(concat!("excalibur-hyperview/", env!("CARGO_PKG_VERSION")))
        .build()
        .get(url)
        .call()
        .map_err(|e| format!("{url} did not answer: {e}"))?;
    let mut text = String::new();
    response
        .into_reader()
        .take(LARGEST)
        .read_to_string(&mut text)
        .map_err(|e| format!("could not read {url}: {e}"))?;
    Ok(text)
}

fn tls() -> Arc<rustls::ClientConfig> {
    static TLS: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    TLS.get_or_init(|| {
        let mut roots = rustls::RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
        };
        // Whatever Windows will not hand over is simply not added. The
        // built-in list is still there.
        if let Ok(windows) = rustls_native_certs::load_native_certs() {
            roots.add_parsable_certificates(windows);
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = rustls::ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .expect("the default protocol versions are supported by the default provider")
            .with_root_certificates(roots)
            .with_no_client_auth();
        Arc::new(config)
    })
    .clone()
}

/// The page a person downloads a release from, worked out from the feed the
/// program reads: `https://api.github.com/repos/<owner>/<repo>/releases`
/// becomes `https://github.com/<owner>/<repo>/releases/latest`. `None` for a
/// feed that is not a GitHub releases list.
pub fn download_page(feed: &str) -> Option<String> {
    let rest = feed.trim().strip_prefix("https://api.github.com/repos/")?;
    let mut parts = rest.trim_end_matches('/').split('/');
    let (owner, repo, releases) = (parts.next()?, parts.next()?, parts.next()?);
    if owner.is_empty() || repo.is_empty() || releases != "releases" || parts.next().is_some() {
        return None;
    }
    Some(format!("https://github.com/{owner}/{repo}/releases/latest"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_download_page_is_the_feed_in_a_browser() {
        assert_eq!(
            download_page("https://api.github.com/repos/excaliburct/hyperview-releases/releases")
                .as_deref(),
            Some("https://github.com/excaliburct/hyperview-releases/releases/latest")
        );
        assert_eq!(download_page("https://example.com/releases.json"), None);
        assert_eq!(download_page("https://api.github.com/repos/x/releases"), None);
    }

    #[test]
    fn both_lists_make_a_connection_config() {
        // Building it must not panic on a machine with no store at all.
        let config = tls();
        assert!(Arc::strong_count(&config) >= 2);
    }
}
