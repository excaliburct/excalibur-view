//! Printing on Linux and the Mac, through CUPS, the print system both use.
//!
//! CUPS is asked with its own small programs, the way a terminal would ask
//! it: `lpstat` for the printers and the default one, `lpoptions` for the
//! paper sizes a printer offers, and `lp` to send the sheets. Nothing is
//! linked against it, so a computer without CUPS still runs the program; it
//! just has no printers to list, and printing falls back to opening the
//! sheets in whatever reads PDFs there.
//!
//! The programs are always run directly, never through a shell, so a printer
//! or file name with spaces or odd characters in it goes through as it is.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// A paper size a printer offers: CUPS's own name for it, which is what
/// goes back to `lp`, and its size where the name says or is well known.
#[derive(Clone, Debug, PartialEq)]
pub struct Paper {
    /// CUPS's name: `Letter`, `ARCHD`, `na_letter_8.5x11in`.
    pub keyword: String,
    /// Across and down, in inches, when known.
    pub inches: Option<(f32, f32)>,
}

/// What goes to the printer with the sheets.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    /// How many of each.
    pub copies: u16,
    /// Whole sets one after another rather than all the first sheets, then
    /// all the second.
    pub collate: bool,
    /// The paper, by CUPS's name; `None` for the printer's own.
    pub media: Option<String>,
    /// Fitted to the paper rather than at the sheet's own size.
    pub fit_to_paper: bool,
    /// `Some(true)` for landscape, `Some(false)` for portrait, `None` to let
    /// CUPS turn each sheet to suit the paper.
    pub landscape: Option<bool>,
    /// The job's name in the printer's queue.
    pub title: String,
}

impl Default for Choice {
    fn default() -> Choice {
        Choice {
            copies: 1,
            collate: true,
            media: None,
            fit_to_paper: true,
            landscape: None,
            title: String::new(),
        } // WHAT: one copy, fitted, turned to suit. WHY: what Batch Print and an untouched dialog mean.
    }
}

/// The CUPS programs, found on the system's PATH, or on another for a test.
#[derive(Clone, Debug, Default)]
pub struct Cups {
    /// A PATH to look for the programs on instead of the system's.
    path: Option<OsString>,
}

impl Cups {
    /// The system's own CUPS.
    pub fn system() -> Cups {
        Cups { path: None } // WHAT: the programs on the ordinary PATH. WHY: where CUPS puts them.
    }

    /// The programs found on this PATH instead: a test's stand-ins.
    pub fn on_path(path: impl Into<OsString>) -> Cups {
        Cups {
            path: Some(path.into()),
        } // WHAT: a PATH of its own. WHY: tests put a pretend lpstat and lp on it.
    }

    fn command(&self, program: &str) -> Command {
        let mut command = Command::new(program); // WHAT: the program, by name. WHY: found on the PATH, as a terminal finds it.
        if let Some(path) = &self.path {
            command.env("PATH", path); // WHAT: this PATH for finding it. WHY: a test's stand-ins.
        }
        command.env("LC_ALL", "C"); // WHAT: answers in plain English. WHY: lpstat's words are translated, and they are read here.
        command
    }

    /// Runs one of the programs and gives back what it printed, or `None`
    /// when it is not there or failed.
    fn ask(&self, program: &str, args: &[&str]) -> Option<String> {
        let out = self.command(program).args(args).output().ok()?; // WHAT: run it and wait. WHY: these answer at once.
        if !out.status.success() {
            return None; // WHAT: it failed. WHY: its answer means nothing then.
        }
        Some(String::from_utf8_lossy(&out.stdout).to_string()) // WHAT: what it printed. WHY: read below.
    }

