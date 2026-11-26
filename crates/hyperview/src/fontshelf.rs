//! Whole fonts on this computer, for Edit Text.
//!
//! Nearly every drawing embeds its fonts cut down to the letters it used. A
//! letter typed that the cut-down copy lacks can't be set in it, so the
//! paragraph is set in the same family's whole font from this computer's
//! font folders instead, or in the closest font that spaces its letters the
//! same (Liberation Sans for Arial, and so on), before falling back to
//! Helvetica. A font whose licence (OS/2 fsType) forbids putting it inside a
//! file is never used.
//!
//! Only the header, the name table and the OS/2 table of each file are read
//! to list the fonts, so listing a Windows font folder of a thousand files
//! reads a few kilobytes of each. A font is read whole only when chosen.

use std::io::{Read, Seek, SeekFrom}; // WHAT: reading parts of a file. WHY: only a font's few tables are read to list it
use std::path::{Path, PathBuf}; // WHAT: file paths. WHY: fonts are files

/// The biggest font file read whole. // WHAT: a cap, 64 MB. WHY: a CJK font is 20 MB; anything bigger is not a font worth putting in a drawing
const LARGEST_FONT: u64 = 64 * 1024 * 1024; // WHAT: the cap. WHY: the same

/// The most font files listed. // WHAT: a cap. WHY: a folder walk can never run away on a strange disk
const MOST_FILES: usize = 20_000; // WHAT: twenty thousand. WHY: far past any real font folder

/// How deep into a font folder the walk goes. // WHAT: a cap. WHY: Linux keeps fonts a few folders down; nothing needs more
const DEEPEST: usize = 6; // WHAT: six folders. WHY: the same

/// One font file on this computer, as its own tables describe it.
#[derive(Clone, Debug, PartialEq)]
pub struct Face {
    pub path: PathBuf, // WHAT: where it is. WHY: read whole when chosen
    pub postscript: String, // WHAT: its PostScript name, squashed (see `squash`). WHY: PDF fonts are named by it, so it is the closest match
    pub family: String, // WHAT: its family as it names itself ("Arial"). WHY: shown in the sentence that says what was used
    pub keys: Vec<String>, // WHAT: its family names, squashed. WHY: matched against the drawing's font's family
    pub bold: bool, // WHAT: a bold face. WHY: the weight has to match
    pub italic: bool, // WHAT: an italic or oblique face. WHY: the slant has to match
    pub fs_type: u16, // WHAT: its licence's embedding flags. WHY: a font that forbids it is never put in a file
}

/// The fonts found on this computer.
#[derive(Clone, Debug, Default)]
pub struct Shelf {
    pub faces: Vec<Face>, // WHAT: every TrueType face found. WHY: chosen from
}

/// A whole font chosen to set a paragraph in.
#[derive(Clone, Debug)]
pub struct Pick {
    pub face: Face, // WHAT: which one. WHY: named in the sentence
    pub bytes: Vec<u8>, // WHAT: the font file. WHY: handed to pdfium to load into the drawing
    pub stand_in: bool, // WHAT: another family that spaces like it, not its own. WHY: the sentence says so
}

/// What `Shelf::choose` found.
#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub pick: Option<Pick>, // WHAT: the font to use, if any fits. WHY: none means Helvetica
    pub refused: Vec<String>, // WHAT: families that would have done but whose licence forbids embedding. WHY: the person is told in one sentence
}

/// Whether a font's licence lets it be put inside a file somebody may go on editing. // WHAT: the fsType check. WHY: the owner's rule: respect each font's embedding flags
///
/// Bits 0 to 3 say how it may be embedded: none set is installable, 8 is
/// editable, 4 is preview and print, 2 alone is restricted. Older fonts may
/// set several; the least restrictive one counts, as the OpenType spec says.
/// A drawing changed in Edit Text stays editable, so only installable and
/// editable fonts are used: "preview and print" needs the file to be opened
/// read-only, which no PDF can promise. Bit 9 (0x0200) means only bitmaps may
/// be embedded, and pdfium embeds outlines, so that is refused too.
pub fn may_embed(fs_type: u16) -> bool {
    let usage = fs_type & 0x000F; // WHAT: the embedding bits. WHY: the licence
    let outlines = fs_type & 0x0200 == 0; // WHAT: outlines allowed, not bitmaps only. WHY: pdfium puts the outlines in
    outlines && (usage == 0 || usage & 0x0008 != 0) // WHAT: installable or editable. WHY: the only two that allow an editable file
}

