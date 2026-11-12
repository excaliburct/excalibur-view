//! The Excalibur Office Server's parts: which are switched on here, and how
//! each one is.
//!
//! One server runs every Excalibur part an office has (docs/OFFICE-SERVER.md).
//! View is always there, because this is View's server grown. Every other part
//! is switched on by the office's signed licence, its `modules` line, and by
//! nothing else: no setting, file or request can turn a part on that the
//! licence did not.
//!
//! What a part reports here is what Fleet and the Manager see, so it is never
//! hopeful: a part this build does not carry yet says so, rather than "running".

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use hub::license::Module;
use serde::Serialize;

use crate::api::{Answer, Server, Shared};

/// One part of the Office Server, as the Manager and Fleet are told it.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Part {
    pub name: &'static str, // WHAT: the part's licence name (view, cad, archetype, citadel). WHY: the same word the licence and Fleet use
    pub title: &'static str, // WHAT: its name for people. WHY: the Manager shows it as is
    pub licensed: bool, // WHAT: whether the licence switches it on. WHY: an office sees what it has, and what it could add
    pub state: &'static str, // WHAT: running, not-in-this-build, off; for Citadel also looking, not-found, not-answering, certificate-changed. WHY: Fleet turns a licensed part that isn't running amber
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>, // WHAT: the part's running version, when it runs. WHY: Fleet compares it with the shelf
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>, // WHAT: a sentence about the part, when there is one: for Citadel, how its 1.0 data moved in. WHY: "moved N jobs", "nothing to move" or "move failed: ..." shown to an admin and to Fleet
}

/// The parts whose code this build carries. A part is added here in the same
/// change that brings its code in, and never before.
pub const IN_THIS_BUILD: &[Module] = &[Module::View, Module::Cad, Module::Archetype]; // WHAT: View, CAD's office drawings (crate::cad) and Archetype's jobs (crate::archetype). WHY: a licensed part this build can't run must say so, not pretend

/// The parts this office's licence switches on. View always; the rest only
/// from a licence that checks out.
pub fn licensed(server: &Server) -> Vec<Module> {
    match crate::license::held_license(server) {
        Some(license) => license.parts(), // WHAT: the licence's own list, its signature checked. WHY: it is signed, so nobody here can add to it; a founding office with a licence gets its parts too
        None => vec![Module::View], // WHAT: no licence: View only. WHY: every other part is paid for by a licence
    }
}

/// Whether `part` is switched on and served here.
pub fn serves(server: &Server, part: Module) -> bool {
    serves_of(&licensed(server), IN_THIS_BUILD, part) // WHAT: this office's licence, this build. WHY: the rule itself is below, where a test can reach it
}

/// [`serves`], from a licensed list and a built list.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn serves_of(licensed: &[Module], built: &[Module], part: Module) -> bool {
    built.contains(&part) && licensed.contains(&part) // WHAT: both carried and licensed. WHY: a part's routes answer 404 otherwise, as if it didn't exist
}

/// Every part Excalibur makes, with this office's standing for each.
pub fn parts(server: &Server) -> Vec<Part> {
    let citadel = if server.cell.supervised() {
        server.cell.seen().or_else(crate::citadel::last) // WHAT: the supervisor's word, else the last look at the cell's port while it starts. WHY: §6a: a cell given up on shows as not answering
    } else {
        Some(crate::citadel::Seen::NotFound) // WHAT: no cell here (no program, or not Windows). WHY: the cell is the only way Citadel runs; something else on 47110 is never reported as Citadel
    };
    with_move(parts_of(&licensed(server), IN_THIS_BUILD, citadel.as_ref()), crate::cellmove::recorded(&server.store).as_ref()) // WHAT: every part, Citadel's with how its old data moved in. WHY: shown on /api/v1/office
}

/// `parts` with Citadel's move added: its words as the note, and, after a
/// failure, the state "move-failed" and no version.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn with_move(mut parts: Vec<Part>, moved: Option<&crate::cellmove::Moved>) -> Vec<Part> {
    let Some(moved) = moved else { return parts }; // WHAT: no move recorded. WHY: nothing to add
    for part in parts.iter_mut().filter(|p| p.name == Module::Citadel.name() && p.licensed) {
        if !moved.lets_the_cell_start() {
            part.state = "move-failed"; // WHAT: the alarm. WHY: the cell was not started, and an admin must act
            part.version = None; // WHAT: nothing runs. WHY: never a version for a cell that isn't running
        }
        part.note = Some(moved.words()); // WHAT: "moved N jobs", "nothing to move" or "move failed: ...". WHY: shown on /api/v1/office
    }
    parts
}