    /// The printers, and which is the default: `None` when CUPS is not there
    /// at all, which is a different thing from CUPS with no printers.
    pub fn printers(&self) -> Option<(Vec<String>, Option<String>)> {
        let list = match self.ask("lpstat", &["-e"]) {
            Some(text) => names_from_e(&text), // WHAT: `lpstat -e`, one name a line. WHY: CUPS 2's plainest list.
            None => names_from_a(&self.ask("lpstat", &["-a"])?), // WHAT: `lpstat -a` instead. WHY: older CUPS has no -e; with neither, CUPS is not there.
        };
        let default = self
            .ask("lpstat", &["-d"])
            .and_then(|t| default_from(&t))
            .filter(|d| list.contains(d)); // WHAT: the default, when it is one of the printers. WHY: a default that has been taken away is no default.
        Some((list, default))
    }

    /// The paper sizes a printer offers, and which it uses unless told.
    pub fn papers(&self, printer: &str) -> (Vec<Paper>, Option<String>) {
        let Some(text) = self.ask("lpoptions", &["-p", printer, "-l"]) else {
            return (Vec::new(), None); // WHAT: nothing known. WHY: the printer's own paper is used.
        };
        let (keywords, default) = media_from(&text); // WHAT: the names offered. WHY: what the Paper list shows.
        let papers = keywords
            .into_iter()
            .map(|keyword| Paper {
                inches: size_of(&keyword),
                keyword,
            })
            .collect(); // WHAT: each with its size where known. WHY: the preview needs it.
        (papers, default)
    }

    /// Sends a PDF to a printer.
    pub fn print(&self, printer: &str, file: &Path, choice: &Choice) -> Result<(), String> {
        let out = self
            .command("lp")
            .args(lp_args(printer, choice))
            .arg(file) // WHAT: the file, after `--`. WHY: a name starting with a dash is still a file.
            .output()
            .map_err(|e| format!("The print system (CUPS) could not be started: {e}"))?; // WHAT: lp missing. WHY: said plainly.
        if out.status.success() {
            return Ok(()); // WHAT: queued. WHY: the printer has it.
        }
        let said = String::from_utf8_lossy(&out.stderr).trim().to_string(); // WHAT: what lp said. WHY: passed on as it is.
        Err(if said.is_empty() {
            format!("{printer} would not take the sheets.")
        } else {
            format!("{printer} would not take the sheets: {said}")
        }) // WHAT: why it failed. WHY: the person can act on it.
    }
}

/// What `lp` is told, before the file: the printer, copies, paper,
/// orientation, sizing and the job's name.
pub fn lp_args(printer: &str, choice: &Choice) -> Vec<String> {
    let mut args = vec!["-d".to_string(), printer.to_string()]; // WHAT: this printer. WHY: not just the default, which is all plain `lp` used.
    args.push("-n".into()); // WHAT: the number of copies. WHY: as chosen.
    args.push(choice.copies.max(1).to_string()); // WHAT: at least one. WHY: nought copies is not a print.
    if choice.copies > 1 {
        args.push("-o".into());
        args.push(format!("collate={}", choice.collate)); // WHAT: whole sets or not. WHY: only means anything with more than one copy.
    }
    if let Some(media) = choice.media.as_deref().filter(|m| !m.is_empty()) {
        args.push("-o".into());
        args.push(format!("media={media}")); // WHAT: the paper. WHY: as chosen from what the printer offers.
    }
    match choice.landscape {
        Some(true) => {
            args.push("-o".into());
            args.push("landscape".into()); // WHAT: turned a quarter. WHY: chosen.
        }
        Some(false) => {
            args.push("-o".into());
            args.push("orientation-requested=3".into()); // WHAT: upright. WHY: chosen; 3 is IPP's portrait.
        }
        None => {} // WHAT: nothing said. WHY: CUPS turns each sheet to suit the paper, as Auto means.
    }
    args.push("-o".into());
    args.push(if choice.fit_to_paper {
        "fit-to-page".into()
    } else {
        "print-scaling=none".into()
    }); // WHAT: fitted, or the sheet's own size. WHY: a scaled drawing printed at actual size must not be shrunk.
    if !choice.title.is_empty() {
        args.push("-t".into());
        args.push(choice.title.clone()); // WHAT: the job's name. WHY: so it can be picked out in the printer's queue.
    }
    args.push("--".into()); // WHAT: the end of the options. WHY: whatever follows is the file.
    args
}