/// A name as letters and digits in lower case: "Times New Roman" and "TimesNewRoman" both become "timesnewroman". // WHAT: a name to compare. WHY: PDF writers drop spaces and change case
pub fn squash(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect() // WHAT: letters and digits, lower case. WHY: the same
}

/// The name without the six-letter tag a cut-down font is given ("ABCDEF+ArialMT" is "ArialMT"). // WHAT: the font's own name. WHY: the tag is different in every file
pub fn untagged(name: &str) -> &str {
    if crate::textedit::is_subset(name) { &name[7..] } else { name } // WHAT: after the plus sign, when there is a tag. WHY: the same
}

/// Takes a word off the end of a squashed name, if it ends with it and something is left. // WHAT: trims a suffix. WHY: "ArialMT" is "arial", "ArialBold" is "arial"
fn trim(name: &mut String, word: &str) -> bool {
    if name.len() > word.len() && name.ends_with(word) { // WHAT: ends with it, and more is left. WHY: "mt" alone is not a family
        name.truncate(name.len() - word.len()); // WHAT: taken off. WHY: the family is what is left
        return true; // WHAT: trimmed. WHY: the caller tries again
    }
    false // WHAT: not there. WHY: nothing done
}

/// A PDF font's family and anything else its name says besides weight and slant, squashed. // WHAT: reads a PDF font name. WHY: matched against this computer's fonts
///
/// "ABCDEF+Arial-BoldMT" is ("arial", ""); "TimesNewRomanPS-BoldItalicMT" is
/// ("timesnewroman", ""); "Calibri-Light" is ("calibri", "light");
/// "Arial,Bold" is ("arial", "").
pub fn family_of(pdf_font: &str) -> (String, String) {
    let name = untagged(pdf_font); // WHAT: without the tag. WHY: the tag says nothing about the font
    let (head, tail) = match name.find(['-', ',']) { // WHAT: where the style starts, if it is marked. WHY: "Arial-Bold", "Arial,Bold"
        Some(i) => (&name[..i], &name[i + 1..]), // WHAT: family and style. WHY: split there
        None => (name, ""), // WHAT: all family. WHY: no mark
    };
    let mut base = squash(head); // WHAT: the family, squashed. WHY: compared
    while trim(&mut base, "psmt") || trim(&mut base, "mt") || trim(&mut base, "ps") {} // WHAT: Monotype's and Adobe's tails off. WHY: "ArialMT" and "TimesNewRomanPS" name Arial and Times New Roman
    for word in ["bolditalic", "boldoblique", "bold", "italic", "oblique", "regular"] { // WHAT: a style run into the family. WHY: "ArialBold" with no hyphen
        if trim(&mut base, word) { // WHAT: taken off. WHY: weight and slant come from the flags
            break; // WHAT: one is enough. WHY: a name ends in one style
        }
    }
    let mut extra = squash(tail); // WHAT: the style part. WHY: "Light" or "Narrow" are part of which face
    while trim(&mut extra, "psmt") || trim(&mut extra, "mt") || trim(&mut extra, "ps") {} // WHAT: the same tails. WHY: "BoldMT"
    for word in ["bold", "italic", "oblique", "regular"] { // WHAT: weight and slant out of it. WHY: those come from the flags, which the person can change
        extra = extra.replace(word, ""); // WHAT: removed. WHY: what is left names the face
    }
    if extra == "mt" || extra == "ps" { // WHAT: only a tail was left. WHY: "Bold" + "MT"
        extra.clear(); // WHAT: nothing. WHY: the same
    }
    (base, extra) // WHAT: family and the rest. WHY: matched
}

