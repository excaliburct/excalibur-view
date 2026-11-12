//! Excalibur View Office licenses.
//!
//! A license is a small text file, signed by Excalibur Construction
//! Technologies and checked by the office's own server against a key built
//! into the program. Nothing phones home: a server that never touches the
//! internet checks a license exactly as well as one that does.
//!
//! The desktop app is free and never needs one. A license is about the office
//! server — the part that lets people work together — and three rules hold
//! whatever state a license is in:
//!
//! - **Nothing is ever deleted or locked away for not paying.** An office with
//!   no license can still open, print and export every drawing and markup it
//!   has. Only sharing *new* work through the server pauses.
//! - **Nobody already signed in is ever turned away.** A user limit stops an
//!   administrator adding the next person, never removes one.
//! - **A version you have keeps working.** Updates end; the program does not.
//!
//! What is signed is not the JSON, for the same reason a release's signature
//! is not (`update::Release::signing_payload`): two encoders will not agree on
//! key order. It is a fixed text, and it starts with a different first line
//! from a release's, so a signature on one can never be passed off as a
//! signature on the other even though the same publisher key makes both.

use serde::{Deserialize, Serialize};

use crate::update::Trusted;

/// What a license file is called: `Example Steel, Inc.evlicense`.
pub const EXTENSION: &str = "evlicense";

/// `updates_through` for a license whose updates never end.
pub const FOREVER: &str = "forever";

/// Where somebody buys or renews. Shown in the program, never fetched by it.
pub const BUY_URL: &str = crate::site::BUY;

/// The first line of the signed text. Different from a release's on purpose.
const DOMAIN: &str = "excalibur-license-v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edition {
    Office,
    Sealed,
}

impl Edition {
    pub const fn name(self) -> &'static str {
        match self {
            Edition::Office => "office",
            Edition::Sealed => "sealed",
        }
    }

    /// As people read it.
    pub fn title(self) -> &'static str {
        match self {
            Edition::Office => "Excalibur View Office",
            Edition::Sealed => "Excalibur View Sealed",
        }
    }
}

/// A part of the Excalibur Office Server an office can have (docs/OFFICE-SERVER.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Module {
    /// Drawings, takeoff, markups, sign-offs: always on.
    View,
    /// Excalibur CAD's office drawings.
    Cad,
    /// Archetype's shared jobs.
    Archetype,
    /// Citadel's estimating jobs, kept blind.
    Citadel,
}

impl Module {
    /// Every part this build knows, in the order the Manager lists them.
    pub const ALL: [Module; 4] = [Module::View, Module::Cad, Module::Archetype, Module::Citadel];

