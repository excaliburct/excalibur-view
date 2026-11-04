//! Chests from outside this repository, read the way the program reads them.
//!
//! The six trade chests are made from data that does not live here and must
//! not — they are the thing being sold — so this asks for them rather than
//! carrying them:
//!
//! ```text
//! EXCALIBUR_TEST_CHESTS=/somewhere/chests cargo test -p chest --test real_chests
//! ```
//!
//! With the variable unset every test here says so and passes. A test that
//! fails on every machine but one is a test people learn to ignore.
//!
//! What it is for: parsing a chest and loading a chest are not the same
//! thing. `arrivals::read_chest` parses the file, writes it back out in Excalibur
//! View's own format, and then reads the seat's whole arrangement off disk
//! again — and it is that second reading, not the first, that decides what
//! somebody sees on the toolbar. A chest that parses and then loses its
//! weights on the way back out would look fine in every test that stops
//! after the first step.

use std::path::PathBuf;

/// The folder of chests, or `None` with a line saying why not.
fn chests() -> Option<Vec<PathBuf>> {
    let folder = match std::env::var("EXCALIBUR_TEST_CHESTS") {
        Ok(folder) if !folder.trim().is_empty() => PathBuf::from(folder),
        _ => {
            println!(
                "EXCALIBUR_TEST_CHESTS is not set, so there are no chests to read. \
                 Set it to a folder of .evtools files to run this."
            );
            return None;
        }
    };
    let mut found: Vec<PathBuf> = std::fs::read_dir(&folder)
        .unwrap_or_else(|e| panic!("{}: {e}", folder.display()))
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("evtools"))
        .collect();
    found.sort();
    assert!(!found.is_empty(), "{} has no .evtools in it", folder.display());
    Some(found)
}

fn name_of(path: &std::path::Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().to_string()
}

#[test]
fn every_chest_reads_and_has_tools_in_it() {
    let Some(files) = chests() else { return };
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let name = name_of(&path);
        let profile = chest::Profile::read(&bytes)
            .unwrap_or_else(|| panic!("{name} did not read as a tool chest at all"));

        let tools: usize = profile.sets.iter().map(|s| s.tools.len()).sum();
        assert!(!profile.sets.is_empty(), "{name} has no tool sets");
        assert!(tools > 0, "{name} has sets but no tools in them");

        // Every tool has to have something to group by. A blank subject is a
        // tool that lands in the markups list under nothing and totals with
        // nothing, which is worse than a tool that is missing.
        for set in &profile.sets {
            assert!(!set.title.trim().is_empty(), "{name} has a set with no title");
            for tool in &set.tools {
                assert!(
                    !tool.subject.trim().is_empty(),
                    "{name}, set {:?}: a tool with no subject",
                    set.title
                );
            }
        }
        println!("{name}: {} sets, {tools} tools", profile.sets.len());
    }
}

#[test]
fn a_chest_still_weighs_the_same_after_being_loaded() {
    let Some(files) = chests() else { return };
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let name = name_of(&path);
        let first = chest::Profile::read(&bytes).unwrap();

        // What `arrivals::read_chest` does with it: written back out in Excalibur
        // View's own format, then read again — which is the copy the seat
        // actually uses.
        assert!(
            chest::native::is_native(&bytes),
            "{name} is not in Excalibur View's own format"
        );
        let written = chest::native::write(&first, "");
        let again = chest::Profile::read(&written)
            .unwrap_or_else(|| panic!("{name} would not read back after being written out"));

        assert_eq!(first.sets.len(), again.sets.len(), "{name}: sets lost on the way back");
        for (before, after) in first.sets.iter().zip(again.sets.iter()) {
            assert_eq!(before.title, after.title, "{name}: a set changed its name");
            assert_eq!(
                before.tools.len(),
                after.tools.len(),
                "{name}, set {:?}: tools lost on the way back",
                before.title
            );
            for (was, is) in before.tools.iter().zip(after.tools.iter()) {
                assert_eq!(was.subject, is.subject, "{name}: a tool changed its subject");
                assert_eq!(was.kind, is.kind, "{name}, {}: changed what it measures", was.subject);
                assert_eq!(
                    was.pounds_per_foot(),
                    is.pounds_per_foot(),
                    "{name}, {}: lost its pounds per foot",
                    was.subject
                );
                assert_eq!(
                    was.pounds_per_square_foot(),
                    is.pounds_per_square_foot(),
                    "{name}, {}: lost its pounds per square foot",
                    was.subject
                );
            }
        }

        let weighed: usize = again.sets.iter().map(|s| s.weighted()).sum();
        println!("{name}: {weighed} tools carry a weight, and still do after loading");
    }
}

#[test]
fn the_tools_carry_a_markup_to_stamp_down() {
    let Some(files) = chests() else { return };
    for path in files {
        let bytes = std::fs::read(&path).unwrap();
        let name = name_of(&path);
        let profile = chest::Profile::read(&bytes).unwrap();

        // A tool with no annotation dictionary is a name on a button that
        // draws nothing when it is clicked.
        let mut empty = Vec::new();
        for set in &profile.sets {
            for tool in &set.tools {
                if tool.annotation.is_empty() && !tool.properties_only {
                    empty.push(format!("{}/{}", set.title, tool.subject));
                }
            }
        }
        assert!(
            empty.is_empty(),
            "{name}: {} tool(s) draw nothing, first few: {:?}",
            empty.len(),
            &empty[..empty.len().min(5)]
        );
    }
}
