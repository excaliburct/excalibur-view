//! The publisher's release feed.
//!
//! One place on the internet says what the newest Hyperview is: a releases
//! page on GitHub, where each release carries the two programs and a
//! `release.json` describing them, each description signed by the release
//! key. A company's server reads it and passes new versions on to its own
//! seats; a seat with no server reads it for itself.
//!
//! Nothing here decides whether a build is trustworthy. This only fetches.
//! What arrives is checked against the keys compiled into the running program
//! by [`crate::update::Trusted::check`] before anything is done with it, so a
//! feed that was taken over could serve only builds that were already signed
//! by the publisher's own key.
//!
//! Nothing goes the other way. The feed is asked what exists and is sent
//! nothing about who is asking: not a company name, not a drawing, not a count
//! of seats.

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::update::{compare, Channel, Release};

/// The file on every release that describes it.
pub const MANIFEST: &str = "release.json";

/// What the viewer is published as, on the platform this copy was built for.
///
/// The platform is part of what is signed, so this is not a label. A
/// signature over the Windows build cannot be passed off as a signature over
/// the Mac one, whatever a feed claims about it.
pub const APP_PLATFORM: &str = if cfg!(target_os = "windows") {
    "windows-x64"
} else if cfg!(target_os = "macos") {
    // One string for both processors, because one download holds both. A Mac
    // never has to know which kind of Mac it is.
    "macos-universal"
} else {
    "linux-x64"
};

/// What the server is published as. A different platform string, so a
/// signature over one program can never be passed off as a signature over the
/// other: the platform is part of what is signed.
pub const SERVER_PLATFORM: &str = if cfg!(target_os = "windows") {
    "windows-x64-server"
} else if cfg!(target_os = "macos") {
    "macos-universal-server"
} else {
    "linux-x64-server"
};

/// What the Office Server's Citadel cell is published as, beside the
/// server in the same release and signed with the same key. Its own platform
/// string, so a signature over the cell can never pass for one over the
/// server, and a server older than the cell never mistakes it for itself.
pub const CITADEL_PLATFORM: &str = "windows-x64-citadel"; // WHAT: the cell's part. WHY: only Windows runs the cell

/// Every platform a viewer is published for. The publisher's tools walk this;
/// a running program only ever cares about its own, which is [`APP_PLATFORM`].
pub const PLATFORMS: [&str; 3] = ["windows-x64", "macos-universal", "linux-x64"];

/// What a seat that does not name a platform is taken to be.
///
/// Every copy that asks without saying was built when there was only one
/// platform to be, so it is a Windows one. This is not a default in the sense
/// of a preference; it is the only thing silence can mean, and it has to keep
/// meaning that for as long as any of those copies is still running.
pub const ASSUMED_PLATFORM: &str = "windows-x64";

/// Larger than any build will be, and small enough that a feed serving
/// something enormous cannot fill a server's disk.
const LARGEST: u64 = 400 * 1024 * 1024;

/// What one release says about itself.
///
/// There are two shapes of this, and both have to be read. Every copy
/// installed before there was a Mac build reads `app` and `server` and knows
/// nothing else, so those two stay exactly where they were: take them away
/// and every one of those copies is stranded on the version it has, with no
/// way to be told otherwise. `apps` and `servers` are what a copy that knows
/// about more than one platform reads, and they hold *every* build in the
/// release, the Windows one included.
///
/// So a release written today says the same thing twice, deliberately. Use
/// [`Published::app_for`] and [`Published::server_for`] rather than reaching
/// for a field, and the two shapes stay one problem instead of two.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Published {
    /// The viewer, for an older copy that reads nothing else.
    pub app: Release,
    /// Every viewer in this release, for a copy that knows to look.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub apps: Vec<Release>,
    /// The server, when this release includes one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<Release>,
    /// Every server in this release.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<Release>,
}

