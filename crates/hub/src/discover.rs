//! Finding the company's server without anybody typing an address.
//!
//! The first person in a shop sets a server up. Everybody else just opens the
//! program, and it should already be offering them "Example Steel — 12 drawing sets"
//! to click. Asking an estimator for an IP address is asking them to go and
//! find somebody who knows one.
//!
//! So a server answers for itself on the local network: a seat shouts once, and
//! anything that is a Hyperview server says what it is called and where it is.
//! Plain UDP on one port — no service to install, no name server to configure,
//! nothing that stops working when a shop's IT changes something.
//!
//! Three things this deliberately does not do.
//!
//! It is **not a way in.** A reply says only what a server calls itself and
//! where to find it; every question after that needs signing in, so answering
//! gives away nothing that browsing to the address would not.
//!
//! It **never leaves the building.** Broadcast traffic does not cross a router,
//! which means a shop's server does not announce itself to the internet, and a
//! seat cannot accidentally find somebody else's.
//!
//! And an answer is **an offer, not an instruction.** What comes back is shown
//! to the person to choose from. A machine on the network claiming to be a
//! server is not a reason to trust it with a password, so the address is always
//! visible before anybody types one.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// The port servers listen on to be found. Not the port they serve on
/// ([`SERVE_PORT`]).
pub const PORT: u16 = 8715;

/// The port the Office Server serves on unless somebody says otherwise (its
/// plain HTTP and its HTTPS, on the one port).
pub const SERVE_PORT: u16 = 8714; // WHAT: the Office Server's port. WHY: a typed address without one goes there

/// What a seat shouts. Version in the string, so an old seat and a new server
/// can recognise each other's shouting and say so rather than sitting silent.
pub const ASKING: &[u8] = b"HYPERVIEW-FIND-1";

/// What a server shouts back.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// What the company calls this installation.
    pub name: String,
    /// Where to actually talk to it: `https://drawings.acme.com` or
    /// `http://192.168.1.40:8714`.
    pub url: String,
    /// The server's own build.
    pub version: String,
    /// The wire version it speaks, so a seat can say plainly that one of them
    /// needs updating rather than failing later.
    pub api_version: u32,
    /// Whether anybody has set it up yet. A server nobody has claimed is the
    /// one the first person in the shop should be offered.
    #[serde(default)]
    pub ready: bool,
    /// Which installation this is, so the same server heard down two different
    /// roads is recognised as one server.
    ///
    /// It is needed because a machine has more than one address. Sitting at the
    /// server itself, the broadcast comes back over the network card and the
    /// direct question comes back over loopback, and both are true answers with
    /// different addresses in them. Without something to match on, the person
    /// who just set a server up sees two of it and has no way to tell which one
    /// is real. Empty on an older server, which cannot be matched and is
    /// therefore left alone.
    #[serde(default)]
    pub id: String,
}

/// A server that answered, and where from.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub answer: Answer,
    pub from: IpAddr,
}

impl Found {
    /// What to show in a list. The address is always part of it: somebody
    /// about to type a password should see where it is going.
    pub fn as_line(&self) -> String {
        let name = if self.answer.name.trim().is_empty() {
            "Excalibur View server".to_string()
        } else {
            self.answer.name.clone()
        };
        if self.answer.ready {
            format!("{name} — {}", self.answer.url)
        } else {
            format!("{name} — {} (not set up yet)", self.answer.url)
        }
    }
}

/// Shouts on the local network and gathers whatever answers.
///
/// Waits the whole time rather than stopping at the first answer: a shop can
/// have more than one, and showing somebody the only one that happened to be
/// quickest is how a seat ends up on the wrong server.
pub fn look(how_long: Duration) -> Vec<Found> {
    let Ok(socket) = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], 0))) else {
        return Vec::new();
    };
    if socket.set_broadcast(true).is_err() {
        return Vec::new();
    }
    let _ = socket.set_read_timeout(Some(Duration::from_millis(250)));

    // Broadcast, and also straight to this machine: the first person setting a
    // server up is usually sitting at it, and some machines do not hear their
    // own broadcast.
    for to in [
        SocketAddr::new(IpAddr::V4(Ipv4Addr::BROADCAST), PORT),
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), PORT),
    ] {
        let _ = socket.send_to(ASKING, to);
    }

    trustworthy(gather(&socket, how_long)) // WHAT: only answers that name the address they came from, on a local network. WHY: an answer can't send a seat anywhere else
}

