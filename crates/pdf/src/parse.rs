//! Reading PDF syntax.
//!
//! Real drawing sets are not tidy. Sheets come out of a dozen different CAD
//! programs, get stapled together by Bluebeam, and arrive with stream lengths
//! that lie and cross reference tables that are off by a few bytes. Everything
//! here is written to keep going where the specification says it may stop.

use crate::object::{Dict, Name, Object, Ref, Stream, StringKind};

#[derive(Debug, Clone)]
pub struct Error {
    pub at: usize,
    pub why: String,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} at byte {}", self.why, self.at)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = std::result::Result<T, Error>;

pub fn is_space(b: u8) -> bool {
    matches!(b, b'\0' | b'\t' | b'\n' | b'\x0c' | b'\r' | b' ')
}

pub fn is_delimiter(b: u8) -> bool {
    matches!(
        b,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

pub fn is_regular(b: u8) -> bool {
    !is_space(b) && !is_delimiter(b)
}

pub struct Reader<'a> {
    pub bytes: &'a [u8],
    pub at: usize,
    /// How many arrays and dictionaries the object being read is inside.
    depth: usize, // WHAT: the nesting so far. WHY: reading is recursive, and a file must not choose how deep the stack goes.
    too_deep: bool, // WHAT: whether anything was refused for nesting too deep. WHY: a caller keeping the bytes as they came can refuse them whole.
}

/// The deepest an array or dictionary may sit inside others. Real files
/// nest a handful deep; a file nested a hundred thousand deep would run the
/// program out of stack and close it, taking the office server with it.
pub const DEEPEST: usize = 256; // WHAT: 256 levels. WHY: far past any real file, far short of the stack.

impl<'a> Reader<'a> {
    pub fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader {
            bytes,
            at: 0,
            depth: 0,
            too_deep: false,
        }
    }

    pub fn at(bytes: &'a [u8], at: usize) -> Reader<'a> {
        Reader {
            bytes,
            at,
            depth: 0,
            too_deep: false,
        }
    }

    /// Whether anything this reader read was nested deeper than [`DEEPEST`]
    /// and left out. What came back is then not what the bytes say.
    #[must_use] // WHAT: the answer must be looked at. WHY: asking and ignoring it would keep a damaged reading
    pub fn went_too_deep(&self) -> bool {
        self.too_deep // WHAT: the flag. WHY: read by whoever keeps the bytes.
    }

    fn fail<T>(&self, why: impl Into<String>) -> Result<T> {
        Err(Error {
            at: self.at,
            why: why.into(),
        })
    }

    pub fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn peek_at(&self, ahead: usize) -> Option<u8> {
        self.bytes.get(self.at + ahead).copied()
    }

    pub fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    /// Skips whitespace and `% comments`, which may appear anywhere a token may.
    pub fn skip_space(&mut self) {
        while let Some(b) = self.peek() {
            if is_space(b) {
                self.at += 1;
            } else if b == b'%' {
                while let Some(c) = self.peek() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.at += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Consumes `word` if it is the next token. Returns whether it did.
    pub fn eat(&mut self, word: &str) -> bool {
        self.skip_space();
        let w = word.as_bytes();
        if self.bytes[self.at..].starts_with(w) {
            let after = self.at + w.len();
            let boundary = self
                .bytes
                .get(after)
                .map(|b| !is_regular(*b))
                .unwrap_or(true);
            // A keyword must not run into the next token: `endstream` is not `end`.
            if !w.last().map(|b| is_regular(*b)).unwrap_or(false) || boundary {
                self.at = after;
                return true;
            }
        }
        false
    }

    /// Reads a bare keyword such as `obj`, `stream` or `xref`.
    pub fn keyword(&mut self) -> &'a [u8] {
        self.skip_space();
        let start = self.at;
        while self.peek().map(is_regular).unwrap_or(false) {
            self.at += 1;
        }
        &self.bytes[start..self.at]
    }

    pub fn integer(&mut self) -> Result<i64> {
        self.skip_space();
        let start = self.at;
        if matches!(self.peek(), Some(b'+') | Some(b'-')) {
            self.at += 1;
        }
        while self.peek().map(|b| b.is_ascii_digit()).unwrap_or(false) {
            self.at += 1;
        }
        if self.at == start {
            return self.fail("expected a number");
        }
        std::str::from_utf8(&self.bytes[start..self.at])
            .ok()
            .and_then(|s| s.parse::<i64>().ok())
            .ok_or_else(|| Error {
                at: start,
                why: "malformed integer".into(),
            })
    }

    fn number(&mut self) -> Result<Object> {
        self.skip_space();
        let start = self.at;
        let mut real = false;
        if matches!(self.peek(), Some(b'+') | Some(b'-')) {
            self.at += 1;
        }
        while let Some(b) = self.peek() {
            match b {
                b'0'..=b'9' => self.at += 1,
                b'.' => {
                    real = true;
                    self.at += 1;
                }
                // Some writers emit a second sign mid-number, as in `1-2`.
                b'+' | b'-' => self.at += 1,
                _ => break,
            }
        }
        if self.at == start {
            return self.fail("expected a number");
        }
        let text = String::from_utf8_lossy(&self.bytes[start..self.at]).to_string();
        if real {
            // `.5`, `4.`, `-.002` are all legal PDF reals.
            let cleaned = text.trim_end_matches('.');
            Ok(Object::Real(cleaned.parse::<f64>().unwrap_or(0.0)))
        } else {
            Ok(Object::Int(text.parse::<i64>().unwrap_or(0)))
        }
    }

    pub fn name(&mut self) -> Result<Name> {
        self.skip_space();
        if self.peek() != Some(b'/') {
            return self.fail("expected a name");
        }
        self.at += 1;
        let mut out = Vec::new();
        while let Some(b) = self.peek() {
            if !is_regular(b) {
                break;
            }
            if b == b'#' {
                let hi = self.peek_at(1).and_then(hex_value);
                let lo = self.peek_at(2).and_then(hex_value);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push(hi * 16 + lo);
                    self.at += 3;
                    continue;
                }
            }
            out.push(b);
            self.at += 1;
        }
        Ok(Name(out))
    }

    fn literal_string(&mut self) -> Result<Object> {
        self.at += 1; // the opening parenthesis
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(b) = self.peek() {
            self.at += 1;
            match b {
                b'\\' => {
                    let Some(e) = self.peek() else { break };
                    self.at += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        // A backslash before a newline is a line continuation.
                        b'\r' => {
                            if self.peek() == Some(b'\n') {
                                self.at += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut value = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek() {
                                    Some(d @ b'0'..=b'7') => {
                                        value = value * 8 + (d - b'0') as u32;
                                        self.at += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push(value as u8);
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(b'(');
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Object::String(out, StringKind::Literal));
                    }
                    out.push(b')');
                }
                other => out.push(other),
            }
        }
        Ok(Object::String(out, StringKind::Literal))
    }

    fn hex_string(&mut self) -> Result<Object> {
        self.at += 1; // the opening angle bracket
        let mut out = Vec::new();
        let mut half: Option<u8> = None;
        while let Some(b) = self.peek() {
            self.at += 1;
            if b == b'>' {
                break;
            }
            let Some(v) = hex_value(b) else { continue };
            match half.take() {
                Some(hi) => out.push(hi * 16 + v),
                None => half = Some(v),
            }
        }
        // An odd number of digits is padded with a trailing zero.
        if let Some(hi) = half {
            out.push(hi * 16);
        }
        Ok(Object::String(out, StringKind::Hex))
    }

    fn array(&mut self) -> Result<Object> {
        if self.depth >= DEEPEST {
            self.at += 1; // WHAT: past the bracket. WHY: whoever called this steps on rather than trying the same byte again.
            self.too_deep = true; // WHAT: noted. WHY: what comes back is not all the bytes say.
            return self.fail("nested too deep"); // WHAT: refused. WHY: deeper would be the file choosing how deep the stack goes.
        }
        self.depth += 1; // WHAT: one level deeper. WHY: counted while this array is read.
        let read = Ok(self.array_inside()); // WHAT: the array itself, which always reads. WHY: read with the level counted.
        self.depth -= 1; // WHAT: back out. WHY: the next object starts where this one did.
        read
    }

    fn array_inside(&mut self) -> Object {
        self.at += 1; // '['
        let mut items = Vec::new();
        loop {
            self.skip_space();
            match self.peek() {
                None => break,
                Some(b']') => {
                    self.at += 1;
                    break;
                }
                _ => {}
            }
            let before = self.at;
            match self.object() {
                Ok(o) => items.push(o),
                Err(_) => {
                    // Skip whatever could not be read rather than losing the
                    // rest of the array.
                    self.at = before + 1;
                }
            }
            if self.at <= before {
                self.at = before + 1;
            }
        }
        Object::Array(items) // WHAT: what was read, unreadable items skipped. WHY: an array never fails as a whole
    }

    pub fn dictionary(&mut self) -> Result<Dict> {
        if self.depth >= DEEPEST {
            self.at += 2; // WHAT: past the `<<`. WHY: as for an array.
            self.too_deep = true; // WHAT: noted. WHY: as for an array.
            return self.fail("nested too deep"); // WHAT: refused. WHY: as for an array.
        }
        self.depth += 1; // WHAT: one level deeper. WHY: as for an array.
        let read = self.dictionary_inside(); // WHAT: the dictionary itself. WHY: read with the level counted.
        self.depth -= 1; // WHAT: back out. WHY: as for an array.
        read
    }

    fn dictionary_inside(&mut self) -> Result<Dict> {
        self.at += 2; // '<<'
        let mut d = Dict::new();
        loop {
            self.skip_space();
            match self.peek() {
                None => break,
                Some(b'>') => {
                    self.at += 1;
                    if self.peek() == Some(b'>') {
                        self.at += 1;
                    }
                    break;
                }
                Some(b'/') => {}
                _ => {
                    // Junk where a key should be: step over it.
                    self.at += 1;
                    continue;
                }
            }
            let key = self.name()?;
            let before = self.at;
            let value = match self.object() {
                Ok(v) => v,
                Err(_) => {
                    self.at = before;
                    Object::Null
                }
            };
            if self.at <= before {
                self.at = before + 1;
            }
            d.set(key, value);
        }
        Ok(d)
    }

    /// Reads one object. Indirect references are recognised here, by looking
    /// ahead for the `int int R` shape and rewinding when it is not there.
    pub fn object(&mut self) -> Result<Object> {
        self.skip_space();
        let Some(b) = self.peek() else {
            return self.fail("end of file where an object was expected");
        };
        match b {
            b'/' => Ok(Object::Name(self.name()?)),
            b'(' => self.literal_string(),
            b'[' => self.array(),
            b'<' => {
                if self.peek_at(1) == Some(b'<') {
                    let dict = self.dictionary()?;
                    self.maybe_stream(dict)
                } else {
                    self.hex_string()
                }
            }
            b'0'..=b'9' | b'+' | b'-' | b'.' => {
                let start = self.at;
                let first = self.number()?;
                if let Object::Int(number) = first {
                    if number >= 0 {
                        let save = self.at;
                        if let Ok(generation) = self.integer() {
                            if (0..=65535).contains(&generation) && self.eat("R") {
                                return Ok(Object::Ref(Ref::new(number as u32, generation as u16)));
                            }
                        }
                        self.at = save;
                    }
                }
                let _ = start;
                Ok(first)
            }
            b']' | b'>' | b')' | b'}' => self.fail("unexpected closing delimiter"),
            _ => {
                let word = self.keyword();
                match word {
                    b"true" => Ok(Object::Bool(true)),
                    b"false" => Ok(Object::Bool(false)),
                    b"null" => Ok(Object::Null),
                    b"" => {
                        self.at += 1;
                        self.fail("unreadable byte")
                    }
                    // `endobj`, `endstream` and friends end an object rather
                    // than being one; the caller notices the position did not move.
                    _ => Ok(Object::Null),
                }
            }
        }
    }

    /// After a dictionary, turns it into a stream if `stream` follows.
    fn maybe_stream(&mut self, dict: Dict) -> Result<Object> {
        let save = self.at;
        self.skip_space();
        if !self.bytes[self.at..].starts_with(b"stream") {
            self.at = save;
            return Ok(Object::Dict(dict));
        }
        self.at += b"stream".len();
        // The keyword is followed by CRLF or LF, never by CR alone.
        if self.peek() == Some(b'\r') {
            self.at += 1;
        }
        if self.peek() == Some(b'\n') {
            self.at += 1;
        }
        let start = self.at;

        let declared = dict.get("Length").and_then(|o| o.as_i64());
        let end = match declared {
            Some(n) if n >= 0 && start + n as usize <= self.bytes.len() => {
                let candidate = start + n as usize;
                if looks_like_endstream(self.bytes, candidate) {
                    candidate
                } else {
                    find_endstream(self.bytes, start)
                }
            }
            // /Length is an indirect reference, or a lie. Both are common.
            _ => find_endstream(self.bytes, start),
        };
        let data = self.bytes[start..end.min(self.bytes.len())].to_vec();
        self.at = end;
        self.eat("endstream");
        Ok(Object::Stream(Box::new(Stream { dict, data })))
    }

    /// Reads `12 0 obj … endobj` starting at the current position.
    pub fn indirect(&mut self) -> Result<(Ref, Object)> {
        self.skip_space();
        let number = self.integer()?;
        let generation = self.integer()?;
        if !self.eat("obj") {
            return self.fail("expected 'obj'");
        }
        let value = self.object()?;
        self.skip_space();
        self.eat("endobj");
        Ok((
            Ref::new(number.max(0) as u32, generation.max(0) as u16),
            value,
        ))
    }
}

fn hex_value(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// True when, allowing for whitespace, `endstream` sits at `at`.
fn looks_like_endstream(bytes: &[u8], at: usize) -> bool {
    let mut i = at;
    let limit = (at + 4).min(bytes.len());
    while i < limit && is_space(bytes[i]) {
        i += 1;
    }
    bytes[i..].starts_with(b"endstream")
}

/// Finds the end of stream data when the declared length cannot be trusted.
/// The last newline before `endstream` belongs to the keyword, not the data.
fn find_endstream(bytes: &[u8], from: usize) -> usize {
    let Some(found) = find(bytes, b"endstream", from) else {
        return bytes.len();
    };
    let mut end = found;
    if end > from && bytes[end - 1] == b'\n' {
        end -= 1;
    }
    if end > from && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    end
}

pub fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || from >= haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

pub fn rfind(haystack: &[u8], needle: &[u8], before: usize) -> Option<usize> {
    let end = before.min(haystack.len());
    if needle.is_empty() || needle.len() > end {
        return None;
    }
    haystack[..end]
        .windows(needle.len())
        .rposition(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> Object {
        Reader::new(text.as_bytes()).object().unwrap()
    }

    #[test]
    fn the_eight_object_types_read_back_as_themselves() {
        assert_eq!(read("true"), Object::Bool(true));
        assert_eq!(read("null"), Object::Null);
        assert_eq!(read("42"), Object::Int(42));
        assert_eq!(read("-3.25"), Object::Real(-3.25));
        assert_eq!(read("/Type"), Object::name("Type"));
        assert_eq!(read("12 0 R"), Object::Ref(Ref::new(12, 0)));
        assert_eq!(read("[1 2]").as_array().unwrap().len(), 2);
        assert!(read("<</A 1>>").as_dict().unwrap().has("A"));
    }

    #[test]
    fn a_number_that_is_not_a_reference_rewinds_cleanly() {
        let mut r = Reader::new(b"12 0 /Next");
        assert_eq!(r.object().unwrap(), Object::Int(12));
        assert_eq!(r.object().unwrap(), Object::Int(0));
        assert_eq!(r.object().unwrap(), Object::name("Next"));
    }

    #[test]
    fn odd_reals_that_cad_writes_are_still_numbers() {
        assert_eq!(read(".5"), Object::Real(0.5));
        assert_eq!(read("4."), Object::Real(4.0));
        assert_eq!(read("-.002"), Object::Real(-0.002));
    }

    #[test]
    fn an_object_nested_past_any_real_file_is_refused_rather_than_read_to_the_bottom() {
        // WHAT: arrays and dictionaries a hundred thousand deep, read on a thread with a small stack. WHY: unlimited, reading them ran out of stack and closed the whole program; on the office server, one markup or drawing did that to everybody.
        let deep_arrays = format!("{}1{}", "[".repeat(100_000), "]".repeat(100_000));
        let deep_dicts = format!("{}1{}", "<</A ".repeat(100_000), ">>".repeat(100_000));
        let read = std::thread::Builder::new()
            .stack_size(512 * 1024) // WHAT: half a megabyte of stack. WHY: less than a server's threads have, so an unlimited reader fails here first.
            .spawn(move || {
                let arrays = Reader::new(deep_arrays.as_bytes()).object(); // WHAT: read. WHY: must come back, whatever it says.
                let dicts = Reader::new(deep_dicts.as_bytes()).object(); // WHAT: read. WHY: the same.
                (arrays.is_ok(), dicts.is_ok()) // WHAT: whether each came back at all. WHY: what matters is that the thread lived.
            })
            .unwrap()
            .join(); // WHAT: the reader's thread finished. WHY: a stack overflow would end the test process instead.
        assert!(
            read.is_ok(),
            "reading a deeply nested object did not come back"
        );
        let shallow = Reader::new(b"[[[1 2] <</K [3]>>]]").object().unwrap(); // WHAT: ordinary nesting. WHY: still read whole.
        assert_eq!(shallow.as_array().map(|a| a.len()), Some(1));
    }

    #[test]
    fn string_escapes_and_nesting_survive() {
        assert_eq!(read(r"(a\(b\)c)").as_string().unwrap(), b"a(b)c");
        assert_eq!(
            read("(outer (inner) done)").as_string().unwrap(),
            b"outer (inner) done"
        );
        assert_eq!(read(r"(\101\102)").as_string().unwrap(), b"AB");
        assert_eq!(
            read("(line\\\ncontinued)").as_string().unwrap(),
            b"linecontinued"
        );
    }

    #[test]
    fn hex_strings_pad_an_odd_final_digit() {
        assert_eq!(read("<48656C6C6F>").as_string().unwrap(), b"Hello");
        assert_eq!(read("<4A>").as_string().unwrap(), b"J");
        assert_eq!(read("<4>").as_string().unwrap(), &[0x40]);
    }

    #[test]
    fn names_decode_their_escapes() {
        assert_eq!(read("/A#20B"), Object::name("A B"));
        assert_eq!(read("/Adobe#20Green"), Object::name("Adobe Green"));
    }

    #[test]
    fn a_stream_whose_length_lies_is_still_read_to_the_right_place() {
        let file = b"<</Length 99>>\nstream\nHELLO\nendstream";
        let s = Reader::new(file).object().unwrap();
        assert_eq!(s.as_stream().unwrap().data, b"HELLO");
    }

    #[test]
    fn a_stream_whose_length_is_indirect_is_found_by_searching() {
        let file = b"<</Length 7 0 R>>\nstream\nABCDEF\nendstream";
        let s = Reader::new(file).object().unwrap();
        assert_eq!(s.as_stream().unwrap().data, b"ABCDEF");
    }

    #[test]
    fn binary_stream_data_is_taken_verbatim_when_the_length_is_right() {
        let mut file = b"<</Length 5>>\nstream\n".to_vec();
        file.extend_from_slice(&[0, 1, b'e', b'n', b'd']);
        file.extend_from_slice(b"\nendstream");
        let s = Reader::new(&file).object().unwrap();
        assert_eq!(s.as_stream().unwrap().data, vec![0, 1, b'e', b'n', b'd']);
    }

    #[test]
    fn an_indirect_object_gives_back_its_reference() {
        let (r, o) = Reader::new(b"7 0 obj\n<</A 1>>\nendobj")
            .indirect()
            .unwrap();
        assert_eq!(r, Ref::new(7, 0));
        assert!(o.as_dict().unwrap().has("A"));
    }

    #[test]
    fn a_keyword_is_not_matched_inside_a_longer_word() {
        let mut r = Reader::new(b"endstream");
        assert!(!r.eat("end"));
        assert!(r.eat("endstream"));
    }

    #[test]
    fn comments_are_skipped_wherever_they_appear() {
        assert_eq!(read("% a note\n  42"), Object::Int(42));
        let d = read("<< % why\n /A 1 >>");
        assert!(d.as_dict().unwrap().has("A"));
    }

    #[test]
    fn a_broken_array_does_not_swallow_the_rest_of_the_file() {
        let o = read("[1 ) 2]");
        assert_eq!(o.as_array().unwrap().len(), 2);
    }
}