impl Published {
    /// Every viewer in this release, whichever shape it was written in.
    pub fn viewers(&self) -> Vec<&Release> {
        if self.apps.is_empty() {
            vec![&self.app]
        } else {
            self.apps.iter().collect()
        }
    }

    /// Every server in this release, whichever shape it was written in.
    pub fn services(&self) -> Vec<&Release> {
        if self.servers.is_empty() {
            self.server.iter().collect()
        } else {
            self.servers.iter().collect()
        }
    }

    /// The viewer for one platform, or `None` when this release has none for
    /// it — which is the ordinary answer on the day a platform is added, and
    /// means "nothing new for you", not "something is wrong".
    pub fn app_for(&self, platform: &str) -> Option<&Release> {
        self.viewers().into_iter().find(|r| r.platform == platform)
    }

    /// The server for one platform.
    pub fn server_for(&self, platform: &str) -> Option<&Release> {
        self.services().into_iter().find(|r| r.platform == platform)
    }
}

/// The newest release a feed has for a channel, and where its files are.
#[derive(Clone, Debug)]
pub struct Found {
    pub published: Published,
    files: Vec<(String, String)>,
}

impl Found {
    /// Where to download one of the release's files, by the name its manifest
    /// gives it.
    pub fn url(&self, name: &str) -> Option<&str> {
        self.files
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, url)| url.as_str())
    }
}

#[derive(Deserialize)]
struct Listed {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    name: String,
    browser_download_url: String,
}

pub struct Feed {
    url: String,
    agent: ureq::Agent,
}

impl Feed {
    /// `url` is the releases list, e.g.
    /// `https://api.github.com/repos/<owner>/<repo>/releases`.
    pub fn new(url: &str) -> Feed {
        Feed {
            url: url.trim().to_string(),
            agent: crate::web::agent()
                .timeout_connect(Duration::from_secs(10))
                .timeout_read(Duration::from_secs(300))
                .user_agent(concat!("excalibur-hyperview/", env!("CARGO_PKG_VERSION")))
                .build(),
        }
    }

    /// The newest release on a channel, or `None` when there is nothing.
    ///
    /// A stable follower sees only full releases whose manifest says stable.
    /// A preview follower — the shop trying the next one first — sees
    /// prereleases too.
    pub fn newest(&self, channel: Channel) -> Result<Option<Found>, String> {
        crate::web::outbound("Checking for a new version", &self.url)?;
        let listed: Vec<Listed> = self
            .agent
            .get(&self.url)
            .set("Accept", "application/vnd.github+json")
            .query("per_page", "20")
            .call()
            .map_err(|e| format!("the release feed did not answer: {e}"))?
            .into_json()
            .map_err(|e| format!("the release feed's answer is not readable: {e}"))?;
        choose(listed, channel, |url| self.text(url))
    }

    /// Downloads one file off a release.
    pub fn fetch(&self, url: &str) -> Result<Vec<u8>, String> {
        crate::web::outbound("Downloading a new version", url)?;
        let response = self
            .agent
            .get(url)
            .set("Accept", "application/octet-stream")
            .call()
            .map_err(|e| format!("could not download {url}: {e}"))?;
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(LARGEST)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("the download stopped part way: {e}"))?;
        Ok(bytes)
    }

    fn text(&self, url: &str) -> Result<String, String> {
        let bytes = self.fetch(url)?;
        String::from_utf8(bytes).map_err(|_| "a release manifest is not text".to_string())
    }
}