/// The answers a seat may act on: each names, as its address, the very
/// machine it came from, and that machine is on a local network. Anything
/// else on the network could answer naming any address at all.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn trustworthy(found: Vec<Found>) -> Vec<Found> {
    found.into_iter().filter(|f| names_itself(f) && local(f.from)).collect() // WHAT: kept only when both hold. WHY: review: a discovery answer could name any URL
}

/// Whether an address is on a local network: the client's rule
/// (`office::is_local`) when the client is built, the same rule here
/// otherwise.
fn local(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => v4.is_private() || v4.is_loopback() || v4.is_link_local(), // WHAT: 10/8, 172.16/12, 192.168/16, 127/8, 169.254/16. WHY: §2.2
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80, // WHAT: ::1, fc00::/7, fe80::/10. WHY: §2.2
    }
}

/// Whether an answer's address is the machine it came from.
fn names_itself(found: &Found) -> bool {
    let Ok(url) = url::Url::parse(found.answer.url.trim()) else { return false }; // WHAT: the address, read as the connection reads it. WHY: never guessed at
    let host = match url.host() {
        Some(url::Host::Ipv4(ip)) => IpAddr::V4(ip), // WHAT: an IPv4 address. WHY: compared as is
        Some(url::Host::Ipv6(ip)) => IpAddr::V6(ip), // WHAT: an IPv6 address. WHY: compared as is
        _ => return false, // WHAT: a name, or nothing. WHY: a name could point anywhere; only an address can be matched to the sender
    };
    host.to_canonical() == found.from.to_canonical() // WHAT: the same machine. WHY: an IPv4 sender heard over IPv6 compares as itself
}

