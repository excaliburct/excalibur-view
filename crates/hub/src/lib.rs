//! The Hyperview wire model: what a desktop, a self-hosted server and anybody
//! else's integration all agree on.
//!
//! Two rules shape everything here.
//!
//! **The PDF is still the file of record.** The server holds the drawing and a
//! stream of markups, and a markup on the wire is the *annotation dictionary
//! itself*, not some summary of it. Anything that came out of a drawing can be
//! put back into that drawing byte for byte. There is no lossy middle format
//! for fidelity to leak out of.
//!
//! **Nothing is estimated, on the wire either.** A quantity crossing this API
//! carries whether its sheet had a scale. A sheet with no scale reports its
//! measurements as unscaled and leaves them out of the totals, exactly as the
//! viewer does, so an integration cannot accidentally read a missing scale as
//! a zero.

pub mod device; // WHAT: device keys and the lines they sign. WHY: codeless linking to the Office Server (docs/OFFICE-ENROLLMENT.md)
pub mod discover;
pub mod license;
pub mod site;
pub mod sealed;
pub mod model;
pub mod plugin;
pub mod update;

#[cfg(feature = "client")]
pub mod client;

#[cfg(feature = "client")]
pub mod feed;

#[cfg(feature = "client")]
pub mod web;

#[cfg(feature = "client")]
pub mod pinned; // WHAT: HTTPS by a pinned certificate. WHY: Excalibur's own servers make their own certificates

#[cfg(feature = "client")]
pub mod keystore; // WHAT: where this computer keeps its device key. WHY: DPAPI on Windows, a 0600 file elsewhere (docs/OFFICE-ENROLLMENT.md §1)

#[cfg(feature = "client")]
pub mod office; // WHAT: the office server's address and pin, shared by every app on the PC. WHY: §2: the second app never asks again

#[cfg(feature = "client")]
pub mod secure; // WHAT: seats moved onto an Office Server's HTTPS and kept there. WHY: sign-ins and drawings encrypted, the certificate pinned

pub use model::*;

#[cfg(feature = "client")]
pub use client::Client;

/// The version of the wire protocol this build speaks. A server tells a client
/// what it speaks; a client that is too old is told so plainly rather than
/// being allowed to guess.
pub const API_VERSION: u32 = 1;

/// The path everything hangs off.
pub const API_ROOT: &str = "/api/v1";