/// Families that set their letters at the same widths, so one stands in for
/// another without the spacing changing. Squashed names, in order of preference. // WHAT: metric-compatible fonts. WHY: the owner's rule: the closest metric-compatible font before Helvetica
const STAND_INS: &[&[&str]] = &[
    &["arial", "helvetica", "liberationsans", "arimo", "nimbussans", "nimbussansl", "texgyreheros", "freesans"], // WHAT: Arial's widths. WHY: Arial was drawn to Helvetica's widths, and Liberation Sans and Arimo to Arial's
    &["arialnarrow", "helveticanarrow", "liberationsansnarrow", "nimbussansnarrow"], // WHAT: the narrow ones. WHY: the same, condensed
    &["timesnewroman", "times", "timesroman", "liberationserif", "tinos", "nimbusroman", "nimbusromanno9l", "texgyretermes", "freeserif"], // WHAT: Times's widths. WHY: Liberation Serif and Tinos match Times New Roman
    &["couriernew", "courier", "liberationmono", "cousine", "nimbusmono", "nimbusmonops", "texgyrecursor", "freemono"], // WHAT: Courier's widths. WHY: every one is 600 units a letter
    &["calibri", "carlito"], // WHAT: Calibri's. WHY: Carlito was made to its widths
    &["cambria", "caladea"], // WHAT: Cambria's. WHY: Caladea was made to its widths
    &["georgia", "gelasio"], // WHAT: Georgia's. WHY: Gelasio was made to its widths
];

/// The families that space like this one, other than itself, best first. // WHAT: a family's stand-ins. WHY: tried after its own
pub fn stand_ins(family: &str) -> Vec<&'static str> {
    STAND_INS
        .iter()
        .find(|group| group.contains(&family)) // WHAT: its group. WHY: they all share widths
        .map(|group| group.iter().copied().filter(|f| *f != family).collect()) // WHAT: the others. WHY: itself was tried already
        .unwrap_or_default() // WHAT: none. WHY: a family with no known twin goes to Helvetica
}

/// The folders fonts are kept in on this kind of computer. // WHAT: where to look. WHY: each system keeps them in its own place
pub fn system_folders() -> Vec<PathBuf> {
    let mut out = Vec::new(); // WHAT: the list. WHY: filled below
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from); // WHAT: the person's own folder. WHY: fonts installed for one person live there
    if cfg!(windows) { // WHAT: Windows. WHY: C:\Windows\Fonts, and the per-person folder Windows 10 added
        let windows = std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")); // WHAT: the Windows folder. WHY: not always on C:
        out.push(windows.join("Fonts")); // WHAT: the system's fonts. WHY: Arial, Times New Roman, Calibri live here
        if let Some(local) = std::env::var_os("LOCALAPPDATA") { // WHAT: the person's app data. WHY: fonts installed "for me only"
            out.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts")); // WHAT: their fonts. WHY: the same
        }
    } else if cfg!(any(target_os = "macos", target_os = "ios")) { // WHAT: a Mac or an iPad. WHY: the system fonts are under /System/Library/Fonts
        out.push(PathBuf::from("/System/Library/Fonts")); // WHAT: the system's fonts, Supplemental among them. WHY: Arial and Times New Roman are in Supplemental
        out.push(PathBuf::from("/Library/Fonts")); // WHAT: fonts for everybody. WHY: installed by Office and others
        if let Some(home) = &home { // WHAT: the person's folder. WHY: their own fonts
            out.push(home.join("Library").join("Fonts")); // WHAT: their fonts. WHY: Font Book puts them here
        }
    } else if cfg!(target_os = "android") { // WHAT: Android. WHY: its fonts are in /system/fonts
        out.push(PathBuf::from("/system/fonts")); // WHAT: the system's fonts. WHY: Roboto, Noto, and the metric twins some makers ship
        out.push(PathBuf::from("/product/fonts")); // WHAT: the maker's fonts. WHY: some put theirs here
    } else { // WHAT: Linux and the rest. WHY: fontconfig's standard folders
        out.push(PathBuf::from("/usr/share/fonts")); // WHAT: the distribution's fonts. WHY: Liberation and DejaVu live here
        out.push(PathBuf::from("/usr/local/share/fonts")); // WHAT: fonts installed by hand. WHY: fontconfig reads it
        if let Some(data) = std::env::var_os("XDG_DATA_HOME") { // WHAT: the person's data folder, if set. WHY: fontconfig reads its fonts folder
            out.push(PathBuf::from(data).join("fonts")); // WHAT: their fonts. WHY: the same
        }
        if let Some(home) = &home { // WHAT: the person's folder. WHY: the usual places
            out.push(home.join(".local").join("share").join("fonts")); // WHAT: their fonts. WHY: the standard place
            out.push(home.join(".fonts")); // WHAT: the old place. WHY: fontconfig still reads it
        }
    }
    out // WHAT: the list. WHY: walked by `Shelf::from_folders`
}

