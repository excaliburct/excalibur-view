//! A whole document: the bytes, the cross reference, and the page tree.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use crate::filters;
use crate::object::{Dict, Object, Ref};
use crate::parse::Reader;
use crate::xref::{self, Slot, XRef};

pub struct Document {
    pub bytes: Vec<u8>,
    pub xref: XRef,
    /// True when the file declares an /Encrypt dictionary. Until [`unlock`]
    /// has been given the right password, every string and stream in it is
    /// ciphertext, and Hyperview says so rather than showing rubbish.
    ///
    /// [`unlock`]: Document::unlock
    pub encrypted: bool,
    /// Set once a password has opened the file. From then on every object is
    /// put back the way it was written as it is read.
    crypt: Option<crate::opening::Opened>,
    /// Which object holds `/Encrypt`. That one is never itself encrypted --
    /// it is what tells a reader how to decrypt everything else -- so it is
    /// the one object that has to be left alone.
    encrypt_object: Option<u32>,
    objects: RefCell<HashMap<u32, Rc<Object>>>,
    bundles: RefCell<HashMap<u32, Rc<Bundle>>>,
    pages: RefCell<Option<Rc<Vec<Ref>>>>,
    /// How many bytes of object streams have been decoded and kept so far.
    decoded: std::cell::Cell<usize>, // WHAT: a running total. WHY: each object stream is held to a limit, and so are all of them together.
    /// How many objects have been searched for through the whole file.
    hunts: std::cell::Cell<usize>, // WHAT: a running count. WHY: each search reads the whole file, so a file of wrong offsets must not ask for one per object.
}

/// The most a document's object streams may decode to, all together. One
/// stream is held to `filters::LARGEST_DECODED`; a file of a thousand of them
/// is held to this.
pub const LARGEST_BUNDLES: usize = 1024 * 1024 * 1024; // WHAT: 1 GiB. WHY: many times any real drawing set, short of a server's memory.

/// The most objects one document may search the whole file for. Each search
/// reads every byte; a damaged file with a hundred thousand wrong offsets
/// otherwise reads itself a hundred thousand times.
pub const MOST_HUNTS: usize = 64; // WHAT: sixty-four searches. WHY: enough for a file with a few bad offsets, which is the case this is for.

/// The most page tree nodes one document's walk visits, pages and the nodes
/// above them, found or not.
pub const MOST_NODES: usize = 400_000; // WHAT: four hundred thousand. WHY: twice the most pages ever kept; a tree naming millions of missing kids is walked no further.

/// A decoded object stream and the offsets of the objects packed into it.
struct Bundle {
    data: Vec<u8>,
    entries: Vec<(u32, usize)>,
}