/// [`parts`], from a licensed list, a built list and the last look at
/// Citadel. Kept apart so a test can ask about any of them.
#[must_use] // WHAT: the answer is the point of calling it. WHY: dropping it is always a mistake
pub fn parts_of(licensed: &[Module], built: &[Module], citadel: Option<&crate::citadel::Seen>) -> Vec<Part> {
    Module::ALL
        .iter()
        .map(|&part| {
            let on = licensed.contains(&part); // WHAT: switched on by the licence. WHY: decides whether it should be running
            if part == Module::Citadel && on {
                return Part {
                    name: part.name(),
                    title: part.title(),
                    licensed: true,
                    state: citadel.map_or("looking", |s| s.state()), // WHAT: what the last look saw, or "looking" before the first. WHY: Citadel runs alongside, so its state is what it answers
                    version: citadel.and_then(super::citadel::Seen::version), // WHAT: Citadel's own version. WHY: Fleet compares it with Citadel's shelf
                    note: None, // WHAT: none yet. WHY: `parts` adds the move's result
                };
            }
            let state = match (on, built.contains(&part)) {
                (true, true) => "running", // WHAT: licensed and carried. WHY: it is served
                (true, false) => "not-in-this-build", // WHAT: licensed, but this build lacks it. WHY: Fleet shows it needs an update, never a false green
                (false, _) => "off", // WHAT: not licensed. WHY: shown so an office knows it exists
            };
            Part {
                name: part.name(),
                title: part.title(),
                licensed: on,
                state,
                version: (state == "running").then(|| crate::VERSION.to_string()), // WHAT: the running build's version. WHY: every part carried today ships inside this server
                note: None, // WHAT: nothing to add. WHY: only Citadel's move has a sentence
            }
        })
        .collect()
}

/// The Office Server, as a whole.
#[derive(Serialize)]
pub struct Office {
    pub product: &'static str, // WHAT: always excalibur-office. WHY: the Manager knows it is talking to an Office Server
    pub version: String, // WHAT: the server's version. WHY: the same as /api/version
    pub parts: Vec<Part>, // WHAT: every part and its standing. WHY: one place to see what this office runs
}