/// Collects answers until the time is up, keeping one per address.
pub fn gather(socket: &UdpSocket, how_long: Duration) -> Vec<Found> {
    let until = Instant::now() + how_long;
    let mut found: Vec<Found> = Vec::new();
    let mut buffer = [0u8; 2048];
    while Instant::now() < until {
        let Ok((n, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        let Ok(answer) = serde_json::from_slice::<Answer>(&buffer[..n]) else {
            continue;
        };
        if answer.url.trim().is_empty() {
            continue;
        }
        // The same server heard twice — over broadcast and directly — is one
        // server, not two.
        if let Some(already) = found.iter_mut().find(|f| same_one(&f.answer, &answer)) {
            // Keep whichever address somebody else could also use. A loopback
            // address works from the machine it answered on and nowhere else,
            // and it is the one that gets written down and handed round.
            if reachable_by_others(&answer.url) && !reachable_by_others(&already.answer.url) {
                already.answer = answer;
                already.from = from.ip();
            }
            continue;
        }
        found.push(Found {
            answer,
            from: from.ip(),
        });
    }
    // Ones already set up first: a seat joining an office wants the running
    // server, and only the person setting one up wants the other.
    found.sort_by(|a, b| {
        b.answer
            .ready
            .cmp(&a.answer.ready)
            .then(a.answer.name.cmp(&b.answer.name))
    });
    found
}

/// Answers seats that come looking. A server runs this for as long as it runs.
///
/// `describe` is handed the machine that asked, because the address a server
/// should give out depends on who is asking. A server listening on `0.0.0.0`
/// has no single address; the useful one is whichever of its own the asking
/// machine can actually reach, and [`facing`] works that out.
///
/// Returns only if the socket cannot be opened, which on a machine where
/// something else already holds the port is ordinary and not worth stopping a
/// server over — it simply will not be found automatically.
pub fn answer_for(mut describe: impl FnMut(SocketAddr) -> Answer) {
    let Ok(socket) = UdpSocket::bind(SocketAddr::from(([0, 0, 0, 0], PORT))) else {
        return;
    };
    let mut buffer = [0u8; 256];
    let mut last_answered: std::collections::HashMap<std::net::IpAddr, std::time::Instant> = std::collections::HashMap::new(); // WHAT: when each asker was last answered. WHY: one answer a second each
    loop {
        let Ok((n, from)) = socket.recv_from(&mut buffer) else {
            continue;
        };
        if &buffer[..n] != ASKING {
            continue;
        }
        if !on_a_local_network(from.ip()) {
            continue; // WHAT: only the office's own networks answered. WHY: an answer to a forged internet address made this server an amplifier (audit, 2026-11-11)
        }
        let now = std::time::Instant::now(); // WHAT: now. WHY: compared with the last answer
        if last_answered.get(&from.ip()).is_some_and(|at| now.duration_since(*at) < std::time::Duration::from_secs(1)) {
            continue; // WHAT: answered under a second ago. WHY: a flood from one address costs one answer a second
        }
        if last_answered.len() >= 4096 {
            last_answered.retain(|_, at| now.duration_since(*at) < std::time::Duration::from_secs(1)); // WHAT: the old ones forgotten. WHY: the map stays small
        }
        last_answered.insert(from.ip(), now); // WHAT: noted. WHY: as above
        let Ok(text) = serde_json::to_vec(&describe(from)) else {
            continue;
        };
        let _ = socket.send_to(&text, from);
    }
}

/// WHAT: whether an address is on a local network: this machine, a private
/// range, link-local, or carrier-grade NAT. WHY: discovery is for the office's
/// own network; nobody on the internet needs an answer.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn on_a_local_network(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets(); // WHAT: the first two numbers. WHY: carrier-grade NAT is 100.64/10
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || (a == 100 && (64..128).contains(&b))
        }
        std::net::IpAddr::V6(v6) => {
            let first = v6.segments()[0]; // WHAT: the first 16 bits. WHY: unique-local fc00::/7 and link-local fe80::/10
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80 || v6.to_ipv4_mapped().is_some_and(|v4| on_a_local_network(v4.into()))
        }
    }
}

/// Two answers from one server.
fn same_one(a: &Answer, b: &Answer) -> bool {
    if !a.id.trim().is_empty() && a.id == b.id {
        return true;
    }
    // An older server has no id to match on, so fall back to the address. Two
    // of it is better than one of somebody else's.
    a.url == b.url
}

/// Whether an address means anything to a machine other than the one that gave
/// it out.
fn reachable_by_others(url: &str) -> bool {
    let host = url
        .trim_start_matches("http://")
        .trim_start_matches("https://")
        .split(['/', ':'])
        .next()
        .unwrap_or("");
    !(host == "localhost"
        || host == "::1"
        || host.parse::<IpAddr>().map(|ip| ip.is_loopback()).unwrap_or(false))
}

/// Which of this machine's own addresses the given machine would reach it on.
///
/// A shop machine can easily have four: a wired card, wifi, a VPN and a virtual
/// switch some accounting package installed. Announcing the wrong one sends a
/// seat to an address that does not answer. Rather than guessing, this asks the
/// operating system the same question it answers when something actually
/// connects: route a socket towards the asker and read back which address it
/// chose. No packet is sent — a UDP `connect` only sets the destination.
pub fn facing(peer: IpAddr) -> Option<IpAddr> {
    let bind: SocketAddr = match peer {
        IpAddr::V4(_) => ([0, 0, 0, 0], 0).into(),
        IpAddr::V6(_) => (Ipv6Addr::UNSPECIFIED, 0).into(),
    };
    let socket = UdpSocket::bind(bind).ok()?;
    socket.connect(SocketAddr::new(peer, PORT)).ok()?;
    let mine = socket.local_addr().ok()?.ip();
    // A machine asking from itself gets told to use itself, which is right.
    if mine.is_unspecified() {
        return None;
    }
    Some(mine)
}