/// Reads part of a file. // WHAT: a few bytes at a place. WHY: only a font's tables are read
fn read_at(file: &mut std::fs::File, at: u64, len: usize) -> Option<Vec<u8>> {
    file.seek(SeekFrom::Start(at)).ok()?; // WHAT: to the place. WHY: the table is there
    let mut buffer = vec![0u8; len]; // WHAT: room. WHY: filled
    file.read_exact(&mut buffer).ok()?; // WHAT: the bytes. WHY: all of them or nothing
    Some(buffer) // WHAT: them. WHY: parsed by the caller
}

/// One font file listed: its names, weight, slant and licence, read from its tables alone. // WHAT: lists a font. WHY: a whole folder is listed without reading whole files
pub fn describe(path: &Path) -> Option<Face> {
    let mut file = std::fs::File::open(path).ok()?; // WHAT: opened. WHY: read in parts
    let head = read_at(&mut file, 0, 12)?; // WHAT: the sfnt header. WHY: says what kind of font and how many tables
    let version = u32::from_be_bytes([head[0], head[1], head[2], head[3]]); // WHAT: the kind. WHY: only TrueType outlines are used
    if version != 0x0001_0000 && version != u32::from_be_bytes(*b"true") { // WHAT: not TrueType (a CFF .otf or a .ttc collection). WHY: pdfium puts a font in as TrueType, which those aren't
        return None; // WHAT: skipped. WHY: the same
    }
    let count = u16::from_be_bytes([head[4], head[5]]) as usize; // WHAT: how many tables. WHY: the directory's size
    if count == 0 || count > 128 { // WHAT: none or absurd. WHY: a broken file
        return None; // WHAT: skipped. WHY: the same
    }
    let directory = read_at(&mut file, 12, count * 16)?; // WHAT: the table directory. WHY: where each table is
    let mut find = |tag: &[u8; 4], most: u32| -> Option<Vec<u8>> { // WHAT: one table's bytes. WHY: name and OS/2
        let entry = directory.as_chunks::<16>().0.iter().find(|e| e[..4] == tag[..])?; // WHAT: its entry. WHY: by tag
        let offset = u32::from_be_bytes([entry[8], entry[9], entry[10], entry[11]]); // WHAT: where it starts. WHY: read from there
        let length = u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]); // WHAT: how long. WHY: read that much
        if length == 0 || length > most { // WHAT: empty or too big. WHY: a broken file never costs memory
            return None; // WHAT: none. WHY: the same
        }
        read_at(&mut file, offset as u64, length as usize) // WHAT: the table. WHY: parsed
    };
    let has_glyf = directory.as_chunks::<16>().0.iter().any(|e| e[..4] == b"glyf"[..]); // WHAT: TrueType outlines. WHY: a "true" font without them is bitmaps only
    if !has_glyf { // WHAT: no outlines. WHY: nothing to draw letters with
        return None; // WHAT: skipped. WHY: the same
    }
    let name_bytes = find(b"name", 1 << 20)?; // WHAT: the name table, up to 1 MB. WHY: its names
    let os2_bytes = find(b"OS/2", 4096)?; // WHAT: the OS/2 table. WHY: weight, slant and licence
    let names = ttf_parser::name::Table::parse(&name_bytes)?; // WHAT: the names. WHY: read below
    let os2 = ttf_parser::os2::Table::parse(&os2_bytes)?; // WHAT: the OS/2 table. WHY: read below
    let fs_type = u16::from_be_bytes([os2_bytes[8], os2_bytes[9]]); // WHAT: fsType, at byte 8 in every version. WHY: the licence's embedding flags
    let named = |id: u16| -> Option<String> { // WHAT: a name by its number. WHY: family, PostScript name
        names.names.into_iter().filter(|n| n.name_id == id && n.is_unicode()).find_map(|n| n.to_string()) // WHAT: the first readable one. WHY: Windows and Unicode entries are UTF-16
    };
    let family = named(ttf_parser::name_id::FAMILY)?; // WHAT: its family ("Arial", "Calibri Light"). WHY: shown and matched
    let postscript = named(ttf_parser::name_id::POST_SCRIPT_NAME).unwrap_or_default(); // WHAT: its PostScript name ("Arial-BoldMT"). WHY: the closest match to a PDF font's name
    let mut keys = vec![squash(&family)]; // WHAT: its family, squashed. WHY: matched
    if let Some(typographic) = named(ttf_parser::name_id::TYPOGRAPHIC_FAMILY) { // WHAT: its wider family ("Calibri" for "Calibri Light"). WHY: matched too
        keys.push(squash(&typographic)); // WHAT: kept. WHY: the same
    }
    let (base, extra) = family_of(&postscript); // WHAT: its PostScript name read like a PDF font's. WHY: "ArialMT" names Arial however its family is spelled
    if !base.is_empty() { // WHAT: something there. WHY: an empty key matches nothing
        keys.push(format!("{base}{extra}")); // WHAT: the face's key. WHY: "calibrilight"
        keys.push(base); // WHAT: the family's key. WHY: "calibri"
    }
    keys.sort(); // WHAT: in order. WHY: duplicates fall together
    keys.dedup(); // WHAT: each once. WHY: tidy
    Some(Face {
        path: path.to_path_buf(), // WHAT: where it is. WHY: read when chosen
        postscript: squash(&postscript), // WHAT: squashed. WHY: compared
        family, // WHAT: as it names itself. WHY: shown
        keys, // WHAT: its family keys. WHY: matched
        bold: os2.is_bold() || os2.weight().to_number() >= 600, // WHAT: bold by its flag or its weight. WHY: some faces only set one
        italic: os2.style() != ttf_parser::Style::Normal, // WHAT: italic or oblique. WHY: either is the slanted face
        fs_type, // WHAT: its licence flags. WHY: checked before use
    })
}