/// `GET /api/v1/office`: the parts this server runs. Anybody signed in may ask,
/// as they may ask for the licence's standing.
pub(crate) async fn read(State(server): State<Shared>, headers: HeaderMap) -> Answer<Json<Office>> {
    crate::api::caller(&server, &headers)?; // WHAT: anybody signed in, or a key. WHY: the same door as /license; nothing here is secret within the office
    Ok(Json(Office { product: "excalibur-office", version: crate::VERSION.to_string(), parts: parts(&server) }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_of<'a>(parts: &'a [Part], name: &str) -> &'a Part {
        parts.iter().find(|p| p.name == name).expect("every part is listed") // WHAT: one part by name. WHY: the tests read each
    }

    #[test]
    fn an_office_without_parts_runs_view_and_nothing_else() {
        let parts = parts_of(&[Module::View], IN_THIS_BUILD, None); // WHAT: a licence with no parts line, or none at all. WHY: every office today
        assert_eq!(parts.len(), Module::ALL.len(), "every part is listed, switched on or not");
        assert_eq!(state_of(&parts, "view").state, "running");
        assert_eq!(state_of(&parts, "view").version.as_deref(), Some(crate::VERSION));
        for name in ["cad", "archetype", "citadel"] {
            assert_eq!(state_of(&parts, name).state, "off", "{name} isn't licensed");
            assert!(!state_of(&parts, name).licensed);
            assert!(state_of(&parts, name).version.is_none(), "{name} reports no version it isn't running");
        }
    }

    #[test]
    fn a_licensed_part_this_build_lacks_says_so_and_never_claims_to_run() {
        let parts = parts_of(&[Module::View, Module::Archetype], &[Module::View], None); // WHAT: Archetype licensed on a build without it. WHY: the gap Fleet must see
        assert_eq!(state_of(&parts, "archetype").state, "not-in-this-build");
        assert!(state_of(&parts, "archetype").licensed);
        assert!(state_of(&parts, "archetype").version.is_none());
        let later = parts_of(&[Module::View, Module::Archetype], &[Module::View, Module::Archetype, Module::Cad], None); // WHAT: a build that carries it, and CAD unlicensed. WHY: then it runs, and CAD still doesn't
        assert_eq!(state_of(&later, "archetype").state, "running");
        assert_eq!(state_of(&later, "cad").state, "off", "carried but not licensed stays off");
    }

    #[test]
    fn citadel_is_what_its_last_look_saw_and_off_unless_licensed() {
        use crate::citadel::Seen;
        let on = [Module::View, Module::Citadel]; // WHAT: a licence with Citadel. WHY: an office that runs Citadel
        assert_eq!(state_of(&parts_of(&on, IN_THIS_BUILD, None), "citadel").state, "looking", "before the first look");
        let running = Seen::Running { version: "1.0.2".into() };
        let part = parts_of(&on, IN_THIS_BUILD, Some(&running));
        assert_eq!(state_of(&part, "citadel").state, "running");
        assert_eq!(state_of(&part, "citadel").version.as_deref(), Some("1.0.2"), "Citadel's own version, not this server's");
        let changed = Seen::CertificateChanged { seen: "b".repeat(64) };
        assert_eq!(state_of(&parts_of(&on, IN_THIS_BUILD, Some(&changed)), "citadel").state, "certificate-changed");
        assert_eq!(state_of(&parts_of(&on, IN_THIS_BUILD, Some(&Seen::NotFound)), "citadel").state, "not-found");
        let unlicensed = parts_of(&[Module::View], IN_THIS_BUILD, Some(&running)); // WHAT: Citadel running, but not licensed. WHY: the licence still decides
        assert_eq!(state_of(&unlicensed, "citadel").state, "off");
        assert!(state_of(&unlicensed, "citadel").version.is_none());
    }

    #[test]
    fn a_part_is_served_only_when_both_carried_and_licensed() {
        let built = [Module::View, Module::Cad]; // WHAT: a build carrying CAD. WHY: so licensing alone decides
        assert!(serves_of(&[Module::View, Module::Cad], &built, Module::Cad), "licensed and carried");
        assert!(!serves_of(&[Module::View], &built, Module::Cad), "carried but not licensed is not served");
        assert!(!serves_of(&[Module::View, Module::Citadel], &built, Module::Citadel), "licensed but not carried is not served");
        assert!(serves_of(&[Module::View], IN_THIS_BUILD, Module::View), "View, always");
    }

    #[test]
    fn citadel_s_move_is_said_and_a_failed_one_is_an_alarm() {
        use crate::cellmove::Moved;
        let on = [Module::View, Module::Citadel];
        let running = crate::citadel::Seen::Running { version: "1.1.0".into() };
        let moved = with_move(parts_of(&on, IN_THIS_BUILD, Some(&running)), Some(&Moved::Moved { jobs: 12 }));
        assert_eq!(state_of(&moved, "citadel").note.as_deref(), Some("moved 12 jobs"));
        assert_eq!(state_of(&moved, "citadel").state, "running");
        let failed = with_move(parts_of(&on, IN_THIS_BUILD, Some(&running)), Some(&Moved::Failed { reason: "no access".into() }));
        assert_eq!(state_of(&failed, "citadel").state, "move-failed", "an alarm, never running");
        assert!(state_of(&failed, "citadel").version.is_none());
        assert!(state_of(&failed, "citadel").note.as_deref().unwrap_or_default().starts_with("move failed: no access"));
        assert!(state_of(&with_move(parts_of(&[Module::View], IN_THIS_BUILD, None), Some(&Moved::Nothing)), "citadel").note.is_none(), "unlicensed, nothing said");
    }

    #[test]
    fn this_build_carries_view_cad_and_archetype_and_nothing_else_until_a_part_is_brought_in() {
        assert_eq!(IN_THIS_BUILD, &[Module::View, Module::Cad, Module::Archetype]); // WHAT: pinned. WHY: adding a part here must be a deliberate, reviewed change
    }
}