    /// Its name in a license and the API.
    pub const fn name(self) -> &'static str {
        match self {
            Module::View => "view",
            Module::Cad => "cad",
            Module::Archetype => "archetype",
            Module::Citadel => "citadel",
        }
    }

    /// The part a license's name stands for, if this build knows it.
    pub fn named(name: &str) -> Option<Module> {
        Module::ALL.into_iter().find(|m| m.name() == name)
    }

    /// As people read it.
    pub const fn title(self) -> &'static str {
        match self {
            Module::View => "Excalibur View",
            Module::Cad => "Excalibur CAD drawings",
            Module::Archetype => "Archetype jobs",
            Module::Citadel => "Citadel (estimating)",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct License {
    /// `evl_…`, quoted when somebody rings about it.
    pub id: String,
    /// Printed on the license and shown in the program.
    pub company: String,
    pub edition: Edition,
    /// How many people the office server may have accounts for. Nought is no
    /// limit.
    pub users: u32,
    /// The last day of updates, `YYYY-MM-DD`, or `forever`.
    pub updates_through: String,
    /// `YYYY-MM-DD`.
    pub issued: String,
    /// "Founding customer", or whatever else was agreed. Signed with the rest.
    #[serde(default)]
    pub note: String,
    /// The Office Server's parts this office has, besides View: `cad`, `archetype`, `citadel`. Signed with the rest;
    /// written only when there are any, so every license made before parts existed reads, and checks, exactly as before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<String>,
    /// Which of the program's keys signed it.
    pub key: String,
    /// Ed25519, hex.
    pub signature: String,
}

impl License {
    /// Exactly the bytes that get signed. `tools/publish.py sign-license`
    /// writes the same text; a test in that direction keeps them honest.
    pub fn signing_payload(&self) -> Vec<u8> {
        format!(
            "{DOMAIN}\n\
             id={}\n\
             company={}\n\
             edition={}\n\
             users={}\n\
             updates_through={}\n\
             issued={}\n\
             note={}\n",
            self.id,
            self.company,
            self.edition.name(),
            self.users,
            self.updates_through,
            self.issued,
            self.note,
        )
        .into_bytes()
        .into_iter()
        .chain(if self.modules.is_empty() { Vec::new() } else { format!("modules={}\n", self.modules.join(",")).into_bytes() }) // WHAT: the parts line, last, only when there are parts. WHY: an older license's signed text is unchanged, so it still checks out
        .collect()
    }

    /// The Office Server's parts this license switches on: View always, then each part it names that this build knows.
    /// A part this build doesn't know yet is skipped, never an error: a license issued for a newer server still works here.
    pub fn parts(&self) -> Vec<Module> {
        let mut on = vec![Module::View]; // WHAT: View is always there. WHY: every Office Server is View's server grown
        for name in &self.modules {
            if let Some(part) = Module::named(name) {
                if !on.contains(&part) {
                    on.push(part); // WHAT: a known part, once. WHY: the list the server serves
                }
            }
        }
        on
    }

    /// Reads a license file and checks it is signed by a key in `trusted`.
    /// The error is a sentence for the person who chose the file.
    pub fn read(text: &str, trusted: &Trusted) -> Result<License, String> {
        let license = License::parse(text)?;
        license.check(trusted)?;
        Ok(license)
    }

    /// Reads a license file without checking its signature: for showing what
    /// a file says, never for deciding anything.
    pub fn parse(text: &str) -> Result<License, String> {
        let text = text.trim_start_matches('\u{feff}').trim();
        let license: License = serde_json::from_str(text).map_err(|_| {
            "That is not an Excalibur View license file. It should be the .evlicense file \
             that came by email."
                .to_string()
        })?;
        license.well_formed()?;
        Ok(license)
    }

    fn well_formed(&self) -> Result<(), String> {
        let fields = [
            &self.id,
            &self.company,
            &self.updates_through,
            &self.issued,
            &self.note,
            &self.key,
        ];
        if fields.iter().any(|f| f.contains(['\n', '\r'])) {
            return Err("That license file has been changed since it was issued.".into());
        }
        // WHAT: each part's name is short plain lowercase, at most 16 parts, none twice. WHY: the names go into the signed text
        // joined by commas, so a comma, a newline or "=" in one could make two different lists sign the same
        let plain = |m: &String| (1..=32).contains(&m.len()) && m.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && m.as_bytes()[0].is_ascii_lowercase();
        if self.modules.len() > 16 || !self.modules.iter().all(plain) || (1..self.modules.len()).any(|i| self.modules[..i].contains(&self.modules[i])) {
            return Err("That license file has been changed since it was issued.".into());
        }
        if self.id.trim().is_empty() || self.company.trim().is_empty() {
            return Err("That license file is missing who it is for.".into());
        }
        if self.updates_through != FOREVER && !is_date(&self.updates_through) {
            return Err("That license file has no readable updates date.".into());
        }
        if !is_date(&self.issued) {
            return Err("That license file has no readable issue date.".into());
        }
        Ok(())
    }

    /// The signature, against the keys this build was made with.
    pub fn check(&self, trusted: &Trusted) -> Result<(), String> {
        let Some(key) = trusted.key(&self.key) else {
            return Err(format!(
                "That license is signed with a key this version does not know ({}). \
                 Update Excalibur View, or ask for the license to be issued again.",
                self.key
            ));
        };
        let signature = crate::update::unhex(&self.signature)
            .ok_or("That license file's signature is not readable.")?;
        crate::update::verify(key, &self.signing_payload(), &signature).map_err(|_| {
            "That license file does not check out: it was changed after it was issued, or it \
             was not issued by Excalibur Construction Technologies."
                .to_string()
        })
    }

    pub fn unlimited(&self) -> bool {
        self.users == 0
    }

    pub fn forever(&self) -> bool {
        self.updates_through == FOREVER
    }

    /// Whether a version published on `published` (an RFC 3339 time or a
    /// plain date) comes with this license's updates.
    pub fn covers(&self, published: &str) -> bool {
        if self.forever() {
            return true;
        }
        match published.get(..10).filter(|d| is_date(d)) {
            // Dates written YYYY-MM-DD compare as text.
            Some(day) => day <= self.updates_through.as_str(),
            // A release that does not say when it was published is given the
            // benefit of the doubt: refusing it would punish the customer for
            // the publisher's mistake.
            None => true,
        }
    }
}

/// `YYYY-MM-DD`, and a real day of a real month.
pub fn is_date(text: &str) -> bool {
    let b = text.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let number = |r: std::ops::Range<usize>| text.get(r).and_then(|s| s.parse::<u32>().ok());
    matches!(
        (number(0..4), number(5..7), number(8..10)),
        (Some(y), Some(m), Some(d)) if y >= 2000 && (1..=12).contains(&m) && (1..=31).contains(&d)
    )
}

/// Where an office stands, as the server tells the program. Everything the
/// program shows about licensing is worked out from this, so the website, the
/// server and the window say the same thing.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Standing {
    /// `founding`, `licensed`, `trial` or `unlicensed`.
    pub state: String,
    /// `office` or `sealed`, when there is a license or a founding install.
    #[serde(default)]
    pub edition: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    /// Administrators only.
    #[serde(default)]
    pub license_id: Option<String>,
    /// `None` is no limit.
    #[serde(default)]
    pub users_allowed: Option<u32>,
    #[serde(default)]
    pub users: u32,
    /// `None` is forever, or no license.
    #[serde(default)]
    pub updates_through: Option<String>,
    #[serde(default)]
    pub trial_days_left: Option<u32>,
    /// Whether new work can be shared through the server right now.
    #[serde(default = "yes")]
    pub sharing: bool,
    /// One sentence, for whoever is looking.
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub buy_url: String,
}