/// The choosing, apart from the fetching, so it can be tested without a
/// network.
fn choose(
    listed: Vec<Listed>,
    channel: Channel,
    read: impl Fn(&str) -> Result<String, String>,
) -> Result<Option<Found>, String> {
    let mut candidates: Vec<Listed> = listed
        .into_iter()
        .filter(|r| !r.draft)
        .filter(|r| channel == Channel::Preview || !r.prerelease)
        .filter(|r| r.assets.iter().any(|a| a.name == MANIFEST))
        .collect();
    // Newest by version, not by the order the page happens to list them in.
    candidates.sort_by(|a, b| compare(bare(&b.tag_name), bare(&a.tag_name)));

    for listed in candidates {
        let Some(manifest) = listed.assets.iter().find(|a| a.name == MANIFEST) else {
            continue;
        };
        let text = read(&manifest.browser_download_url)?;
        let published: Published = match serde_json::from_str(&text) {
            Ok(p) => p,
            // One broken release does not hide an older good one.
            Err(_) => continue,
        };
        if channel == Channel::Stable && published.app.channel != Channel::Stable {
            continue;
        }
        let files = listed
            .assets
            .into_iter()
            .map(|a| (a.name, a.browser_download_url))
            .collect();
        return Ok(Some(Found { published, files }));
    }
    Ok(None)
}