/// Every font file under a folder, a few folders deep, without following links to folders. // WHAT: the walk. WHY: Linux keeps fonts in sub-folders; a link loop can't trap it
fn walk(folder: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(folder) else { return }; // WHAT: its contents. WHY: a folder that isn't there is skipped
    for entry in entries.flatten() { // WHAT: each thing in it. WHY: fonts or folders
        if out.len() >= MOST_FILES { // WHAT: enough. WHY: the cap
            return; // WHAT: stops. WHY: the same
        }
        let path = entry.path(); // WHAT: its path. WHY: checked
        let Ok(kind) = entry.file_type() else { continue }; // WHAT: what it is, without following a link. WHY: a linked folder is never walked into
        if kind.is_dir() { // WHAT: a real folder. WHY: walked into
            if depth < DEEPEST { // WHAT: not too deep. WHY: the cap
                walk(&path, depth + 1, out); // WHAT: its fonts. WHY: the same
            }
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("ttf")) { // WHAT: a TrueType file. WHY: the only kind used
            out.push(path); // WHAT: listed. WHY: described below
        }
    }
}

/// Where the whole fonts come from. // WHAT: the program's fonts or a given folder. WHY: the tests can't depend on what a build machine has installed
#[derive(Clone, Debug, Default)]
pub enum Source {
    #[default]
    System, // WHAT: this computer's font folders. WHY: what the program uses
    Folders(Vec<PathBuf>), // WHAT: only these folders. WHY: the tests' own fonts
}

impl Source {
    /// The fonts this source has, listed when first asked for. // WHAT: the shelf. WHY: nothing is listed unless a letter is missing
    pub fn shelf(&self) -> std::borrow::Cow<'static, Shelf> {
        match self {
            Source::System => std::borrow::Cow::Borrowed(Shelf::system()), // WHAT: the system's, listed once. WHY: kept for the program's life
            Source::Folders(folders) => std::borrow::Cow::Owned(Shelf::from_folders(folders)), // WHAT: these folders'. WHY: a test's fonts
        }
    }
}

impl Shelf {
    /// The fonts in these folders. // WHAT: lists them. WHY: the tests give their own folder; the program gives `system_folders`
    pub fn from_folders(folders: &[PathBuf]) -> Shelf {
        let mut files = Vec::new(); // WHAT: every font file. WHY: described
        for folder in folders { // WHAT: each folder. WHY: in order, the system's first
            walk(folder, 0, &mut files); // WHAT: its fonts. WHY: listed
        }
        Shelf { faces: files.iter().filter_map(|f| describe(f)).collect() } // WHAT: each one that is a usable TrueType font. WHY: chosen from
    }