impl Document {
    pub fn open(path: impl AsRef<Path>) -> std::io::Result<Document> {
        let bytes = std::fs::read(path)?;
        Ok(Document::from_bytes(bytes))
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Document {
        let xref = xref::load(&bytes);
        let encrypted = xref.trailer.has("Encrypt");
        let encrypt_object = xref.trailer.get("Encrypt").and_then(|o| o.as_ref()).map(|r| r.number);
        Document {
            bytes,
            xref,
            encrypted,
            crypt: None,
            encrypt_object,
            objects: RefCell::new(HashMap::new()),
            bundles: RefCell::new(HashMap::new()),
            pages: RefCell::new(None),
            decoded: std::cell::Cell::new(0), // WHAT: nothing decoded yet. WHY: counted as object streams are read.
            hunts: std::cell::Cell::new(0), // WHAT: nothing searched for yet. WHY: counted as objects are hunted.
        }
    }

    /// Opens a locked file with a password, or says it is the wrong one.
    ///
    /// Both the user's password and the owner's are tried. Either opens the
    /// file; the difference between them is only what the file *asks* a reader
    /// to allow, and that was never enforcement -- see [`crate::crypt`].
    ///
    /// Anything already read was read as ciphertext, so it is all thrown away
    /// and read again.
    pub fn unlock(&mut self, password: &str) -> bool {
        if !self.encrypted {
            // Nothing to open. Saying yes is right: the caller asked for a
            // readable document and has one.
            return true;
        }
        if self.crypt.is_some() {
            return true;
        }
        let Some(encrypt) = self.xref.trailer.get("Encrypt").cloned() else {
            return false;
        };
        let encrypt = match self.follow(&encrypt).as_dict() {
            Some(dict) => dict.clone(),
            None => return false,
        };
        // The first half of the trailer's /ID goes into the key for every
        // handler before revision 5. It is not encrypted and never was.
        let id = self
            .xref
            .trailer
            .get("ID")
            .and_then(|o| o.as_array())
            .and_then(|a| a.first())
            .and_then(|o| o.as_bytes())
            .map(|b| b.to_vec())
            .unwrap_or_default();
        let Some(opened) = crate::opening::open(&encrypt, &id, password) else {
            return false;
        };
        self.crypt = Some(opened);
        self.objects.borrow_mut().clear();
        self.bundles.borrow_mut().clear();
        *self.pages.borrow_mut() = None;
        true
    }

    /// The opened lock, for writing an update back into a locked file.
    ///
    /// `None` on a file that is not locked, and on one nobody has opened yet
    /// -- in which case nothing should be written to it at all, because what
    /// is being written was read as ciphertext.
    pub fn sealing(&self) -> Option<&crate::opening::Opened> {
        self.crypt.as_ref()
    }

    /// Which lock the file uses, once a password has opened it.
    ///
    /// [`crate::opening::Lock::weak`] is the part worth putting on screen: most
    /// locked drawings in circulation are held shut with something that has
    /// not been real protection since the nineties.
    pub fn lock(&self) -> Option<crate::opening::Lock> {
        self.crypt.as_ref().map(|c| c.lock)
    }

    /// True when the file is locked and no password has opened it yet. Every
    /// string and stream read in this state is ciphertext.
    pub fn still_locked(&self) -> bool {
        self.encrypted && self.crypt.is_none()
    }

    /// What the file asks a reader to allow, once opened. A request, not a
    /// lock.
    pub fn allowed(&self) -> Option<crate::crypt::Allowed> {
        self.crypt.as_ref().map(|c| c.allowed)
    }

    /// The version from the header, and from the catalog if it overrides it.
    pub fn version(&self) -> String {
        let header = self
            .bytes
            .get(5..8)
            .map(|b| String::from_utf8_lossy(b).to_string())
            .unwrap_or_else(|| "1.4".into());
        match self.catalog().get("Version").and_then(|o| o.as_name()) {
            Some(n) => n.as_str().to_string(),
            None => header,
        }
    }

    /// Fetches an object by reference. A missing or unreadable object comes
    /// back as null rather than an error, which is what every PDF reader does
    /// and what keeps one bad object from closing a drawing set.
    pub fn get(&self, reference: Ref) -> Rc<Object> {
        if let Some(found) = self.objects.borrow().get(&reference.number) {
            return found.clone();
        }
        let object = Rc::new(self.load(reference));
        self.objects
            .borrow_mut()
            .insert(reference.number, object.clone());
        object
    }

    fn load(&self, reference: Ref) -> Object {
        match self.xref.slots.get(&reference.number).copied() {
            Some(Slot::InFile { offset, .. }) => {
                let object = match Reader::at(&self.bytes, offset).indirect() {
                    // The object at that offset must be the one asked for;
                    // a file with shifted offsets otherwise returns a stranger.
                    Ok((found, object)) if found.number == reference.number => object,
                    _ => self.hunt(reference),
                };
                self.unscramble(reference, object)
            }
            // Objects packed into an object stream are *not* decrypted here.
            // The stream that holds them was decrypted as a whole when it was
            // read, so what comes out of it is already plain; running it
            // through again would turn readable text into rubbish.
            Some(Slot::InStream { stream, index }) => self.from_bundle(stream, index, reference),
            _ => Object::Null,
        }
    }

    /// One object as it was before the file was locked, when a password has
    /// opened it, and untouched otherwise.
    fn unscramble(&self, reference: Ref, mut object: Object) -> Object {
        let Some(crypt) = &self.crypt else { return object };
        if Some(reference.number) == self.encrypt_object {
            return object;
        }
        crypt.object(reference.number, reference.generation, &mut object);
        object
    }

    /// Last resort for one object whose offset is wrong: scan for its header.
    fn hunt(&self, reference: Ref) -> Object {
        if self.hunts.get() >= MOST_HUNTS {
            return Object::Null; // WHAT: no more searching. WHY: a missing object reads as null, which every caller already handles.
        }
        self.hunts.set(self.hunts.get() + 1); // WHAT: one more search. WHY: counted against the limit.
        let needle = format!("{} {} obj", reference.number, reference.generation);
        let mut at = 0usize;
        while let Some(found) = crate::parse::find(&self.bytes, needle.as_bytes(), at) {
            at = found + needle.len();
            let starts_cleanly = found == 0
                || !self.bytes[found - 1].is_ascii_digit();
            if !starts_cleanly {
                continue;
            }
            if let Ok((got, object)) = Reader::at(&self.bytes, found).indirect() {
                if got.number == reference.number {
                    return object;
                }
            }
        }
        Object::Null
    }

    fn from_bundle(&self, stream: u32, index: u32, want: Ref) -> Object {
        let bundle = match self.bundle(stream) {
            Some(b) => b,
            None => return Object::Null,
        };
        // Trust the object number over the index: some writers get it wrong.
        let entry = bundle
            .entries
            .iter()
            .find(|(number, _)| *number == want.number)
            .or_else(|| bundle.entries.get(index as usize));
        let Some((_, offset)) = entry.copied() else {
            return Object::Null;
        };
        if offset >= bundle.data.len() {
            return Object::Null;
        }
        Reader::at(&bundle.data, offset)
            .object()
            .unwrap_or(Object::Null)
    }

    fn bundle(&self, number: u32) -> Option<Rc<Bundle>> {
        if let Some(found) = self.bundles.borrow().get(&number) {
            return Some(found.clone());
        }
        // Object streams never contain object streams, so this cannot recurse.
        let Some(Slot::InFile { offset, .. }) = self.xref.slots.get(&number).copied() else {
            return None;
        };
        let (found, object) = Reader::at(&self.bytes, offset).indirect().ok()?;
        // Read from the bytes rather than through `get`, so nothing has
        // decrypted it yet -- and it has to be, before the filters see it.
        let object = self.unscramble(found, object);
        let stream = object.as_stream()?;
        let left = LARGEST_BUNDLES.saturating_sub(self.decoded.get()).min(filters::LARGEST_DECODED); // WHAT: what this stream may still decode to. WHY: its own limit, and what is left of the document's.
        let data = filters::decode_within(stream, left).ok()?; // WHAT: decoded within that. WHY: past it, the stream reads as missing, like any damaged one.
        self.decoded.set(self.decoded.get() + data.len()); // WHAT: added to the total. WHY: the next stream gets what is left.
        let count = stream.dict.get("N").and_then(|o| o.as_i64()).unwrap_or(0).max(0) as usize;
        let first = stream
            .dict
            .get("First")
            .and_then(|o| o.as_i64())
            .unwrap_or(0)
            .max(0) as usize;

        let mut header = Reader::new(&data);
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let Ok(number) = header.integer() else { break };
            let Ok(offset) = header.integer() else { break };
            entries.push((number.max(0) as u32, first + offset.max(0) as usize));
        }
        let bundle = Rc::new(Bundle { data, entries });
        self.bundles.borrow_mut().insert(number, bundle.clone());
        Some(bundle)
    }