fn bare(tag: &str) -> &str {
    tag.trim().trim_start_matches(['v', 'V'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str, channel: Channel) -> Release {
        for_platform(version, channel, APP_PLATFORM)
    }

    fn for_platform(version: &str, channel: Channel, platform: &str) -> Release {
        Release {
            version: version.into(),
            channel,
            platform: platform.into(),
            published: "2026-09-21T12:00:00Z".into(),
            notes: String::new(),
            download: "Hyperview.exe".into(),
            bytes: 1,
            digest: "00".into(),
            signature: "00".into(),
            key: "mesafab-2026".into(),
            minimum_api_version: 1,
        }
    }

    fn listed(tag: &str, prerelease: bool) -> Listed {
        Listed {
            tag_name: tag.into(),
            draft: false,
            prerelease,
            assets: vec![
                Asset {
                    name: MANIFEST.into(),
                    browser_download_url: format!("https://feed/{tag}/release.json"),
                },
                Asset {
                    name: "Hyperview.exe".into(),
                    browser_download_url: format!("https://feed/{tag}/Hyperview.exe"),
                },
            ],
        }
    }

    fn reader(url: &str) -> Result<String, String> {
        let tag = url.split('/').nth(3).unwrap_or("");
        let version = bare(tag);
        let channel = if version.ends_with('9') {
            Channel::Preview
        } else {
            Channel::Stable
        };
        Ok(serde_json::to_string(&Published {
            app: release(version, channel),
            apps: Vec::new(),
            server: None,
            servers: Vec::new(),
        })
        .unwrap())
    }

    #[test]
    fn the_newest_by_version_wins_not_the_first_listed() {
        let found = choose(
            vec![listed("v0.4.1", false), listed("v0.10.0", false), listed("v0.5.0", false)],
            Channel::Stable,
            reader,
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.published.app.version, "0.10.0");
        assert_eq!(found.url("Hyperview.exe"), Some("https://feed/v0.10.0/Hyperview.exe"));
    }

    #[test]
    fn a_stable_follower_never_sees_a_prerelease() {
        let found = choose(
            vec![listed("v0.4.1", false), listed("v0.5.9", true)],
            Channel::Stable,
            reader,
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.published.app.version, "0.4.1");
    }

    #[test]
    fn the_shop_trying_things_first_does_see_it() {
        let found = choose(
            vec![listed("v0.4.1", false), listed("v0.5.9", true)],
            Channel::Preview,
            reader,
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.published.app.version, "0.5.9");
    }

    #[test]
    fn a_release_whose_manifest_says_preview_is_not_stable_just_because_it_was_not_ticked() {
        // v0.4.9 is a full release on the page, but its signed manifest says
        // preview. The signature decides, not the checkbox.
        let found = choose(
            vec![listed("v0.4.1", false), listed("v0.4.9", false)],
            Channel::Stable,
            reader,
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.published.app.version, "0.4.1");
    }

    #[test]
    fn drafts_and_releases_without_a_manifest_are_ignored() {
        let mut draft = listed("v0.9.0", false);
        draft.draft = true;
        let mut bare_release = listed("v0.8.0", false);
        bare_release.assets.retain(|a| a.name != MANIFEST);
        let found = choose(
            vec![draft, bare_release, listed("v0.4.1", false)],
            Channel::Stable,
            reader,
        )
        .unwrap()
        .unwrap();
        assert_eq!(found.published.app.version, "0.4.1");
    }

    #[test]
    fn an_empty_feed_is_nothing_new_not_an_error() {
        assert!(choose(Vec::new(), Channel::Stable, reader).unwrap().is_none());
    }

    // ---- the two shapes a manifest comes in ---------------------------------

    /// The shape every copy shipped before there was a Mac build writes and
    /// reads: one `app`, no list. It has to keep working exactly as it did,
    /// because those copies cannot be told otherwise.
    #[test]
    fn a_manifest_from_before_there_were_two_platforms_still_reads() {
        let text = r#"{"app":{"version":"0.6.3","channel":"stable","platform":"windows-x64",
            "published":"2026-09-21T12:00:00Z","notes":"","download":"Hyperview.exe",
            "bytes":1,"digest":"00","signature":"00","key":"mesafab-2026",
            "minimum_api_version":1}}"#;
        let published: Published = serde_json::from_str(text).unwrap();
        assert!(published.apps.is_empty());
        assert_eq!(published.viewers().len(), 1);
        assert_eq!(published.app_for("windows-x64").unwrap().version, "0.6.3");
        assert!(published.app_for("macos-universal").is_none());
    }

    #[test]
    fn a_release_carrying_two_platforms_hands_each_one_its_own() {
        let published = Published {
            app: for_platform("0.7.0", Channel::Stable, "windows-x64"),
            apps: vec![
                for_platform("0.7.0", Channel::Stable, "windows-x64"),
                for_platform("0.7.0", Channel::Stable, "macos-universal"),
            ],
            server: None,
            servers: Vec::new(),
        };
        assert_eq!(published.app_for("windows-x64").unwrap().platform, "windows-x64");
        assert_eq!(published.app_for("macos-universal").unwrap().platform, "macos-universal");
        assert!(published.app_for("linux-x64").is_none());
        assert_eq!(published.viewers().len(), 2);
    }

    /// Written by today's publisher, read by a copy that only knows `app`.
    #[test]
    fn an_old_copy_reading_a_new_manifest_finds_the_windows_build() {
        let published = Published {
            app: for_platform("0.7.0", Channel::Stable, "windows-x64"),
            apps: vec![
                for_platform("0.7.0", Channel::Stable, "windows-x64"),
                for_platform("0.7.0", Channel::Stable, "macos-universal"),
            ],
            server: None,
            servers: Vec::new(),
        };
        let text = serde_json::to_string(&published).unwrap();

        // What an older copy's narrower struct sees.
        #[derive(serde::Deserialize)]
        struct AsItWasRead {
            app: Release,
        }
        let old: AsItWasRead = serde_json::from_str(&text).unwrap();
        assert_eq!(old.app.platform, "windows-x64");
        assert_eq!(old.app.version, "0.7.0");
    }

    /// The empty list is left out rather than written as `[]`, so a manifest
    /// for one platform is byte-for-byte what it always was.
    #[test]
    fn a_one_platform_manifest_is_written_the_way_it_always_was() {
        let published = Published {
            app: for_platform("0.6.3", Channel::Stable, "windows-x64"),
            apps: Vec::new(),
            server: None,
            servers: Vec::new(),
        };
        let text = serde_json::to_string(&published).unwrap();
        assert!(!text.contains("apps"), "{text}");
        assert!(!text.contains("servers"), "{text}");
    }

    #[test]
    fn this_build_asks_for_its_own_platform_and_no_other() {
        assert!(PLATFORMS.contains(&APP_PLATFORM));
        assert!(SERVER_PLATFORM.starts_with(APP_PLATFORM));
        assert_ne!(APP_PLATFORM, SERVER_PLATFORM);
    }
}