/// The names in `lpstat -e`'s answer: one a line.
pub fn names_from_e(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(String::from)
        .collect() // WHAT: every line that has a name on it. WHY: that is all -e prints.
}

/// The names in `lpstat -a`'s answer: the first word of each line.
pub fn names_from_a(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.split_whitespace().next())
        .map(String::from)
        .collect() // WHAT: "NAME accepting requests since ..." to NAME. WHY: the name comes first.
}

/// The default in `lpstat -d`'s answer, if there is one.
pub fn default_from(text: &str) -> Option<String> {
    let line = text.lines().find(|l| l.contains("default destination"))?; // WHAT: the line that says. WHY: the answer is one sentence.
    let (_, name) = line.split_once(':')?; // WHAT: after the colon. WHY: "system default destination: NAME"; "no system default destination" has none.
    let name = name.trim(); // WHAT: without spaces. WHY: a name, not a phrase.
    (!name.is_empty()).then(|| name.to_string())
}

/// The paper sizes in `lpoptions -p NAME -l`'s answer, and the one marked
/// with a star as the printer's own. The `PageSize` line is used, or a
/// `media` line when a printer offers that instead.
pub fn media_from(text: &str) -> (Vec<String>, Option<String>) {
    let line = text
        .lines()
        .find(|l| l.starts_with("PageSize/") || l.starts_with("PageSize:"))
        .or_else(|| {
            text.lines()
                .find(|l| l.starts_with("media/") || l.starts_with("media:"))
        }); // WHAT: the paper line. WHY: PageSize for most drivers, media for some.
    let Some((_, values)) = line.and_then(|l| l.split_once(':')) else {
        return (Vec::new(), None); // WHAT: no paper offered. WHY: the printer's own is used.
    };
    let mut default = None; // WHAT: the starred one. WHY: the printer's own.
    let mut out = Vec::new(); // WHAT: every size. WHY: the list to choose from.
    for word in values.split_whitespace() {
        let keyword = word.trim_start_matches('*'); // WHAT: the name without its star. WHY: the star only marks the default.
        if keyword.is_empty()
            || keyword == "Custom.WIDTHxHEIGHT"
            || out.iter().any(|k: &String| k == keyword)
        {
            continue; // WHAT: skipped. WHY: the custom placeholder is not a size, and each once.
        }
        if word.starts_with('*') {
            default = Some(keyword.to_string()); // WHAT: the default. WHY: shown as chosen until somebody picks another.
        }
        out.push(keyword.to_string());
    }
    (out, default)
}

