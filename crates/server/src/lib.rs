#![recursion_limit = "256"]
//! The self-hosted Hyperview server.
//!
//! A company runs one of these. It holds the drawings, the markups people have
//! put on them, the shared tool chests, and the version of the program the
//! office is meant to be running. Everything it does is reachable over an open,
//! documented API, so an estimating system, an ERP, a spreadsheet or anything
//! else can ask it the same questions the viewer does and get the same answers.
//!
//! What it is deliberately not: the place the drawing lives. The PDF is the
//! file of record. This server keeps a copy of those exact bytes and a stream
//! of the annotation dictionaries that belong on them, and applying the second
//! to the first reproduces the drawing somebody would hand a general
//! contractor. Nothing is translated into a private format on the way in.
//!
//! It is built to be run by somebody who is not a systems administrator. One
//! binary, a SQLite file, a folder of drawings. No message queue, no container
//! orchestration, and nothing that stops working when the machine reboots.

pub mod api;
pub mod audit;
pub mod auth;
pub mod config;
pub mod fleet;
pub mod folders; // WHAT: groups, who sees which folder, and files that aren't drawings. WHY: the field and the shop see only the folders given to them.
pub mod license;
pub mod mcp;
pub mod notices;
pub mod openapi;
pub mod patience;
pub mod presence;
pub mod projects;
pub mod quantities;
pub mod run;
pub mod store;
pub mod tunnel;
pub mod updates;
pub mod winsvc;

pub use config::Config;
pub use store::Store;

/// The server's own build, which a client is told so a person can see at a
/// glance whether the two match.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The public keys a release must be signed by before this server will hold it
/// for anybody.
///
/// The same list a seat pins, and for the same reason: a company's server is
/// not a place where a build gets to become trusted. It checks the signature
/// when it takes a release, and every seat checks it again before it runs one.
/// An empty list refuses everything, which is how a build ships until a key is
/// compiled into it.
pub fn trusted() -> hub::update::Trusted {
    hub::update::Trusted::pinned(KEYS)
}

/// The keys a licence may be signed with: every release key, and the keys
/// made for licences alone.
///
/// Kept apart from [`trusted`] on purpose. A licence key lives where it can
/// sign at three in the morning without anybody, and that is only safe because
/// nothing but a licence is ever checked against it.
pub fn license_trust() -> hub::update::Trusted {
    let mut trust = hub::update::Trusted::pinned(KEYS);
    trust.keys.extend(hub::update::Trusted::pinned(LICENSE_KEYS).keys);
    trust
}

/// The keys a plugin may be signed with: every release key, and the keys made
/// for plugins alone. The same pair a seat checks against, so a plugin the
/// server takes is one every seat will load.
pub fn plugin_trust() -> hub::update::Trusted {
    plugin_trust_from(KEYS, PLUGIN_KEYS)
}

fn plugin_trust_from(releases: &[(&str, &str)], plugins: &[(&str, &str)]) -> hub::update::Trusted {
    let mut trust = hub::update::Trusted::pinned(releases);
    trust.keys.extend(hub::update::Trusted::pinned(plugins).keys);
    trust
}

#[cfg(test)]
mod keys {
    use ed25519_dalek::{Signer, SigningKey};

    /// A plugin sealed the way `publish.py sign-plugin` seals one.
    fn sealed(key: &SigningKey, name: &str) -> Vec<u8> {
        use sha2::Digest;
        let wasm = b"\0asm\x01\0\0\0 a plugin".to_vec();
        let sha = hub::update::hex(&sha2::Sha256::digest(&wasm));
        let signature = key.sign(&plugin_api::signing_payload("acme-checks", "1.0.0", &sha));
        let header = plugin_api::Header {
            id: "acme-checks".into(),
            version: "1.0.0".into(),
            name: "Acme Checks".into(),
            sha256: sha,
            bytes: wasm.len() as u64,
            key: name.into(),
            signature: hub::update::hex(&signature.to_bytes()),
        };
        plugin_api::seal(&header, &wasm)
    }

    #[test]
    fn a_plugin_key_seals_plugins_and_nothing_else() {
        let partner = SigningKey::from_bytes(&[9u8; 32]);
        let public = hub::update::hex(&partner.verifying_key().to_bytes());
        let plugins = [("partner-2026", public.as_str())];
        let file = sealed(&partner, "partner-2026");

        let for_plugins = super::plugin_trust_from(super::KEYS, &plugins);
        assert!(hub::plugin::check(&for_plugins, &file).is_ok(), "the plugin loads");
        for (name, _) in super::KEYS {
            assert!(for_plugins.key(name).is_some(), "plugins signed with {name} still load");
        }
        // Neither a release nor a license is checked against it.
        assert!(hub::plugin::check(&super::trusted(), &file).is_err());
        assert!(super::trusted().key("partner-2026").is_none());
        assert!(super::license_trust().key("partner-2026").is_none());
    }

    #[test]
    fn the_plugin_keys_this_build_trusts_sign_nothing_but_plugins() {
        let releases = super::trusted();
        let licenses = super::license_trust();
        let plugins = super::plugin_trust();
        for (name, _) in super::PLUGIN_KEYS {
            assert!(plugins.key(name).is_some(), "{name} signs plugins");
            assert!(releases.key(name).is_none(), "{name} must never sign a release");
            assert!(licenses.key(name).is_none(), "{name} must never sign a license");
        }
        assert_eq!(
            plugins.keys.len(),
            super::KEYS.len() + super::PLUGIN_KEYS.len(),
            "every key in both lists is well formed"
        );
    }
}

/// The same file the program compiles, read in rather than copied, so the
/// server and the seats can never disagree about whose signature counts. A
/// second list that somebody had to remember to update at release time is how
/// a server ends up refusing the build every seat would have taken.
mod pinned {
    #![allow(dead_code)]
    include!("../../hyperview/src/trust.rs");
}

pub use pinned::{HOME_CHANNEL, HOME_FEED, KEYS, LICENSE_FEED, LICENSE_KEYS, PLUGIN_KEYS};