#[cfg(test)]
mod tests {

    #[test]
    fn only_local_networks_are_answered() {
        for local in ["127.0.0.1", "10.1.2.3", "172.16.0.9", "192.168.20.3", "169.254.1.1", "100.64.0.1", "::1", "fd00::1", "fe80::1", "::ffff:192.168.1.5"] {
            assert!(super::on_a_local_network(local.parse().unwrap()), "{local}"); // WHAT: answered. WHY: the office's own networks
        }
        for outside in ["8.8.8.8", "100.128.0.1", "2001:4860::8888", "::ffff:8.8.8.8"] {
            assert!(!super::on_a_local_network(outside.parse().unwrap()), "{outside}"); // WHAT: not answered. WHY: the internet
        }
    }
    use super::*;

    fn an_answer(name: &str, url: &str, ready: bool) -> Answer {
        Answer {
            name: name.into(),
            url: url.into(),
            version: "0.1.0".into(),
            api_version: 1,
            ready,
            id: format!("inst-{name}"),
        }
    }

    /// A server answering on a port of its own, so tests do not fight over the
    /// real one or need a network.
    fn a_server(answer: Answer) -> (UdpSocket, u16) {
        let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("a socket");
        let port = socket.local_addr().unwrap().port();
        let listening = socket.try_clone().expect("a clone");
        std::thread::spawn(move || {
            let mut buffer = [0u8; 256];
            for _ in 0..20 {
                let Ok((n, from)) = listening.recv_from(&mut buffer) else {
                    return;
                };
                if &buffer[..n] == ASKING {
                    let text = serde_json::to_vec(&answer).unwrap();
                    let _ = listening.send_to(&text, from);
                }
            }
        });
        (socket, port)
    }

    fn ask(ports: &[u16], how_long: Duration) -> Vec<Found> {
        let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("a socket");
        socket.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
        for port in ports {
            let _ = socket.send_to(ASKING, SocketAddr::from(([127, 0, 0, 1], *port)));
        }
        gather(&socket, how_long)
    }

