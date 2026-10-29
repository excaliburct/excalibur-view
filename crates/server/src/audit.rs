//! The audit log: who reached which drawing, and when.
//!
//! An office handling Controlled Unclassified Information is asked by an
//! auditor to show exactly this, and "we don't keep that" is not an answer
//! that passes. NIST SP 800-171's audit and accountability family wants
//! records that let you trace an action to the person who took it, and wants
//! them retained and reviewable.
//!
//! **What is recorded** is reaching and changing: signing in and failing to,
//! opening or downloading a drawing set, exporting a takeoff, adding or
//! removing people, changing what a person may do, adding a license, taking a
//! key. **What is not recorded** is every markup as it is drawn. A log nobody
//! can read is a log nobody reads, and the markups are themselves already a
//! signed record of who drew what.
//!
//! **Append only.** Nothing in the program edits or deletes a row. Whoever
//! administers the machine can of course reach the database directly; that is
//! true of every log ever kept, and is the reason an auditor asks who
//! administers the machine rather than trusting the software.
//!
//! The log lives on the office's own server with everything else. It is never
//! sent anywhere, sealed or not.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// RFC 3339, in UTC.
    pub at: String,
    /// The person's id, or `None` for something that happened before anybody
    /// was identified — a sign-in that failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<String>,
    /// Their email as it read at the time, so the row still says who after
    /// the account is gone.
    pub email: String,
    /// One of the constants below.
    pub action: String,
    /// What it was done to: a set's name, a person's email, a license id.
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub detail: String,
    /// Where from, when the server can tell.
    #[serde(default)]
    pub address: String,
}

/// What can be recorded. Kept as a short fixed list so a log can be read and
/// filtered by somebody who has never seen this program before.
///
/// Every one of these is written somewhere. A constant defined and never
/// recorded is worse than no constant: it reads, to anybody auditing the
/// program, like coverage that is not there. `OPENED_SET` used to be one --
/// it came out rather than being wired up, because a seat asks about a set
/// every eight seconds and a log of that is a log nobody reads.
pub mod action {
    pub const SIGNED_IN: &str = "signed in";
    pub const SIGN_IN_REFUSED: &str = "sign-in refused";
    pub const SIGNED_OUT: &str = "signed out";
    pub const DOWNLOADED_SET: &str = "downloaded a drawing set";
    pub const EXPORTED_TAKEOFF: &str = "exported a takeoff";
    pub const UPLOADED_SET: &str = "uploaded a drawing set";
    pub const ADDED_PERSON: &str = "added a person";
    pub const REMOVED_PERSON: &str = "removed a person";
    pub const CHANGED_ROLE: &str = "changed what a person may do";
    pub const CHANGED_PASSWORD: &str = "changed a password";
    pub const ADDED_LICENSE: &str = "added a license";
    pub const MADE_KEY: &str = "made a key for another program";
    pub const REVOKED_KEY: &str = "took back a key";
    pub const CHANGED_SETTING: &str = "changed a server setting";
    pub const KEY_REFUSED: &str = "a key asked for something it is not for"; // WHAT: a key refused by its purpose. WHY: a sync key straying off its list is worth a line someone can find.
    pub const MADE_GROUP: &str = "made a group of people"; // WHAT: a group made. WHY: groups decide who sees which folder.
    pub const CHANGED_GROUP: &str = "changed who is in a group"; // WHAT: somebody put in a group or taken out. WHY: that changes which folders they see.
    pub const CHANGED_FOLDER: &str = "changed who sees a folder"; // WHAT: a folder's rule set or changed. WHY: somebody will ask when a folder opened to the field.
    pub const UPLOADED_FILE: &str = "put a file in a folder"; // WHAT: a file that isn't a drawing put in a project. WHY: the same line a drawing set gets.
    pub const DOWNLOADED_FILE: &str = "downloaded a file"; // WHAT: a file taken off the server. WHY: reaching a file is the line an auditor wants, as for a drawing.
    pub const MOVED: &str = "moved to another folder"; // WHAT: a set or a file moved. WHY: moving one can change who sees it.
}

impl Entry {
    pub fn new(action: &str, email: &str) -> Entry {
        Entry {
            at: now(),
            person: None,
            email: email.to_string(),
            action: action.to_string(),
            subject: String::new(),
            detail: String::new(),
            address: String::new(),
        }
    }

    pub fn by(mut self, person: &str) -> Entry {
        self.person = Some(person.to_string());
        self
    }

    pub fn about(mut self, subject: impl Into<String>) -> Entry {
        self.subject = subject.into();
        self
    }

    pub fn saying(mut self, detail: impl Into<String>) -> Entry {
        self.detail = detail.into();
        self
    }

    pub fn from(mut self, address: impl Into<String>) -> Entry {
        self.address = address.into();
        self
    }

    /// One line, the way it reads in an export.
    pub fn line(&self) -> String {
        let mut said = format!("{}  {}  {}", self.at, self.email, self.action);
        if !self.subject.is_empty() {
            said.push_str(&format!(": {}", self.subject));
        }
        if !self.detail.is_empty() {
            said.push_str(&format!(" ({})", self.detail));
        }
        said
    }
}

pub fn now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// The whole log as a CSV an auditor can open, quoted properly.
pub fn as_csv(entries: &[Entry]) -> String {
    let mut out = String::from("when,who,action,subject,detail,from\n");
    for e in entries {
        out.push_str(&format!(
            "{},{},{},{},{},{}\n",
            field(&e.at),
            field(&e.email),
            field(&e.action),
            field(&e.subject),
            field(&e.detail),
            field(&e.address)
        ));
    }
    out
}

fn field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_entry_reads_as_a_sentence() {
        let e = Entry::new(action::DOWNLOADED_SET, "pat@acme.test")
            .by("p1")
            .about("S-101 through S-140")
            .from("192.168.1.22");
        assert!(e.line().contains("pat@acme.test"));
        assert!(e.line().contains("downloaded a drawing set"));
        assert!(e.line().contains("S-101 through S-140"));
    }

    #[test]
    fn a_refused_sign_in_is_recorded_without_a_person() {
        let e = Entry::new(action::SIGN_IN_REFUSED, "someone@nowhere.test");
        assert!(e.person.is_none(), "nobody was identified, and that is the point");
        assert_eq!(e.email, "someone@nowhere.test");
    }

    #[test]
    fn a_comma_in_a_company_name_does_not_break_the_csv() {
        let e = Entry::new(action::ADDED_LICENSE, "sam@example.com")
            .about("Example Steel, Inc.")
            .saying("6 users, updates through 2027-09-22");
        let csv = as_csv(&[e]);
        assert!(csv.contains("\"Example Steel, Inc.\""));
        assert!(csv.contains("\"6 users, updates through 2027-09-22\""));
        assert_eq!(csv.lines().count(), 2, "a header and one row");
    }

    #[test]
    fn a_quote_in_a_field_is_doubled_not_dropped() {
        let e = Entry::new(action::UPLOADED_SET, "pat@acme.test").about("the \"as-built\" set");
        assert!(as_csv(&[e]).contains("\"the \"\"as-built\"\" set\""));
    }

    #[test]
    fn the_time_is_utc_and_sorts_as_text() {
        let e = Entry::new(action::SIGNED_IN, "pat@acme.test");
        assert!(e.at.ends_with('Z') || e.at.contains('+'), "{}", e.at);
        assert_eq!(e.at.len() >= 20, true, "{}", e.at);
    }
}