/// A paper's size from its name, in inches: the common names, PWG's own
/// names (which carry their size), and PPD's `w612h792` (in points).
pub fn size_of(keyword: &str) -> Option<(f32, f32)> {
    let base = keyword.split('.').next().unwrap_or(keyword); // WHAT: without ".Borderless" and the like. WHY: the same paper.
    let known: &[(&str, (f32, f32))] = &[
        ("letter", (8.5, 11.0)),
        ("legal", (8.5, 14.0)),
        ("tabloid", (11.0, 17.0)),
        ("ledger", (17.0, 11.0)),
        ("executive", (7.25, 10.5)),
        ("11x17", (11.0, 17.0)),
        ("ansic", (17.0, 22.0)),
        ("ansid", (22.0, 34.0)),
        ("ansie", (34.0, 44.0)),
        ("archa", (9.0, 12.0)),
        ("archb", (12.0, 18.0)),
        ("archc", (18.0, 24.0)),
        ("archd", (24.0, 36.0)),
        ("arche", (36.0, 48.0)),
        ("arche1", (30.0, 42.0)),
        ("a0", (33.11, 46.81)),
        ("a1", (23.39, 33.11)),
        ("a2", (16.54, 23.39)),
        ("a3", (11.69, 16.54)),
        ("a4", (8.27, 11.69)),
        ("a5", (5.83, 8.27)),
    ]; // WHAT: the sizes a fabricator's printers and plotters offer. WHY: so the preview shows the real paper.
    let lower = base.to_ascii_lowercase(); // WHAT: compared without case. WHY: drivers spell them every way.
    if let Some((_, size)) = known.iter().find(|(name, _)| *name == lower) {
        return Some(*size); // WHAT: a known name. WHY: its size.
    }
    if let Some(rest) = lower.strip_prefix('w') {
        if let Some((w, h)) = rest.split_once('h') {
            if let (Ok(w), Ok(h)) = (w.parse::<f32>(), h.parse::<f32>()) {
                return Some((w / 72.0, h / 72.0)); // WHAT: a PPD size in points. WHY: "w612h792" is letter.
            }
        }
    }
    let last = lower.rsplit('_').next()?; // WHAT: PWG's size part, "8.5x11in" or "210x297mm". WHY: the name carries its size.
    let (number, per_inch) = match last.strip_suffix("in") {
        Some(n) => (n, 1.0),                      // WHAT: inches. WHY: "8.5x11in".
        None => (last.strip_suffix("mm")?, 25.4), // WHAT: millimetres, or no size at all. WHY: "210x297mm"; anything else says no size.
    };
    let (w, h) = number.split_once('x')?; // WHAT: across by down. WHY: as PWG writes it.
    Some((
        w.parse::<f32>().ok()? / per_inch,
        h.parse::<f32>().ok()? / per_inch,
    )) // WHAT: in inches. WHY: what the preview works in.
}

/// A paper's name as a person reads it: "Letter (8.5 × 11 in)".
pub fn label_of(keyword: &str) -> String {
    let base = keyword.split('.').next().unwrap_or(keyword); // WHAT: without ".Borderless". WHY: shorter, and the same paper.
    let name = if base.contains('_') {
        base.split('_')
            .nth(1)
            .unwrap_or(base)
            .replace('-', " ")
            .to_uppercase() // WHAT: PWG's middle part, "arch-d" to "ARCH D". WHY: the part a person recognises.
    } else {
        base.to_string() // WHAT: the name as the driver gives it. WHY: already readable.
    };
    let borderless = keyword.contains(".Borderless") || keyword.contains(".Fullbleed"); // WHAT: printed to the edge. WHY: worth saying, it is a different choice.
    let size = size_of(keyword)
        .map(|(w, h)| format!(" ({} × {} in)", trim(w), trim(h)))
        .unwrap_or_default(); // WHAT: its size. WHY: two printers can call the same paper different things.
    format!(
        "{name}{size}{}",
        if borderless { ", borderless" } else { "" }
    )
}

fn trim(n: f32) -> String {
    let s = format!("{n:.2}"); // WHAT: two places. WHY: enough for any paper.
    s.trim_end_matches('0').trim_end_matches('.').to_string() // WHAT: "8.50" to "8.5", "11.00" to "11". WHY: reads as people write sizes.
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    /// A folder with pretend CUPS programs in it. Each prints what it is
    /// given here; `lp` also writes the words it was called with to a file.
    fn pretend(name: &str, programs: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("exv-cups-{name}-{}", std::process::id())); // WHAT: a folder of this test's own. WHY: tests run side by side.
        let _ = std::fs::remove_dir_all(&dir); // WHAT: nothing left from before. WHY: a clean start.
        std::fs::create_dir_all(&dir).unwrap(); // WHAT: make it. WHY: the programs go in it.
        for (program, script) in programs {
            let path = dir.join(program); // WHAT: the program's file. WHY: found on the PATH by its name.
            std::fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap(); // WHAT: a little shell script. WHY: stands in for CUPS's own.
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            // WHAT: runnable. WHY: a program has to be.
        }
        dir
    }

    const LPSTAT: &str = r#"case "$1" in