    /// This computer's fonts, listed the first time they are wanted and kept. // WHAT: the system's shelf. WHY: listed once, and only when a letter is missing
    pub fn system() -> &'static Shelf {
        static SHELF: std::sync::OnceLock<Shelf> = std::sync::OnceLock::new(); // WHAT: kept for the program's life. WHY: fonts are rarely installed while it runs
        SHELF.get_or_init(|| Shelf::from_folders(&system_folders())) // WHAT: listed once. WHY: the same
    }

    /// The whole font to set these letters in for a PDF font of this name,
    /// weight and slant: its own family by PostScript name, then by family,
    /// then a family that spaces the same. Every letter has to be in it and its
    /// licence has to allow embedding. // WHAT: the choice. WHY: the owner's order: the same family, then a metric twin, then Helvetica
    pub fn choose(&self, pdf_font: &str, bold: bool, italic: bool, letters: &str) -> Choice {
        let exact = squash(untagged(pdf_font)); // WHAT: the PDF font's full name, squashed. WHY: the same face by PostScript name
        let (base, extra) = family_of(pdf_font); // WHAT: its family and face. WHY: matched by family when the name differs
        let face_key = format!("{base}{extra}"); // WHAT: "calibrilight". WHY: a face named as its own family
        let styled = |f: &&Face| f.bold == bold && f.italic == italic; // WHAT: the weight and slant asked for. WHY: a regular face never stands in for bold
        let mut tiers: Vec<(Vec<&Face>, bool)> = Vec::new(); // WHAT: candidates best first, and whether each tier is a stand-in. WHY: tried in order
        tiers.push((self.faces.iter().filter(styled).filter(|f| !exact.is_empty() && f.postscript == exact).collect(), false)); // WHAT: the same PostScript name. WHY: the very face
        if !extra.is_empty() { // WHAT: a face of a wider family ("Light"). WHY: its own key first
            tiers.push((self.faces.iter().filter(styled).filter(|f| f.keys.contains(&face_key)).collect(), false)); // WHAT: that face. WHY: "Calibri Light" before "Calibri"
        }
        if !base.is_empty() { // WHAT: a family. WHY: matched
            tiers.push((self.faces.iter().filter(styled).filter(|f| f.keys.contains(&base)).collect(), false)); // WHAT: the family in this weight and slant. WHY: the same family
        }
        for twin in stand_ins(&base) { // WHAT: each family that spaces the same. WHY: the closest after its own
            tiers.push((self.faces.iter().filter(styled).filter(|f| f.keys.iter().any(|k| k == twin)).collect(), true)); // WHAT: it, in this weight and slant. WHY: tried in turn
        }
        let mut choice = Choice::default(); // WHAT: nothing yet. WHY: filled below
        let mut tried: Vec<&Path> = Vec::new(); // WHAT: files already tried. WHY: a face can be in several tiers
        for (faces, stand_in) in tiers { // WHAT: each tier. WHY: best first
            for face in faces { // WHAT: each face in it. WHY: the first that fits wins
                if tried.contains(&face.path.as_path()) { // WHAT: tried already. WHY: once is enough
                    continue; // WHAT: next. WHY: the same
                }
                tried.push(&face.path); // WHAT: noted. WHY: the same
                if !may_embed(face.fs_type) { // WHAT: its licence forbids embedding. WHY: the owner's rule; never put in the file
                    if !choice.refused.contains(&face.family) { // WHAT: not said yet. WHY: one mention a family
                        choice.refused.push(face.family.clone()); // WHAT: remembered. WHY: the person is told
                    }
                    continue; // WHAT: the next. WHY: a permitted one may fit
                }
                let Some(bytes) = read_whole(&face.path) else { continue }; // WHAT: the font file. WHY: checked and loaded
                let Ok(parsed) = ttf_parser::Face::parse(&bytes, 0) else { continue }; // WHAT: read. WHY: its letters
                let has_all = letters.chars().filter(|c| !c.is_whitespace()).all(|c| parsed.glyph_index(c).is_some_and(|g| g.0 != 0)); // WHAT: every letter typed is in it. WHY: a font missing one is no better than the cut-down one
                let fs_now = parsed.raw_face().table(ttf_parser::Tag::from_bytes(b"OS/2")).and_then(|t| t.get(8..10)).map(|b| u16::from_be_bytes([b[0], b[1]])).unwrap_or(0x0002); // WHAT: the licence flags read again from the file itself, restricted if they are missing. WHY: the file may have changed since it was listed
                if !may_embed(fs_now) { // WHAT: forbidden now. WHY: the same rule
                    if !choice.refused.contains(&face.family) { // WHAT: not said yet. WHY: once
                        choice.refused.push(face.family.clone()); // WHAT: remembered. WHY: told
                    }
                    continue; // WHAT: next. WHY: the same
                }
                if has_all { // WHAT: it fits. WHY: chosen
                    choice.pick = Some(Pick { face: face.clone(), bytes, stand_in }); // WHAT: the pick. WHY: loaded into the drawing
                    return choice; // WHAT: done. WHY: the first that fits is the best
                }
            }
        }
        choice // WHAT: nothing fits. WHY: Helvetica, and the refusals said
    }
}