    /// Resolves a value that might be an indirect reference.
    pub fn follow(&self, object: &Object) -> Rc<Object> {
        match object {
            Object::Ref(r) => self.get(*r),
            other => Rc::new(other.clone()),
        }
    }

    /// Looks a key up in a dictionary and resolves it in one step, which is
    /// what nearly every caller wants.
    pub fn at(&self, dict: &Dict, key: &str) -> Rc<Object> {
        match dict.get(key) {
            Some(value) => self.follow(value),
            None => Rc::new(Object::Null),
        }
    }

    pub fn catalog(&self) -> Dict {
        self.xref
            .trailer
            .get("Root")
            .map(|r| self.follow(r))
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default()
    }

    pub fn info(&self) -> Dict {
        self.xref
            .trailer
            .get("Info")
            .map(|r| self.follow(r))
            .and_then(|o| o.as_dict().cloned())
            .unwrap_or_default()
    }

    /// Every page, in order. Built once and kept.
    pub fn pages(&self) -> Rc<Vec<Ref>> {
        if let Some(found) = self.pages.borrow().as_ref() {
            return found.clone();
        }
        let mut found = Vec::new();
        let catalog = self.catalog();
        if let Some(root) = catalog.get("Pages").and_then(|o| o.as_ref()) {
            let mut seen = std::collections::HashSet::new(); // WHAT: the nodes already walked. WHY: asked once per node; a list was searched end to end each time, which made a wide tree take the square of its width.
            self.walk(root, &mut found, &mut seen, 0);
        }
        if found.is_empty() {
            // A file whose page tree is broken still has page objects in it.
            let mut numbers: Vec<u32> = self.xref.slots.keys().copied().collect();
            numbers.sort_unstable();
            for number in numbers {
                let reference = Ref::new(number, 0);
                let object = self.get(reference);
                let is_page = object
                    .as_dict()
                    .and_then(|d| d.get("Type"))
                    .map(|t| t.is_name("Page"))
                    .unwrap_or(false);
                if is_page {
                    found.push(reference);
                }
            }
        }
        let pages = Rc::new(found);
        *self.pages.borrow_mut() = Some(pages.clone());
        pages
    }