-e) printf 'Office_Laser\nPlotter_T830\n' ;;
-d) printf 'system default destination: Plotter_T830\n' ;;
*) exit 1 ;;
esac"#; // WHAT: two printers, the plotter the default. WHY: what a shop's computer has.

    #[test]
    fn the_printers_and_the_default_come_from_lpstat() {
        let dir = pretend("list", &[("lpstat", LPSTAT)]); // WHAT: a pretend lpstat. WHY: no real printers here.
        let (list, default) = Cups::on_path(&dir).printers().expect("CUPS is there"); // WHAT: ask it. WHY: as the Print dialog does.
        assert_eq!(
            list,
            vec!["Office_Laser".to_string(), "Plotter_T830".into()]
        ); // WHAT: both printers. WHY: every printer is offered, not just the default.
        assert_eq!(default.as_deref(), Some("Plotter_T830")); // WHAT: the default named. WHY: the dialog opens on it.
    }

    #[test]
    fn older_cups_without_dash_e_is_read_from_dash_a() {
        let script = r#"case "$1" in
-a) printf 'Office_Laser accepting requests since Mon 01 Jan 2024\n' ;;
-d) printf 'no system default destination\n' ;;
*) exit 1 ;;
esac"#; // WHAT: an lpstat with no -e and no default. WHY: older CUPS.
        let dir = pretend("older", &[("lpstat", script)]); // WHAT: put it on a PATH. WHY: as above.
        let (list, default) = Cups::on_path(&dir).printers().expect("CUPS is there"); // WHAT: ask. WHY: the same question.
        assert_eq!(list, vec!["Office_Laser".to_string()]); // WHAT: the printer read off -a. WHY: the fallback works.
        assert_eq!(default, None); // WHAT: no default. WHY: none was set.
    }

    #[test]
    fn no_cups_at_all_is_told_apart_from_cups_with_no_printers() {
        let empty = pretend("none", &[]); // WHAT: a PATH with nothing on it. WHY: a computer without CUPS.
        assert_eq!(Cups::on_path(&empty).printers(), None); // WHAT: no CUPS. WHY: the dialog says so and offers the PDF reader instead.
        let quiet = pretend("quiet", &[("lpstat", "exit 0")]); // WHAT: CUPS with nothing set up. WHY: a fresh install.
        assert_eq!(Cups::on_path(&quiet).printers(), Some((Vec::new(), None))); // WHAT: CUPS, no printers. WHY: a different thing to tell somebody.
    }

    #[test]
    fn the_papers_a_printer_offers_come_from_lpoptions_with_its_own_marked() {
        let script = r#"printf 'ColorModel/Color Mode: *Gray RGB\nPageSize/Media Size: Letter Legal *ARCHD na_arch-e_36x48in w612h792 Custom.WIDTHxHEIGHT\n'"#; // WHAT: a plotter's options. WHY: the PageSize line among others.
        let dir = pretend("papers", &[("lpoptions", script)]); // WHAT: a pretend lpoptions. WHY: as above.
        let (papers, default) = Cups::on_path(&dir).papers("Plotter_T830"); // WHAT: ask for its papers. WHY: as the dialog does.
        let names: Vec<&str> = papers.iter().map(|p| p.keyword.as_str()).collect(); // WHAT: just the names. WHY: easy to compare.
        assert_eq!(
            names,
            ["Letter", "Legal", "ARCHD", "na_arch-e_36x48in", "w612h792"]
        ); // WHAT: every real size, the placeholder left out. WHY: each can be chosen.
        assert_eq!(default.as_deref(), Some("ARCHD")); // WHAT: the starred one. WHY: the printer's own.
        assert_eq!(papers[2].inches, Some((24.0, 36.0))); // WHAT: ARCH D's size. WHY: the preview shows the real paper.
        assert_eq!(papers[3].inches, Some((36.0, 48.0))); // WHAT: a PWG name's size. WHY: read from the name.
        assert_eq!(papers[4].inches, Some((8.5, 11.0))); // WHAT: a PPD size in points. WHY: the same.
        assert_eq!(label_of("na_arch-e_36x48in"), "ARCH E (36 × 48 in)"); // WHAT: a PWG name made readable. WHY: what the list shows.
    }

    #[test]
    fn printing_names_the_printer_copies_paper_and_size_and_sends_the_file_last() {
        let dir = pretend("print", &[]); // WHAT: a folder for the pretend lp. WHY: it writes what it was told there.
        let record = dir.join("told"); // WHAT: where lp writes its words. WHY: read back below.
        let script = format!("printf '%s\\n' \"$@\" > '{}'", record.display()); // WHAT: lp that writes each word it is given on a line. WHY: to see exactly what was sent.
        let dir = pretend("print", &[("lp", &script)]); // WHAT: put it on the PATH. WHY: as above.
        let file = dir.join("-odd name.pdf"); // WHAT: a file whose name starts with a dash and has a space. WHY: must still reach lp as a file.
        std::fs::write(&file, b"%PDF-1.4").unwrap(); // WHAT: something to print. WHY: lp is given a real file.
        let choice = Choice {
            copies: 2,
            collate: true,
            media: Some("ARCHD".into()),
            fit_to_paper: false,
            landscape: Some(true),
            title: "S-201".into(),
        }; // WHAT: everything chosen. WHY: each must reach lp.
        Cups::on_path(&dir)
            .print("Plotter T830", &file, &choice)
            .expect("printed"); // WHAT: print. WHY: as the dialog does.
        let told = std::fs::read_to_string(&record).unwrap(); // WHAT: what lp was given. WHY: checked word by word.
        let words: Vec<&str> = told.lines().collect(); // WHAT: one word a line. WHY: as the pretend lp wrote them.
        assert_eq!(&words[..2], ["-d", "Plotter T830"]); // WHAT: the printer chosen, space and all. WHY: not just the default.
        assert!(told.contains("-n\n2\n")); // WHAT: two copies. WHY: as chosen.
        assert!(
            told.contains("collate=true")
                && told.contains("media=ARCHD")
                && told.contains("landscape")
                && told.contains("print-scaling=none")
        ); // WHAT: collate, paper, turn and actual size. WHY: as chosen.
        assert_eq!(words[words.len() - 2], "--"); // WHAT: the options end before the file. WHY: a dashed name is not an option.
        assert_eq!(words[words.len() - 1], file.display().to_string()); // WHAT: the file last. WHY: what is printed.
    }

    #[test]
    fn a_printer_that_refuses_says_why_and_no_lp_says_cups_is_missing() {
        let dir = pretend(
            "refuse",
            &[(
                "lp",
                "echo 'lp: The printer or class does not exist.' >&2; exit 1",
            )],
        ); // WHAT: lp that refuses. WHY: a printer that has gone.
        let err = Cups::on_path(&dir)
            .print("Gone", Path::new("/nowhere.pdf"), &Choice::default())
            .unwrap_err(); // WHAT: try. WHY: it must fail plainly.
        assert!(err.contains("does not exist"), "{err}"); // WHAT: lp's reason passed on. WHY: the person can act on it.
        let empty = pretend("nolp", &[]); // WHAT: no lp at all. WHY: no CUPS.
        let err = Cups::on_path(&empty)
            .print("Any", Path::new("/nowhere.pdf"), &Choice::default())
            .unwrap_err(); // WHAT: try. WHY: as above.
        assert!(err.contains("CUPS"), "{err}"); // WHAT: says what is missing. WHY: rather than a bare error code.
    }
}