/// A font file read whole, if it isn't too big. // WHAT: reads a chosen font. WHY: pdfium copies it into the drawing
fn read_whole(path: &Path) -> Option<Vec<u8>> {
    let size = std::fs::metadata(path).ok()?.len(); // WHAT: its size. WHY: checked before reading
    if size > LARGEST_FONT { // WHAT: too big. WHY: the cap
        return None; // WHAT: skipped. WHY: the same
    }
    std::fs::read(path).ok() // WHAT: the file. WHY: loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(family: &str, postscript: &str, bold: bool, italic: bool, fs_type: u16) -> Face {
        let (base, extra) = family_of(postscript);
        let mut keys = vec![squash(family), format!("{base}{extra}"), base];
        keys.sort();
        keys.dedup();
        Face { path: PathBuf::from(format!("/nowhere/{postscript}.ttf")), postscript: squash(postscript), family: family.into(), keys, bold, italic, fs_type }
    }

    #[test]
    fn pdf_font_names_read_as_their_family() {
        assert_eq!(family_of("ABCDEF+ArialMT"), ("arial".into(), "".into()));
        assert_eq!(family_of("Arial-BoldMT"), ("arial".into(), "".into()));
        assert_eq!(family_of("Arial,BoldItalic"), ("arial".into(), "".into()));
        assert_eq!(family_of("TimesNewRomanPS-BoldItalicMT"), ("timesnewroman".into(), "".into()));
        assert_eq!(family_of("TimesNewRomanPSMT"), ("timesnewroman".into(), "".into()));
        assert_eq!(family_of("QWERTY+Calibri-Light"), ("calibri".into(), "light".into()));
        assert_eq!(family_of("ArialNarrow-Bold"), ("arialnarrow".into(), "".into()));
        assert_eq!(family_of("ArialBold"), ("arial".into(), "".into()));
        assert_eq!(family_of("CourierNewPSMT"), ("couriernew".into(), "".into()));
    }

    #[test]
    fn the_licence_flags_are_respected() {
        assert!(may_embed(0x0000), "installable");
        assert!(may_embed(0x0008), "editable");
        assert!(!may_embed(0x0002), "restricted");
        assert!(!may_embed(0x0004), "preview and print needs a read-only file");
        assert!(may_embed(0x000A), "restricted and editable: the least restrictive counts");
        assert!(!may_embed(0x0208), "bitmaps only");
        assert!(may_embed(0x0108), "no subsetting is fine: the whole font goes in");
    }

    #[test]
    fn arial_stands_in_for_helvetica_and_liberation_for_arial() {
        assert_eq!(stand_ins("arial")[..2], ["helvetica", "liberationsans"]);
        assert_eq!(stand_ins("timesnewroman")[..3], ["times", "timesroman", "liberationserif"]);
        assert_eq!(stand_ins("calibri"), ["carlito"]);
        assert!(stand_ins("ubuntu").is_empty());
    }

    #[test]
    fn a_folder_that_is_not_there_lists_nothing() {
        let shelf = Shelf::from_folders(&[PathBuf::from("/nowhere/at/all")]);
        assert!(shelf.faces.is_empty());
        assert!(shelf.choose("ArialMT", false, false, "A").pick.is_none());
    }

    #[test]
    fn a_restricted_face_is_refused_by_name_without_being_read() {
        let shelf = Shelf { faces: vec![face("Arial", "ArialMT", false, false, 0x0002)] };
        let choice = shelf.choose("ABCDEF+ArialMT", false, false, "A");
        assert!(choice.pick.is_none());
        assert_eq!(choice.refused, ["Arial"]);
    }

    #[test]
    fn the_weight_and_slant_have_to_match() {
        let shelf = Shelf { faces: vec![face("Arial", "Arial-BoldMT", true, false, 0x0002)] };
        assert!(shelf.choose("ArialMT", false, false, "A").refused.is_empty(), "a bold face is never tried for regular");
        assert_eq!(shelf.choose("ArialMT", true, false, "A").refused, ["Arial"]);
    }
}