fn yes() -> bool {
    true
}

impl Standing {
    /// The line for Help → About.
    pub fn about_line(&self) -> String {
        match self.state.as_str() {
            "founding" => format!(
                "Excalibur View Office{}. Founding install: every user, every update.",
                self.company.as_deref().map(|c| format!(", {c}")).unwrap_or_default()
            ),
            "licensed" => {
                let edition = match self.edition.as_deref() {
                    Some("sealed") => Edition::Sealed.title(),
                    _ => Edition::Office.title(),
                };
                let who = self.company.as_deref().map(|c| format!(", licensed to {c}")).unwrap_or_default();
                // "Example Steel, Inc." already ends the sentence.
                let stop = if who.ends_with('.') { "" } else { "." };
                let users = match self.users_allowed {
                    Some(n) => format!(" {} of {n} users.", self.users),
                    None => " Every user.".to_string(),
                };
                let updates = match &self.updates_through {
                    Some(day) => format!(" Updates through {}.", long_date(day)),
                    None => " Updates forever.".to_string(),
                };
                format!("{edition}{who}{stop}{users}{updates}")
            }
            "trial" => format!(
                "Excalibur View Office trial: {} day{} left.",
                self.trial_days_left.unwrap_or(0),
                if self.trial_days_left == Some(1) { "" } else { "s" }
            ),
            _ => "Excalibur View Office: no license. Sharing new work is paused; \
                  everything already here can be opened and exported."
                .to_string(),
        }
    }
}