    #[test]
    fn a_server_on_the_network_answers_for_itself() {
        let (_keep, port) = a_server(an_answer("Example Steel", "http://192.168.1.40:8714", true));
        let found = ask(&[port], Duration::from_millis(600));
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].answer.name, "Example Steel");
        // The address is part of what the person sees, always.
        assert!(found[0].as_line().contains("192.168.1.40"));
    }

    #[test]
    fn two_servers_are_both_offered_rather_than_whichever_was_quickest() {
        let (_a, port_a) = a_server(an_answer("Example Steel", "http://192.168.1.40:8714", true));
        let (_b, port_b) = a_server(an_answer("Elm Theater Job", "http://192.168.1.55:8714", true));
        let found = ask(&[port_a, port_b], Duration::from_millis(700));
        assert_eq!(found.len(), 2, "{found:?}");
    }

    #[test]
    fn a_server_nobody_has_set_up_yet_is_marked_and_sorted_last() {
        let (_a, fresh) = a_server(an_answer("New server", "http://192.168.1.60:8714", false));
        let (_b, running) = a_server(an_answer("Example Steel", "http://192.168.1.40:8714", true));
        let found = ask(&[fresh, running], Duration::from_millis(700));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].answer.name, "Example Steel", "the running one first");
        assert!(found[1].as_line().contains("not set up yet"));
    }

    #[test]
    fn the_same_server_heard_twice_is_one_server() {
        let (_keep, port) = a_server(an_answer("Example Steel", "http://192.168.1.40:8714", true));
        // Asked twice, as broadcast and direct would.
        let found = ask(&[port, port], Duration::from_millis(700));
        assert_eq!(found.len(), 1, "{found:?}");
    }

    #[test]
    fn one_server_answering_on_two_of_its_own_addresses_is_still_one_server() {
        // What somebody sitting at the server itself sees: the broadcast comes
        // back over the network card, the direct question over loopback, and
        // both answers are true.
        let mut over_the_network = an_answer("Example Steel", "http://192.168.1.40:8714", true);
        let mut over_loopback = over_the_network.clone();
        over_loopback.url = "http://127.0.0.1:8714".into();
        over_the_network.id = "inst-mesa".into();
        over_loopback.id = "inst-mesa".into();

        let (_a, one) = a_server(over_loopback);
        let (_b, two) = a_server(over_the_network);
        let found = ask(&[one, two], Duration::from_millis(700));
        assert_eq!(found.len(), 1, "{found:?}");
        // And the address kept is the one somebody at another desk can use.
        assert_eq!(found[0].answer.url, "http://192.168.1.40:8714");
    }

    #[test]
    fn two_different_servers_are_not_folded_into_one_by_accident() {
        let mut mesa = an_answer("Example Steel", "http://192.168.1.40:8714", true);
        let mut other = an_answer("Elm Theater", "http://192.168.1.55:8714", true);
        mesa.id = "inst-mesa".into();
        other.id = "inst-fox".into();
        let (_a, one) = a_server(mesa);
        let (_b, two) = a_server(other);
        assert_eq!(ask(&[one, two], Duration::from_millis(700)).len(), 2);
    }

    #[test]
    fn an_address_only_the_server_itself_can_use_is_known_for_what_it_is() {
        assert!(!reachable_by_others("http://127.0.0.1:8714"));
        assert!(!reachable_by_others("http://localhost:8714"));
        assert!(reachable_by_others("http://192.168.1.40:8714"));
        assert!(reachable_by_others("https://drawings.acme.com"));
    }

    #[test]
    fn something_that_is_not_a_server_is_ignored_rather_than_offered() {
        let socket = UdpSocket::bind(SocketAddr::from(([127, 0, 0, 1], 0))).expect("a socket");
        let port = socket.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 256];
            if let Ok((_, from)) = socket.recv_from(&mut buffer) {
                let _ = socket.send_to(b"who is asking?", from);
            }
        });
        assert!(ask(&[port], Duration::from_millis(400)).is_empty());
    }

    #[test]
    fn an_answer_with_no_address_in_it_is_no_use_and_is_dropped() {
        let (_keep, port) = a_server(an_answer("Nowhere", "", true));
        assert!(ask(&[port], Duration::from_millis(400)).is_empty());
    }

    #[test]
    fn a_server_works_out_which_of_its_own_addresses_to_give_out() {
        // Asked by this machine, the answer is this machine — and it is a real
        // address rather than the "anywhere" one, which nothing can connect to.
        let mine = facing(IpAddr::V4(Ipv4Addr::LOCALHOST)).expect("an address");
        assert!(!mine.is_unspecified());
        assert_eq!(mine, IpAddr::V4(Ipv4Addr::LOCALHOST));
    }

    #[test]
    fn finding_nothing_is_an_empty_list_rather_than_a_wait_forever() {
        let began = Instant::now();
        let found = ask(&[], Duration::from_millis(300));
        assert!(found.is_empty());
        assert!(began.elapsed() < Duration::from_secs(2), "it has to give up");
    }

    fn found(url: &str, from: &str) -> Found {
        Found { answer: an_answer("Example Steel", url, true), from: from.parse().unwrap() } // WHAT: one answer heard from `from`. WHY: the filter is tested on its own
    }

    #[test]
    fn an_answer_counts_only_for_the_machine_that_sent_it_on_the_office_network() {
        let kept = trustworthy(vec![
            found("http://192.168.1.40:8714", "192.168.1.40"), // WHAT: names itself. WHY: kept
            found("http://192.168.1.41:8714", "192.168.1.40"), // WHAT: names another machine. WHY: dropped
            found("http://office.example.com:8714", "192.168.1.40"), // WHAT: names a name. WHY: dropped
            found("http://8.8.8.8:8714", "8.8.8.8"), // WHAT: off the office network. WHY: dropped
        ]);
        assert_eq!(kept.len(), 1, "{kept:?}");
        assert_eq!(kept[0].answer.url, "http://192.168.1.40:8714");
    }
}
