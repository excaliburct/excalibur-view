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

/// The page for one version's own release, `.../releases/tag/v<version>`,
/// and the latest release's page when no usable version is given.
///
/// "Latest" is the newest full release only. An office on the early channel
/// is offered early releases that "latest" skips, so sending them there
/// would hand them the version they already have.
pub fn release_page(feed: &str, version: Option<&str>) -> Option<String> {
    let latest = download_page(feed)?; // WHAT: the feed's latest-release page, or None for a feed that is not GitHub's. WHY: the tag page is built from the same place, and a feed with neither has no page at all.
    let version = version.map(|v| v.trim().trim_start_matches('v')).filter(|v| plain_version(v)); // WHAT: the version without a leading "v", kept only when it looks like a version. WHY: it goes into an address, so anything that could leave the releases page is dropped for "latest".
    match version {
        Some(v) => Some(format!("{}tag/v{v}", latest.trim_end_matches("latest"))), // WHAT: ".../releases/" plus "tag/v0.6.23". WHY: the release's tags are "v" and the version, as the publishing script makes them.
        None => Some(latest), // WHAT: the latest release. WHY: with no version known, the newest full release is the best page there is.
    }
}

/// A version as the releases are tagged: a digit first, then digits,
/// letters, dots and dashes, and not too long to be one.
fn plain_version(v: &str) -> bool {
    v.len() <= 40 // WHAT: a cap on its length. WHY: no real version comes close, and an address is no place for an essay.
        && v.starts_with(|c: char| c.is_ascii_digit()) // WHAT: starts with a number. WHY: every release is tagged by its number.
        && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') // WHAT: only characters a tag uses. WHY: a slash, "?" or "#" would point the browser somewhere else.
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

    // WHAT: Download goes to the offered version's own page. WHY: an early release is skipped by "latest", so an early-channel office sent there got the old version again.
    #[test]
    fn the_download_page_for_a_version_is_that_version_s_release() {
        let feed = "https://api.github.com/repos/excaliburct/hyperview-releases/releases"; // WHAT: the feed the program reads. WHY: the page is worked out from it.
        assert_eq!(
            release_page(feed, Some("0.6.23")).as_deref(),
            Some("https://github.com/excaliburct/hyperview-releases/releases/tag/v0.6.23")
        ); // WHAT: a known version opens its tag. WHY: that is the release the box offered.
        assert_eq!(
            release_page(feed, Some(" v0.7.0-beta.1 ")).as_deref(),
            Some("https://github.com/excaliburct/hyperview-releases/releases/tag/v0.7.0-beta.1")
        ); // WHAT: a leading "v" and spaces are not doubled into the tag. WHY: the tag is "v" once.
        assert_eq!(
            release_page(feed, None).as_deref(),
            Some("https://github.com/excaliburct/hyperview-releases/releases/latest")
        ); // WHAT: no version, the latest page. WHY: the fallback still gets somebody a copy.
        for odd in ["", "v", "../../evil", "0.6.23/../../x", "0.6.23?x=1", "0.6.23#top", "latest"] {
            assert_eq!(
                release_page(feed, Some(odd)).as_deref(),
                Some("https://github.com/excaliburct/hyperview-releases/releases/latest"),
                "{odd:?}"
            ); // WHAT: anything that is not a plain version falls back to latest. WHY: it would go into the address otherwise.
        }
        assert_eq!(release_page("https://example.com/releases.json", Some("0.6.23")), None); // WHAT: a feed that is not GitHub's has no page. WHY: the same as download_page.
    }

    #[test]
    fn both_lists_make_a_connection_config() {
        // Building it must not panic on a machine with no store at all.
        let config = tls();
        assert!(Arc::strong_count(&config) >= 2);
    }
}