/// `2027-10-01` → `Oct 1, 2027`.
pub fn long_date(day: &str) -> String {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if !is_date(day) {
        return day.to_string();
    }
    let month = day[5..7].parse::<usize>().unwrap_or(1);
    let date = day[8..10].parse::<u32>().unwrap_or(1);
    format!("{} {date}, {}", MONTHS[month - 1], &day[..4])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    fn issuer() -> (SigningKey, Trusted) {
        // A fixed key made up for the test. The real one lives on the
        // publisher's own PC and never in a repository.
        let signing = SigningKey::from_bytes(&[9u8; 32]);
        let trusted = Trusted {
            keys: vec![("mesafab-2026".into(), signing.verifying_key().to_bytes())],
        };
        (signing, trusted)
    }

    fn a_license(signing: &SigningKey) -> License {
        let mut license = License {
            id: "evl_0001".into(),
            company: "Example Steel, Inc.".into(),
            edition: Edition::Office,
            users: 10,
            updates_through: "2027-10-01".into(),
            issued: "2026-10-01".into(),
            note: String::new(),
            modules: Vec::new(),
            key: "mesafab-2026".into(),
            signature: String::new(),
        };
        license.signature = hex(&signing.sign(&license.signing_payload()).to_bytes());
        license
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn a_license_signed_by_a_known_key_reads() {
        let (signing, trusted) = issuer();
        let text = serde_json::to_string_pretty(&a_license(&signing)).unwrap();
        let read = License::read(&text, &trusted).expect("a good license");
        assert_eq!(read.company, "Example Steel, Inc.");
        assert_eq!(read.users, 10);
        // A byte-order mark, as Notepad writes one, changes nothing.
        assert!(License::read(&format!("\u{feff}{text}"), &trusted).is_ok());
    }

    #[test]
    fn a_changed_license_is_refused() {
        let (signing, trusted) = issuer();
        for change in [
            |l: &mut License| l.users = 0,
            |l: &mut License| l.updates_through = FOREVER.into(),
            |l: &mut License| l.company = "Somebody Else LLC".into(),
            |l: &mut License| l.edition = Edition::Sealed,
            |l: &mut License| l.note = "Founding customer".into(),
        ] {
            let mut license = a_license(&signing);
            change(&mut license);
            let text = serde_json::to_string(&license).unwrap();
            let why = License::read(&text, &trusted).unwrap_err();
            assert!(why.contains("does not check out"), "{why}");
        }
    }

    #[test]
    fn a_license_from_an_unknown_key_is_refused_in_words() {
        let (signing, _) = issuer();
        let other = Trusted {
            keys: vec![("someone-else".into(), SigningKey::from_bytes(&[3u8; 32]).verifying_key().to_bytes())],
        };
        let text = serde_json::to_string(&a_license(&signing)).unwrap();
        assert!(License::read(&text, &other).unwrap_err().contains("does not know"));
        assert!(License::read("hello", &other).unwrap_err().contains("not an Excalibur View license"));
    }

    #[test]
    fn a_release_signature_is_never_a_license_signature() {
        // The same publisher key signs both; the first line keeps them apart.
        let license = a_license(&issuer().0);
        let text = String::from_utf8(license.signing_payload()).unwrap();
        assert!(text.starts_with("excalibur-license-v1\n"));
        assert!(!text.starts_with("hyperview-release-v1"));
    }

    #[test]
    fn a_license_without_parts_signs_the_same_text_as_before_parts_existed() {
        // WHAT: the signed text of a license with no parts, written out. WHY: every license issued before parts existed must still check out
        let license = a_license(&issuer().0);
        assert_eq!(
            String::from_utf8(license.signing_payload()).unwrap(),
            "excalibur-license-v1\nid=evl_0001\ncompany=Example Steel, Inc.\nedition=office\nusers=10\nupdates_through=2027-10-01\nissued=2026-10-01\nnote=\n"
        );
        assert_eq!(license.parts(), vec![Module::View]); // WHAT: View only. WHY: what every office had before
        assert!(!serde_json::to_string(&license).unwrap().contains("modules")); // WHAT: no empty parts line written. WHY: the file stays as it was
    }

    #[test]
    fn parts_are_signed_and_switch_on_only_what_this_build_knows() {
        let (signing, trusted) = issuer();
        let mut license = a_license(&signing);
        license.modules = vec!["citadel".into(), "cad".into(), "someday".into()]; // WHAT: two known parts and one from a newer build. WHY: all three cases
        license.signature = hex(&signing.sign(&license.signing_payload()).to_bytes());
        assert!(String::from_utf8(license.signing_payload()).unwrap().ends_with("note=\nmodules=citadel,cad,someday\n")); // WHAT: the parts line, last. WHY: the publishing script writes the same
        let text = serde_json::to_string(&license).unwrap();
        let read = License::read(&text, &trusted).unwrap(); // WHAT: it checks out. WHY: signed with the rest
        assert_eq!(read.parts(), vec![Module::View, Module::Citadel, Module::Cad]); // WHAT: View, then the known parts; "someday" skipped. WHY: a newer license still works here
        let mut more = read.clone();
        more.modules.push("archetype".into()); // WHAT: a part added after signing. WHY: must not check out
        assert!(more.check(&trusted).is_err());
        let mut fewer = read;
        fewer.modules.clear(); // WHAT: the parts taken off after signing. WHY: must not check out either
        assert!(fewer.check(&trusted).is_err());
    }

    #[test]
    fn a_part_name_that_could_change_the_signed_text_is_refused() {
        let (signing, _) = issuer();
        for bad in ["cad,citadel", "cad\nusers=0", "Cad", "", "c=d", "-cad"] {
            let mut license = a_license(&signing);
            license.modules = vec![bad.into()]; // WHAT: a comma, a newline, capitals, nothing, "=" or a leading dash. WHY: each could make two lists sign alike
            assert!(License::parse(&serde_json::to_string(&license).unwrap()).is_err(), "{bad:?}");
        }
        let mut twice = a_license(&signing);
        twice.modules = vec!["cad".into(), "cad".into()]; // WHAT: a part named twice. WHY: refused, so one list has one signed text
        assert!(License::parse(&serde_json::to_string(&twice).unwrap()).is_err());
    }

    #[test]
    fn newlines_cannot_be_smuggled_into_the_signed_text() {
        let (signing, _) = issuer();
        let mut license = a_license(&signing);
        license.company = "Example Steel\nusers=0".into();
        let text = serde_json::to_string(&license).unwrap();
        assert!(License::parse(&text).is_err());
    }

    #[test]
    fn updates_cover_what_was_published_on_or_before_the_day() {
        let license = a_license(&issuer().0);
        assert!(license.covers("2027-10-01T23:59:00Z"));
        assert!(license.covers("2027-09-30"));
        assert!(!license.covers("2027-10-02T00:00:00Z"));
        assert!(license.covers(""), "an undated release is not held against the customer");
        let mut forever = license.clone();
        forever.updates_through = FOREVER.into();
        assert!(forever.covers("2099-01-01"));
    }

    #[test]
    fn dates_are_real_dates() {
        assert!(is_date("2026-10-01"));
        assert!(!is_date("2026-13-01"));
        assert!(!is_date("2026-1-01"));
        assert!(!is_date("forever"));
        assert_eq!(long_date("2027-10-01"), "Oct 1, 2027");
    }

    #[test]
    fn the_about_line_says_what_the_office_has() {
        let licensed = Standing {
            state: "licensed".into(),
            edition: Some("office".into()),
            company: Some("Example Steel, Inc.".into()),
            users_allowed: Some(10),
            users: 8,
            updates_through: Some("2027-10-01".into()),
            ..Default::default()
        };
        assert_eq!(
            licensed.about_line(),
            "Excalibur View Office, licensed to Example Steel, Inc. 8 of 10 users. Updates through Oct 1, 2027."
        );
        let trial = Standing { state: "trial".into(), trial_days_left: Some(1), ..Default::default() };
        assert_eq!(trial.about_line(), "Excalibur View Office trial: 1 day left.");
    }

    /// What `tools/publish.py sign-license` wrote, with the same made-up
    /// key as [`issuer`], pasted in as it came out. If the script and this
    /// file ever disagree about the signed text, this fails.
    const FROM_THE_SCRIPT: &str = r#"{
  "id": "evl_fixture01",
  "company": "Acme Steel, Inc.",
  "edition": "office",
  "users": 6,
  "updates_through": "2027-10-01",
  "issued": "2026-10-01",
  "note": "",
  "key": "mesafab-2026",
  "signature": "18a62a187b9fef69b2bb249c8f4a147b96bb11f82ca732424a7c76b4bf2652586eada410de35aed3227e4e15f2b0030fce6b042d9232baf0239416fb62dd5406"
}"#;
    const FOUNDING_FROM_THE_SCRIPT: &str = r#"{
  "id": "evl_fixture02",
  "company": "Example Steel, Inc.",
  "edition": "office",
  "users": 0,
  "updates_through": "forever",
  "issued": "2026-10-01",
  "note": "Founding customer",
  "key": "mesafab-2026",
  "signature": "b7df5f8179bccf258f885eafe2ef1c13b6755866d69ceebadc576ab8905795c61f1f63eba6baeaa630838a50fd96909281262597031d7b791713c0aa59c7ec02"
}"#;

    #[test]
    fn a_license_the_publishing_script_signed_reads_here() {
        let (_, trusted) = issuer();
        let license = License::read(FROM_THE_SCRIPT, &trusted).expect("the script's license");
        assert_eq!(license.company, "Acme Steel, Inc.");
        assert_eq!(license.users, 6);
        assert!(license.covers("2027-10-01") && !license.covers("2027-10-02"));
        let founding = License::read(FOUNDING_FROM_THE_SCRIPT, &trusted).expect("the founding one");
        assert!(founding.unlimited() && founding.forever());
        assert_eq!(founding.note, "Founding customer");
        let parts = License::read(PARTS_FROM_THE_SCRIPT, &trusted).expect("the script's license with parts"); // WHAT: a license with parts, as the script signs it. WHY: the two must agree on the parts line too
        assert_eq!(parts.parts(), vec![Module::View, Module::Citadel, Module::Cad]);
        let sealed = License::read(SEALED_FROM_THE_SCRIPT, &trusted).expect("the sealed one"); // WHAT: every field set, the edition sealed, a company in more than ASCII. WHY: the script and this file must agree on each, byte for byte
        assert_eq!((sealed.company.as_str(), sealed.edition, sealed.users, sealed.note.as_str()), ("Ünïcode Fab & Co, LLC", Edition::Sealed, 7, "Added users"));
        assert_eq!((sealed.updates_through.as_str(), sealed.issued.as_str(), sealed.id.as_str()), ("2027-09-24", "2026-09-24", "evl_fixture04"));
    }

    /// `sign-license --company "Ünïcode Fab & Co, LLC" --edition sealed --users 7 --updates-through 2027-09-24
    /// --issued 2026-09-24 --note "Added users" --id evl_fixture04`, with the same made-up key, pasted in as it
    /// came out (2026-11-10). Python writes the company with \u escapes; the signed text is its UTF-8.
    const SEALED_FROM_THE_SCRIPT: &str = r#"{
  "id": "evl_fixture04",
  "company": "\u00dcn\u00efcode Fab & Co, LLC",
  "edition": "sealed",
  "users": 7,
  "updates_through": "2027-09-24",
  "issued": "2026-09-24",
  "note": "Added users",
  "key": "mesafab-2026",
  "signature": "12d26a196f03da7ee4a20af99d2c4634e1e879b001d8c97f71921908686c67a7d8f8513a4a002521309c924ad6c62289e7ace511b843bc225e133dacf4aa6405"
}"#;

    #[test]
    fn the_signed_text_is_eight_lines_in_this_order_and_nothing_else() {
        // WHAT: the signed text of a licence, written out. WHY: tools/publish.py, the licensing service and every server must sign and check exactly this
        let license = a_license(&issuer().0);
        assert_eq!(
            String::from_utf8(license.signing_payload()).unwrap(),
            "excalibur-license-v1\nid=evl_0001\ncompany=Example Steel, Inc.\nedition=office\nusers=10\nupdates_through=2027-10-01\nissued=2026-10-01\nnote=\n"
        );
    }

    #[test]
    fn every_state_reads_as_one_plain_line_on_a_seat() {
        // WHAT: Help > About's line for each standing the server can send. WHY: what a seat shows, state by state
        let line = |state: &str| Standing { state: state.into(), company: Some("Example Steel".into()), ..Default::default() }.about_line();
        assert_eq!(line("founding"), "Excalibur View Office, Example Steel. Founding install: every user, every update.");
        assert!(line("unlicensed").contains("Sharing new work is paused; everything already here can be opened and exported."));
        let forever = Standing { state: "licensed".into(), edition: Some("sealed".into()), company: Some("Example Steel".into()), ..Default::default() };
        assert_eq!(forever.about_line(), "Excalibur View Sealed, licensed to Example Steel. Every user. Updates forever.");
        let trial = Standing { state: "trial".into(), trial_days_left: Some(30), ..Default::default() };
        assert_eq!(trial.about_line(), "Excalibur View Office trial: 30 days left.");
    }

    /// `sign-license --modules citadel,cad`, with the same made-up key, pasted in as it came out (2026-11-10).
    const PARTS_FROM_THE_SCRIPT: &str = r#"{
  "id": "evl_fixture03",
  "company": "Example Excavation",
  "edition": "office",
  "users": 0,
  "updates_through": "2027-11-10",
  "issued": "2026-11-10",
  "note": "",
  "key": "mesafab-2026",
  "modules": [
    "citadel",
    "cad"
  ],
  "signature": "a5735d07bad85d16946578e3dc795640e7418ef3460e2e3f72e634b2286f1bc3bdfdf343898fecad91075222cb813ccedee65912561c511c73f3a6653938b409"
}"#;
}