    fn walk(&self, node: Ref, out: &mut Vec<Ref>, seen: &mut std::collections::HashSet<Ref>, depth: usize) {
        if depth > 64 || out.len() > 200_000 || seen.len() >= MOST_NODES || !seen.insert(node) {
            return; // WHAT: too deep, enough pages, enough nodes, or one walked already. WHY: a loop or a tree of nothing must end, and quickly.
        }
        let object = self.get(node);
        let Some(dict) = object.as_dict() else { return };
        let kids = self.at(dict, "Kids");
        match kids.as_array() {
            Some(kids) => {
                for kid in kids {
                    if let Some(reference) = kid.as_ref() {
                        self.walk(reference, out, seen, depth + 1);
                    }
                }
            }
            // No /Kids means this is a leaf, whatever /Type claims.
            None => out.push(node),
        }
    }

    pub fn page_count(&self) -> usize {
        self.pages().len()
    }

    pub fn page(&self, index: usize) -> Option<Dict> {
        let pages = self.pages();
        let reference = pages.get(index)?;
        self.get(*reference).as_dict().cloned()
    }

    /// Reads an attribute of a page, walking up through /Parent for the ones
    /// that are allowed to be inherited.
    pub fn page_attr(&self, page: &Dict, key: &str) -> Rc<Object> {
        let mut dict = page.clone();
        for _ in 0..64 {
            let value = self.at(&dict, key);
            if !value.is_null() {
                return value;
            }
            let Some(parent) = dict.get("Parent").and_then(|o| o.as_ref()) else {
                break;
            };
            let Some(up) = self.get(parent).as_dict().cloned() else {
                break;
            };
            dict = up;
        }
        Rc::new(Object::Null)
    }

    /// The page box Hyperview measures against: the crop box when there is one,
    /// otherwise the media box, otherwise US letter.
    pub fn page_box(&self, page: &Dict) -> [f64; 4] {
        let crop = self.page_attr(page, "CropBox").as_rect();
        let media = self.page_attr(page, "MediaBox").as_rect();
        match (crop, media) {
            (Some(crop), Some(media)) => [
                crop[0].max(media[0]),
                crop[1].max(media[1]),
                crop[2].min(media[2]),
                crop[3].min(media[3]),
            ],
            (Some(only), None) | (None, Some(only)) => only,
            (None, None) => [0.0, 0.0, 612.0, 792.0],
        }
    }

    /// Page rotation, normalised to 0, 90, 180 or 270.
    pub fn page_rotation(&self, page: &Dict) -> i32 {
        let raw = self.page_attr(page, "Rotate").as_i64().unwrap_or(0);
        (((raw % 360) + 360) % 360 / 90 * 90) as i32
    }

    /// The annotations on a page, as references so they can be edited in place.
    pub fn annotations(&self, page: &Dict) -> Vec<Ref> {
        self.at(page, "Annots")
            .as_array()
            .map(|a| a.iter().filter_map(|o| o.as_ref()).collect())
            .unwrap_or_default()
    }

