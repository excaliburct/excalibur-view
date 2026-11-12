//! The server's signed-request check, fed random headers, targets and bodies:
//! it never panics, and nothing random ever gets through.

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use hyperview_server::{api, Config, Store};

/// xorshift64*, seeded, so a failure can be run again exactly.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12; // WHAT: xorshift. WHY: cheap and well spread
        self.0 ^= self.0 << 25; // WHAT: the second shift. WHY: xorshift64*'s three
        self.0 ^= self.0 >> 27; // WHAT: the third. WHY: likewise
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) // WHAT: the scrambled output. WHY: the * of xorshift64*
    }

    fn text(&mut self, most: usize) -> String {
        let alphabet = b"0123456789abcdef+/=ABCDEFxyz -:\t\""; // WHAT: hex, base64 and separators. WHY: near-misses of real header values
        let n = usize::try_from(self.next() % (most as u64 + 1)).unwrap_or(0); // WHAT: a length up to `most`. WHY: short and long
        (0..n).map(|_| alphabet[usize::try_from(self.next()).unwrap_or(0) % alphabet.len()] as char).collect() // WHAT: the letters, as text. WHY: drawn from the alphabet
    }
}

#[test]
fn random_signed_requests_never_panic_and_never_pass() {
    let n: u64 = rand::random(); // WHAT: a random folder name. WHY: below
    let data = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("random-signed-{n:016x}")); // WHAT: a fresh folder. WHY: tests run side by side
    let config = Config { data: data.clone(), ..Config::default() }; // WHAT: a server on that folder. WHY: a database of its own
    let server = api::Server::new(Store::open(&config.database(), &config.blobs()).unwrap(), config); // WHAT: the server, not listening. WHY: check is called directly
    let key = hub::device::DeviceKey::from_secret(&[3u8; 32]); // WHAT: a computer's key. WHY: its id is sent some rounds
    server
        .store
        .with(|db| {
            Ok(db.execute(
                "INSERT INTO devices (id, public_key, app, machine, os_user, state, admin, asked, changed, scope) VALUES (?1, ?2, 'view', 'PC', 'dana', 'approved', 0, '2026-11-12T00:00:00Z', '2026-11-12T00:00:00Z', 'all')",
                rusqlite::params![key.id(), key.public_key_text()],
            )?)
        })
        .unwrap(); // WHAT: that computer, approved. WHY: so random signatures reach the signature check, not just the device lookup
    let now = hub::device::unix_now(); // WHAT: the time now. WHY: inside the window, after the start
    let mut random = Random(0xF00D_CAFE_D00D_BEEF); // WHAT: a fixed seed. WHY: a failure runs again exactly
    let mut reached = 0usize; // WHAT: rounds the verifier refused. WHY: below
    for round in 0..20_000 { // WHAT: twenty thousand rounds. WHY: wide, and still quick
        let mut headers = HeaderMap::new(); // WHAT: this round's headers. WHY: filled below
        let device = if round % 3 == 0 { key.id() } else { random.text(40) }; // WHAT: sometimes a real id. WHY: deeper into the check
        let time = if round % 2 == 0 { now.to_string() } else { random.text(14) }; // WHAT: sometimes a good time. WHY: likewise
        let signature = if round % 2 == 0 { base64::Engine::encode(&base64::engine::general_purpose::STANDARD, (0..64).map(|_| random.next().to_le_bytes()[0]).collect::<Vec<u8>>()) } else { random.text(100) }; // WHAT: sometimes a well formed signature. WHY: past the header check, to the verifier
        let body_hash = random.text(70); // WHAT: a random body hash. WHY: the hash parser
        let mut sent = vec![(hub::device::HEADER_DEVICE, device), (hub::device::HEADER_TIME, time), (hub::device::HEADER_SIGNATURE, signature)]; // WHAT: the headers this round sends. WHY: as a request would carry them
        if round % 4 != 0 { // WHAT: three rounds in four. WHY: the rest go without
            sent.push((hub::device::HEADER_BODY, body_hash)); // WHAT: a body hash, mostly. WHY: without one the body is hashed and the signature checked
        }
        for (name, value) in sent { // WHAT: each header. WHY: into the map
            if let Ok(v) = HeaderValue::from_str(&value) {
                headers.insert(HeaderName::from_static(Box::leak(name.to_ascii_lowercase().into_boxed_str())), v); // WHAT: the header, when it is one HTTP allows. WHY: as a request would carry it
            }
        }
        let target = format!("/{}", random.text(60)); // WHAT: a random target. WHY: the message builder
        let body = random.text(200).into_bytes(); // WHAT: a random body. WHY: its hash is signed over
        let checked = hyperview_server::devices::check(&server, &headers, "GET", &target, &body, round % 5 == 0, now); // WHAT: the whole check, sometimes in any state. WHY: as the gate and /devices/me call it
        assert!(checked.is_err(), "nothing random gets through (round {round})"); // WHAT: refused, never a panic. WHY: the property
        reached += usize::from(matches!(checked, Err(hyperview_server::devices::Refusal::Signature))); // WHAT: counted when the verifier itself refused it. WHY: the rounds must reach the signature check, not stop at the headers
    }
    assert!(reached > 100, "random requests reached the signature check {reached} times"); // WHAT: the verifier was reached. WHY: a test that stops at the headers proves nothing about signatures
}
