// The signing keys this build will accept an update from.
//
// This file is included verbatim by the release tool as well as compiled into
// the program, so the command that says a release is good and the seat that
// decides whether to install it are reading the same list. That is why it holds
// nothing but the list.

/// The signing keys this build will accept an update from, as
/// `(name, public key as 64 hex characters)`.
///
/// Compiled in on purpose. A Hyperview server hands out installers, and nothing
/// about running a server should be enough to decide what software six machines
/// run — a release signed by a key that is not in this list does not install,
/// whoever is serving it.
///
/// Adding one is deliberate: generate it with `hyperview-release keygen`, on the
/// publisher's own machine, and paste the public half here. The private half
/// never appears in this repository.
///
/// An empty list means this build refuses every update and says so, which is the
/// right way round: a program that installs anything when it has been told to
/// trust nothing is worse than one that installs nothing.
pub const KEYS: &[(&str, &str)] = &[
    // Made on the release PC on 2026-09-21. The private half is there and
    // nowhere else.
    ("mesafab-2026", "14d93acf4729859a3a7b01f458e118173a7a1859305193e84c3bd7513eb72790"),
];

/// The keys that may sign a licence, and nothing else.
///
/// A licence is signed the moment Square says an order is paid, by a service
/// that is always up, so it cannot be signed with a key from [`KEYS`]: those
/// never leave the release PC, because a release key can put code on every
/// office's machines. A key here can make a licence file and that is all. No
/// release, plugin or update is ever checked against this list; only the
/// server's licence check reads it, and it reads [`KEYS`] too, so every
/// licence signed on the PC before this list existed still checks out.
///
/// If one of these ever leaked, the harm is free licences, not software on
/// somebody's machine. The fix is a new key here in the next release and the
/// old one's licences signed again, which their servers fetch by themselves.
pub const LICENSE_KEYS: &[(&str, &str)] = &[
    // Made on the release PC on 2026-09-24. The private half is there and in
    // the licensing service's Cloudflare secrets, and nowhere else.
    ("excalibur-licences-2026", "5e537bbc4a3c6db7a3622ebe558855ad630392348477f5d86878ff278fa4a50b"),
];

/// The keys that may sign a plugin, and nothing else.
///
/// A plugin used to be signed with a key from [`KEYS`], which meant getting
/// the release key out every time somebody's plugin needed signing, and a key
/// that can put a new version of the program on every machine is not one to
/// be using for anything else. A key here can seal a plugin and that is all:
/// no release or license is ever checked against it.
///
/// If one of these leaked, the harm is a plugin nobody meant to trust, and a
/// plugin runs in the sandbox with nothing to reach and can only offer fixes
/// a person clicks. The fix is a new key in the next release and the office's
/// plugins signed again.
///
/// Plugins already signed with a key from [`KEYS`] keep loading. A partner
/// who writes plugins, like FabWire, can be given a key of their own here.
///
/// Make one with `publish.py make-key`, on the PC that will sign with it, and
/// paste the line it prints below.
pub const PLUGIN_KEYS: &[(&str, &str)] = &[];

/// Where new versions are published: a releases page on GitHub.
///
/// A company's server reads this and passes new versions on to its seats. A
/// seat with no server reads it itself. Two cases, worth telling apart:
///
/// A seat signed in to a company's own server takes its answer from **that
/// server**, always. A shop that has held its six seats on 1.2 while it
/// finishes a bid has made a decision, and a program that went round the back
/// of it to a feed of ours would be overruling the person who owns the
/// machines.
///
/// A seat with no server — a copy on a laptop, one somebody was handed at a
/// demo, the first machine in a shop before anybody has set anything up — has
/// nobody to ask, so it asks here itself.
///
/// A company's server asks here on its seats' behalf, and only its
/// administrator can hold it back.
///
/// What travels over it is an installer and the signed manifest that describes
/// it. Nothing about a company goes the other way: not a drawing, not a markup,
/// not a tool chest, not a name. And what arrives is checked against [`KEYS`]
/// before it is run, so this address is a place to fetch from rather than
/// something the program trusts — a feed that was taken over could serve only
/// things that were already signed by a key its publisher holds.
///
/// Empty means a loose seat simply never looks, which is what a build does
/// until somebody fills this in.
pub const HOME_FEED: &str = "https://api.github.com/repos/excaliburct/hyperview-releases/releases";

/// The channel a copy follows when nobody has said otherwise.
pub const HOME_CHANNEL: &str = "stable";

/// Where a renewed license is published, if anywhere.
///
/// A license is a signed file, and today a shop that buys three more seats
/// waits for somebody to sign a new one and email it over. That is the whole
/// of the friction, and it is why "buy six seats at once" is the only shape
/// the thing can be sold in.
///
/// A server that knows its own license id can fetch its own renewal. The file
/// is published under a name derived from the id by SHA-256, so the address
/// cannot be guessed from a company's name and the list of customers is not
/// something anybody can walk. The id is the capability: whoever has it can
/// fetch that license and nothing else.
///
/// Three things this deliberately is not. It is **not an account**: no
/// password, no session, nothing to reset, nothing to keep running. It is
/// **not a check-in**: the server asks for a file and sends nothing about
/// itself, not a seat count, not a company name, not a drawing. And it is
/// **not how a license takes effect** — what arrives is checked against
/// [`KEYS`] exactly like an update, and a file that is not signed, or is
/// worse than the one already held, is ignored.
///
/// A sealed office never asks: the request goes through the same outbound
/// gate as everything else, and its licenses arrive as files, by hand, the way
/// that office wants everything to arrive.
///
/// Empty means no server ever looks. It was empty until 0.6.5, which meant
/// every part of this worked and none of it ran.
///
/// A folder on our own site, because a licence renewal is a small signed file
/// that has to be fetchable by name over HTTPS with no login, and that is all
/// it has to be. Nothing here is trusted because of where it came from: what
/// arrives is checked against [`KEYS`] and against the licence already held,
/// so this address is a place to fetch from and not a thing the server
/// believes.
pub const LICENSE_FEED: &str = "https://excaliburct.com/f";