    /// The highest object number in use, so new objects start after it.
    pub fn highest_object(&self) -> u32 {
        let from_table = self.xref.slots.keys().copied().max().unwrap_or(0);
        let declared = self
            .xref
            .trailer
            .get("Size")
            .and_then(|o| o.as_i64())
            .unwrap_or(0)
            .max(0) as u32;
        from_table.max(declared.saturating_sub(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(objects: &[(&str, &str)], root: u32) -> Vec<u8> {
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, (_, body)) in objects.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes());
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!(
                "trailer\n<</Size {}/Root {root} 0 R>>\nstartxref\n{table}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    fn three_pages() -> Document {
        Document::from_bytes(build(
            &[
                ("catalog", "<</Type/Catalog/Pages 2 0 R>>"),
                (
                    "pages",
                    "<</Type/Pages/Kids[3 0 R 4 0 R]/Count 3/MediaBox[0 0 3024 2160]/Rotate 90>>",
                ),
                ("page", "<</Type/Page/Parent 2 0 R>>"),
                ("inner", "<</Type/Pages/Parent 2 0 R/Kids[5 0 R]/Count 1>>"),
                ("page", "<</Type/Page/Parent 4 0 R/MediaBox[0 0 612 792]/Rotate 0>>"),
            ],
            1,
        ))
    }

    #[test]
    fn a_file_of_wrong_offsets_is_searched_a_limited_number_of_times() {
        // WHAT: a page tree naming two thousand pages, every one listed at the catalog's offset, none really there. WHY: each wrong offset searched the whole file, so the file read itself once per object.
        let count = 2000; // WHAT: how many missing pages. WHY: far more than the limit, few enough that the unlimited reader still finishes.
        let kids: String = (0..count).map(|n| format!("{} 0 R ", n + 3)).collect(); // WHAT: references to them all. WHY: the walk asks for each.
        let mut file = build(&[("catalog", "<</Type/Catalog/Pages 2 0 R>>"), ("pages", &format!("<</Type/Pages/Kids[{kids}]/Count {count}>>"))], 1); // WHAT: the two real objects. WHY: the rest are missing.
        let first = file.windows(5).position(|w| w == b"xref\n").expect("the first table"); // WHAT: where the first table is. WHY: the second points back to it.
        let table = file.len(); // WHAT: a second table, added after. WHY: it lists the missing ones, all at the catalog's offset.
        file.extend_from_slice(format!("xref\n3 {count}\n").as_bytes());
        for _ in 0..count {
            file.extend_from_slice(b"0000000009 00000 n \n"); // WHAT: the catalog's offset, for every one. WHY: the object there is not the one asked for, so each is hunted.
        }
        file.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R/Prev {first}>>\nstartxref\n{table}\n%%EOF\n", count + 3).as_bytes());
        let doc = Document::from_bytes(file); // WHAT: opened. WHY: as an upload is.
        let _ = doc.page_count(); // WHAT: the page tree walked. WHY: what reads every page.
        assert!(doc.hunts.get() <= MOST_HUNTS, "searched the whole file {} times", doc.hunts.get());
    }

    #[test]
    fn a_page_tree_of_a_great_many_missing_kids_is_walked_in_a_moment() {
        // WHAT: one page tree node listing three hundred thousand pages that aren't there. WHY: each was checked against every one seen before it, so the walk took the square of the count, from two megabytes of file.
        // WHAT: the walk timed at a quarter of the count and at the whole, side by side, and the growth compared. WHY: a stopwatch alone failed under load (0.3 s alone, 15 s beside the crypt tests); a busy machine slows both sizes alike, so only the growth says how the walk scales.
        let count = 300_000u32; // WHAT: how many. WHY: far past any drawing set; quadratic, it takes minutes.
        let (mut small, mut large) = (std::time::Duration::MAX, std::time::Duration::MAX); // WHAT: the fastest walk seen at each size. WHY: the fastest of five is the one least slowed by whatever else is running
        for _ in 0..5 {
            small = small.min(walk_of_missing_kids(count / 4)); // WHAT: a quarter of the kids, timed. WHY: the baseline the whole count is compared with
            let whole = walk_of_missing_kids(count); // WHAT: all of them, timed, straight after. WHY: interleaved, so both sizes see the same load
            assert!(whole < std::time::Duration::from_secs(60), "walking the tree took {whole:?}"); // WHAT: a minute at most, however busy the machine. WHY: a hang fails here, at once, rather than three times over
            large = large.min(whole); // WHAT: the fastest whole walk. WHY: as for small
        }
        let floor = small.max(std::time::Duration::from_millis(20)); // WHAT: the quarter's time, at least 20 ms. WHY: a walk too quick to time well must not make the ratio a coin toss
        // WHAT: four times the kids takes under eleven times as long. WHY: one pass grows about four times (measured 4.5 to 5.5 on the HP); checking each kid against every one before it grows sixteen times
        assert!(large < floor * 11, "{} kids took {small:?}, {count} took {large:?}", count / 4);
    }

    /// The time to walk a page tree of `count` kids that aren't there, the file opened before the clock starts.
    fn walk_of_missing_kids(count: u32) -> std::time::Duration {
        let kids: String = (0..count).map(|n| format!("{} 0 R ", n + 10)).collect(); // WHAT: references to objects that don't exist. WHY: none costs a byte of object.
        let doc = Document::from_bytes(build(&[("catalog", "<</Type/Catalog/Pages 2 0 R>>"), ("pages", &format!("<</Type/Pages/Kids[{kids}]/Count {count}>>"))], 1)); // WHAT: opened, a fresh one each time. WHY: the page list is kept once walked, so a reused one would time nothing
        let started = std::time::Instant::now(); // WHAT: now. WHY: timed.
        assert_eq!(doc.page_count(), 0, "none of them is a page"); // WHAT: the tree walked and nothing kept. WHY: what is timed, and still checked
        started.elapsed() // WHAT: how long the walk took. WHY: the test compares two sizes
    }

    #[test]
    fn the_page_tree_flattens_in_order_through_nested_nodes() {
        let doc = three_pages();
        assert_eq!(doc.page_count(), 2);
        assert_eq!(*doc.pages(), vec![Ref::new(3, 0), Ref::new(5, 0)]);
    }

    #[test]
    fn a_page_inherits_its_box_and_rotation_from_the_node_above() {
        let doc = three_pages();
        let first = doc.page(0).unwrap();
        assert_eq!(doc.page_box(&first), [0.0, 0.0, 3024.0, 2160.0]);
        assert_eq!(doc.page_rotation(&first), 90);
        // The second page states its own, which must win.
        let second = doc.page(1).unwrap();
        assert_eq!(doc.page_box(&second), [0.0, 0.0, 612.0, 792.0]);
        assert_eq!(doc.page_rotation(&second), 0);
    }

    #[test]
    fn the_crop_box_is_clipped_to_the_media_box() {
        let doc = Document::from_bytes(build(
            &[
                ("catalog", "<</Type/Catalog/Pages 2 0 R>>"),
                ("pages", "<</Type/Pages/Kids[3 0 R]/Count 1>>"),
                (
                    "page",
                    "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/CropBox[-50 10 1000 700]>>",
                ),
            ],
            1,
        ));
        let page = doc.page(0).unwrap();
        assert_eq!(doc.page_box(&page), [0.0, 10.0, 612.0, 700.0]);
    }

    #[test]
    fn rotation_is_normalised_however_it_was_written() {
        let doc = Document::from_bytes(build(
            &[
                ("catalog", "<</Type/Catalog/Pages 2 0 R>>"),
                ("pages", "<</Type/Pages/Kids[3 0 R]/Count 1>>"),
                ("page", "<</Type/Page/Parent 2 0 R/Rotate -90>>"),
            ],
            1,
        ));
        assert_eq!(doc.page_rotation(&doc.page(0).unwrap()), 270);
    }

    #[test]
    fn an_object_whose_offset_points_at_the_wrong_object_is_hunted_down() {
        let mut file = build(
            &[
                ("catalog", "<</Type/Catalog/Pages 2 0 R>>"),
                ("pages", "<</Type/Pages/Kids[3 0 R]/Count 1>>"),
                ("page", "<</Type/Page/Parent 2 0 R/MediaBox[0 0 99 99]>>"),
            ],
            1,
        );
        // Point object 3 at object 1's offset.
        let text = String::from_utf8_lossy(&file).to_string();
        let lines: Vec<&str> = text.split('\n').collect();
        let first = lines.iter().find(|l| l.ends_with("00000 n ")).unwrap().to_string();
        let broken = text.replacen(
            &lines.iter().filter(|l| l.ends_with("00000 n ")).nth(2).unwrap().to_string(),
            &first,
            1,
        );
        file = broken.into_bytes();
        let doc = Document::from_bytes(file);
        let page = doc.page(0).unwrap();
        assert_eq!(
            doc.page_box(&page),
            [0.0, 0.0, 99.0, 99.0],
            "the right object was found despite the wrong offset"
        );
    }

    #[test]
    fn a_page_tree_that_loops_does_not_hang() {
        let doc = Document::from_bytes(build(
            &[
                ("catalog", "<</Type/Catalog/Pages 2 0 R>>"),
                ("pages", "<</Type/Pages/Kids[2 0 R 3 0 R]/Count 1>>"),
                ("page", "<</Type/Page/Parent 2 0 R>>"),
            ],
            1,
        ));
        assert_eq!(doc.page_count(), 1);
    }
}
