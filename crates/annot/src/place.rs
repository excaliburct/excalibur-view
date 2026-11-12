//! Putting markups into a document, and taking them out again.

use std::collections::BTreeMap;

use pdf::{Dict, Document, Name, Object, Ref, Update};

use crate::markup::Markup;
use crate::viewport;

/// Collects markups for several pages and writes them all in one incremental
/// update, so a page is rewritten once however many markups land on it.
pub struct Placer<'a> {
    doc: &'a Document,
    pub update: Update,
    added: BTreeMap<usize, Vec<Object>>,
    /// Markups to take off a page.
    removed: BTreeMap<usize, Vec<Ref>>,
    /// Scales to set on a page as part of the same save.
    setting: BTreeMap<usize, Option<crate::measure::Measure>>,
    author: String,
    stamp: String,
    scales: BTreeMap<usize, Option<crate::measure::Measure>>,
    how: crate::appearance::Draw,
    /// The order the annotations on a page should end up in.
    ordering: BTreeMap<usize, Vec<Ref>>,
    /// Form fields added in this save, for the document's own list.
    fields: Vec<Ref>,
}

impl<'a> Placer<'a> {
    /// The order the annotations on a page should end up in.
    ///
    /// A PDF has no layer for annotations: what is in front is whatever comes
    /// later in the page's list. So "bring to front" is a reordering of that
    /// list, and this is where it lands.
    pub fn order(&mut self, page: usize, refs: Vec<Ref>) {
        self.ordering.insert(page, refs);
    }

    pub fn new(doc: &'a Document) -> Placer<'a> {
        Placer {
            doc,
            update: Update::new(doc),
            added: BTreeMap::new(),
            removed: BTreeMap::new(),
            setting: BTreeMap::new(),
            author: String::new(),
            stamp: now(),
            scales: BTreeMap::new(),
            how: crate::appearance::Draw::default(),
            ordering: BTreeMap::new(),
            fields: Vec::new(),
        }
    }

    /// How much of each markup gets drawn into the file. See
    /// [`crate::appearance::Draw`].
    pub fn drawing(mut self, how: crate::appearance::Draw) -> Placer<'a> {
        self.how = how;
        self
    }

    /// Whose markups these are. Shows up in the Markups list Author column.
    pub fn by(mut self, author: &str) -> Placer<'a> {
        self.author = author.to_string();
        self
    }

    /// Adds a markup to a page. The appearance is generated here, so what
    /// lands in the file is what every reader will draw.
    /// Sets a page's scale as part of this save. Doing it here rather than in
    /// a separate update matters: both jobs rewrite the page object, and the
    /// last one to do it would otherwise throw the other away.
    pub fn set_scale(&mut self, page: usize, measure: Option<crate::measure::Measure>) {
        self.setting.insert(page, measure.clone());
        // Anything placed after this measures against the new scale.
        self.scales.insert(page, measure);
    }

    /// Takes a markup off a page. The object stays in the file — an
    /// incremental update never deletes — but nothing points at it any more,
    /// which is what every reader goes by.
    pub fn remove(&mut self, page: usize, reference: Ref) {
        self.removed.entry(page).or_default().push(reference);
    }

    /// Replaces a markup that is already in the file, keeping its object
    /// number so anything else referring to it still finds it.
    pub fn replace(&mut self, reference: Ref, markup: &mut Markup) -> bool {
        // A stamp whose picture is only in its appearance keeps it: readers
        // fit the appearance to the new rectangle, so a moved or resized
        // signature is the same signature somewhere else.
        if markup.keeps_its_appearance() {
            markup.dict.set(Name::new("M"), Object::text(&self.stamp));
            self.update
                .replace(reference, Object::Dict(markup.dict.clone()));
            return true;
        }
        if markup.subtype() == crate::markup::Subtype::FreeText && !markup.dict.has("Rotate") {
            if let Some(turn) = self.turn_of_page_holding(reference) {
                markup.turn_with_sheet(turn);
            }
        } // WHAT: a text box edited on a turned sheet takes the sheet's turn. WHY: one written sideways by 0.6.31 or earlier comes upright the next time it is touched
        let Some(appearance) = markup.finish_how(self.how) else {
            return false;
        };
        let stream = self
            .update
            .add(Object::Stream(Box::new(appearance.stream)));
        let mut normal = Dict::new();
        normal.set(Name::new("N"), Object::Ref(stream));
        markup.dict.set(Name::new("AP"), Object::Dict(normal));
        markup.dict.set(Name::new("M"), Object::text(&self.stamp));
        self.update
            .replace(reference, Object::Dict(markup.dict.clone()));
        true
    }

    /// The turn (`/Rotate`) of the page whose annotations hold `reference`, or `None` when no page holds it.
    fn turn_of_page_holding(&self, reference: Ref) -> Option<i32> {
        (0..self.doc.page_count()).find_map(|i| {
            let page = self.doc.page(i)?; // WHAT: each page. WHY: a replace is told the markup, not its page
            self.doc
                .annotations(&page)
                .contains(&reference)
                .then(|| self.doc.page_rotation(&page)) // WHAT: that page's turn. WHY: the text box reads upright with it
        })
    }

    /// The scale in force on a page, read once and kept.
    fn scale(&mut self, page: usize, at: [f64; 2]) -> Option<crate::measure::Measure> {
        if let Some(found) = self.scales.get(&page) {
            return found.clone();
        }
        let sheet = self.doc.page(page);
        let found = sheet.and_then(|p| viewport::scale_at(self.doc, &p, at));
        self.scales.insert(page, found.clone());
        found
    }

    pub fn add(&mut self, page: usize, markup: &mut Markup) -> Option<Ref> {
        // A tool carries the scale it happened to be made at. The sheet's own
        // scale is the only one that means anything, so it replaces it — and
        // where the sheet has none, the markup goes down with no measurement
        // rather than a wrong one.
        if markup.dict.has("Measure") || markup.dict.has("MeasurementTypes") {
            let at = markup.points().first().copied().unwrap_or([0.0, 0.0]);
            match self.scale(page, at) {
                Some(measure) => {
                    markup
                        .dict
                        .set(Name::new("Measure"), Object::Dict(measure.write()));
                }
                None => {
                    markup.dict.remove("Measure");
                }
            }
        }
        if let Some(sheet) = self.doc.page(page) {
            markup.turn_with_sheet(self.doc.page_rotation(&sheet));
        } // WHAT: a new text box on a turned sheet takes the sheet's turn. WHY: its words read upright where the sheet is shown upright
        let mut appearance = markup.finish_how(self.how)?;
        // A picture inside a stamp is its own object: the appearance stream
        // names it, and one copy of the data serves however many times the
        // stamp is used.
        if !appearance.extra.is_empty() {
            let mut objects = Dict::new();
            let mut written: std::collections::HashMap<String, Ref> =
                std::collections::HashMap::new();
            for (name, mut stream) in std::mem::take(&mut appearance.extra) {
                // Transparency is its own image, and a PDF points at it by
                // object rather than by name: `/SMask /Sm0` means nothing, and
                // a reader that finds it shows the see-through part black.
                // The extras are written in order, so by the time the picture
                // is written its mask already has a number.
                for key in ["SMask", "Mask"] {
                    let wanted = stream
                        .dict
                        .get(key)
                        .and_then(|o| o.as_name())
                        .map(|n| n.as_str().to_string());
                    if let Some(wanted) = wanted {
                        match written.get(&wanted) {
                            Some(reference) => {
                                stream.dict.set(Name::new(key), Object::Ref(*reference));
                            }
                            None => {
                                stream.dict.remove(key);
                            }
                        }
                    }
                }
                let reference = self.update.add(Object::Stream(Box::new(stream)));
                written.insert(name.clone(), reference);
                objects.set(Name::new(&name), Object::Ref(reference));
            }
            let mut resources = appearance
                .stream
                .dict
                .get("Resources")
                .and_then(|o| o.as_dict())
                .cloned()
                .unwrap_or_default();
            resources.set(Name::new("XObject"), Object::Dict(objects));
            appearance
                .stream
                .dict
                .set(Name::new("Resources"), Object::Dict(resources));
        }
        let stream = self
            .update
            .add(Object::Stream(Box::new(appearance.stream)));

        let mut normal = Dict::new();
        normal.set(Name::new("N"), Object::Ref(stream));
        markup.dict.set(Name::new("AP"), Object::Dict(normal));

        if !markup.dict.has("T") && !self.author.is_empty() {
            markup.dict.set(Name::new("T"), Object::text(&self.author));
        }
        if !markup.dict.has("M") {
            markup.dict.set(Name::new("M"), Object::text(&self.stamp));
        }
        if !markup.dict.has("CreationDate") {
            markup
                .dict
                .set(Name::new("CreationDate"), Object::text(&self.stamp));
        }
        if !markup.dict.has("NM") {
            // Unique across machines, not merely within this file: six seats
            // marking up the same drawing must not produce two markups the
            // sync would mistake for one.
            markup
                .dict
                .set(Name::new("NM"), Object::text(&crate::name::fresh()));
        }
        // Bluebeam's own pointers mean nothing outside its tool chest.
        markup.dict.remove("BBObjPtr");
        markup.dict.remove("TempLabel");
        markup.dict.remove("TempBBox");
        markup.dict.remove("TempNameID");

        let reference = self.update.add(Object::Dict(markup.dict.clone()));
        self.added
            .entry(page)
            .or_default()
            .push(Object::Ref(reference));
        // A form field is two things at once: an annotation on the page, and
        // an entry in the document's own list of fields. Written as only the
        // first, it looks like a field and cannot be filled in.
        if markup
            .dict
            .get("Subtype")
            .and_then(|o| o.as_name())
            .map(|n| n.as_str() == "Widget")
            .unwrap_or(false)
        {
            self.fields.push(reference);
        }
        Some(reference)
    }

    /// Writes each page's annotation list and scale, and hands back the
    /// finished update. Every change to a page is made in one go.
    pub fn finish(mut self) -> Update {
        let pages = self.doc.pages();
        let touched: Vec<usize> = {
            let mut all: Vec<usize> = self.added.keys().copied().collect();
            all.extend(self.removed.keys().copied());
            all.extend(self.setting.keys().copied());
            all.extend(self.ordering.keys().copied());
            all.sort_unstable();
            all.dedup();
            all
        };
        for index in touched {
            let Some(page_ref) = pages.get(index).copied() else {
                continue;
            };
            let Some(mut page) = self.doc.page(index) else {
                continue;
            };

            let mut list: Vec<Object> = self
                .doc
                .at(&page, "Annots")
                .as_array()
                .map(|a| a.to_vec())
                .unwrap_or_default();
            if let Some(gone) = self.removed.remove(&index) {
                list.retain(|item| match item.as_ref() {
                    Some(r) => !gone.contains(&r),
                    None => true,
                });
            }
            if let Some(mut additions) = self.added.remove(&index) {
                list.append(&mut additions);
            }
            // What is in front of what is decided by the order of this list,
            // so an order asked for is applied here. Anything the caller did
            // not mention — a popup, a form field, a link — keeps its place at
            // the front of the list rather than being dropped.
            if let Some(wanted) = self.ordering.remove(&index) {
                let mut named: Vec<Object> = Vec::with_capacity(wanted.len());
                let mut rest: Vec<Object> = Vec::new();
                let asked: std::collections::HashSet<Ref> = wanted.iter().copied().collect();
                for item in &list {
                    match item.as_ref() {
                        Some(r) if asked.contains(&r) => {}
                        _ => rest.push(item.clone()),
                    }
                }
                let there: std::collections::HashSet<Ref> =
                    list.iter().filter_map(|o| o.as_ref()).collect();
                for reference in wanted {
                    if there.contains(&reference) {
                        named.push(Object::Ref(reference));
                    }
                }
                rest.extend(named);
                list = rest;
            }
            page.set(Name::new("Annots"), Object::Array(list));

            if let Some(measure) = self.setting.remove(&index) {
                crate::viewport::write_scale(self.doc, &mut page, measure.as_ref());
            }
            self.update.replace(page_ref, Object::Dict(page));
        }

        // The document's own list of form fields, which is what makes a field
        // fillable rather than merely visible.
        if !self.fields.is_empty() {
            self.write_fields();
        }
        self.update
    }

    /// Adds the fields written in this save to the document's `/AcroForm`.
    fn write_fields(&mut self) {
        let Some(catalog_ref) = self.doc.xref.trailer.get("Root").and_then(|o| o.as_ref())
        else {
            return;
        };
        let mut catalog = self.doc.catalog();
        let mut form = self
            .doc
            .follow(catalog.get("AcroForm").unwrap_or(&Object::Null))
            .as_dict()
            .cloned()
            .unwrap_or_default();

        let mut list: Vec<Object> = self
            .doc
            .follow(form.get("Fields").unwrap_or(&Object::Null))
            .as_array()
            .map(|a| a.to_vec())
            .unwrap_or_default();
        for reference in std::mem::take(&mut self.fields) {
            list.push(Object::Ref(reference));
        }
        form.set(Name::new("Fields"), Object::Array(list));
        // Left off on purpose. It tells a reader "my appearance streams may be
        // stale, build your own" — and a reader that believes it draws nothing
        // at all until somebody clicks in the field. Hyperview writes a real
        // appearance for every field, so there is nothing to rebuild.
        form.set(Name::new("NeedAppearances"), Object::Bool(false));
        if !form.has("DA") {
            form.set(Name::new("DA"), Object::text("/Helv 0 Tf 0 g"));
        }

        // /AcroForm may already be an object of its own; if it is, replace it
        // in place rather than making a second one the catalog no longer
        // points at.
        match catalog.get("AcroForm").and_then(|o| o.as_ref()) {
            Some(existing) => self.update.replace(existing, Object::Dict(form)),
            None => {
                let made = self.update.add(Object::Dict(form));
                catalog.set(Name::new("AcroForm"), Object::Ref(made));
                self.update.replace(catalog_ref, Object::Dict(catalog));
            }
        }
    }
}

/// Every markup on a page, as `Markup` values ready to draw or measure.
/// Links and the popup notes attached to other markups are left out, because
/// neither is something a person drew.
pub fn read_page(doc: &Document, index: usize) -> Vec<(Ref, Markup)> {
    let Some(page) = doc.page(index) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for reference in doc.annotations(&page) {
        let object = doc.get(reference);
        let Some(dict) = object.as_dict() else { continue };
        let subtype = dict
            .get("Subtype")
            .and_then(|o| o.as_name())
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        // A popup is the note attached to another markup rather than a markup
        // of its own. A link is a region, not something somebody drew. A
        // widget *is* something somebody put there — a form field — and
        // leaving it out would mean a form that vanishes when the file is
        // reopened.
        if subtype == "Popup" || subtype == "Link" {
            continue;
        }
        // Values elsewhere in the file have to be pulled in before the markup
        // can stand on its own.
        let mut dict = dict.clone();
        for key in ["Subj", "Contents", "T", "IT", "MeasurementTypes", "BSIColumnData", "Measure"] {
            if let Some(Object::Ref(r)) = dict.get(key).cloned() {
                dict.set(Name::new(key), (*doc.get(r)).clone());
            }
        }
        out.push((reference, Markup { dict, picture: None }));
    }
    out
}

/// A PDF date string for right now, in UTC.
pub fn now() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = civil(seconds);
    format!("D:{year:04}{month:02}{day:02}{hour:02}{minute:02}{second:02}Z")
}

/// Days since the epoch to a calendar date, by Howard Hinnant's method.
fn civil(seconds: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = if m <= 2 { y + 1 } else { y };
    (
        year,
        m,
        d,
        (rest / 3600) as u32,
        ((rest % 3600) / 60) as u32,
        (rest % 60) as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup::Subtype;

    fn sample() -> Vec<u8> {
        let bodies = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>",
        ];
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!(
                "trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n",
                bodies.len() + 1
            )
            .as_bytes(),
        );
        file
    }

    fn beam() -> Markup {
        let mut m = Markup::new(Subtype::Line);
        m.set_colour([1.0, 0.0, 0.0])
            .set_width(2.0)
            .set_subject("W12x26")
            .set_contents("20'-0\"");
        m.set_line([100.0, 100.0], [460.0, 100.0]);
        m
    }

    #[test]
    fn a_markup_written_in_reads_back_out_with_its_appearance() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc).by("Sam");
        let mut markup = beam();
        let reference = placer.add(0, &mut markup).unwrap();
        let saved = placer.finish().apply(&doc);

        let again = Document::from_bytes(saved);
        let back = again.get(reference);
        let dict = back.as_dict().unwrap();
        assert!(dict.get("Subj").unwrap().as_string() == Some(&b"W12x26"[..]));
        assert!(dict.get("T").unwrap().as_string() == Some(&b"Sam"[..]));
        assert!(dict.has("M") && dict.has("NM"));

        // The appearance has to be a real stream a reader can draw.
        let ap = again.at(dict, "AP");
        let normal = ap.as_dict().unwrap().get("N").unwrap().as_ref().unwrap();
        let stream = again.get(normal);
        let stream = stream.as_stream().expect("the appearance is a stream");
        assert!(stream.dict.get("Subtype").unwrap().is_name("Form"));
        let drawn = String::from_utf8_lossy(&stream.data);
        assert!(drawn.contains("100 100 m"), "{drawn}");
    }

    #[test]
    fn a_stamp_read_from_the_file_keeps_its_picture_when_it_is_moved_and_resized() {
        // Put a picture stamp in, read it back the way a reopened drawing
        // does (no picture in hand, only the appearance), then move it.
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        let picture = crate::markup::Picture {
            width: 2,
            height: 1,
            data: vec![0, 0, 255, 255, 0, 0],
            filter: "FlateDecode",
            grey: false,
            mask: None,
        };
        let mut stamp = Markup::new(Subtype::Stamp).showing(picture);
        stamp.set_box([100.0, 100.0, 200.0, 150.0]);
        let reference = placer.add(0, &mut stamp).unwrap();
        let once = Document::from_bytes(placer.finish().apply(&doc));
        let (_, mut read) = read_page(&once, 0).into_iter().find(|(r, _)| *r == reference).unwrap();
        assert!(read.picture.is_none() && read.keeps_its_appearance());
        let appearance = read.dict.get("AP").cloned();

        read.move_by(50.0, 0.0);
        read.set_box([150.0, 90.0, 350.0, 190.0]);
        assert!(read.keeps_its_appearance(), "moving it didn't drop the picture");
        let mut again = Placer::new(&once);
        assert!(again.replace(reference, &mut read));
        let twice = Document::from_bytes(again.finish().apply(&once));
        let back = twice.get(reference);
        let dict = back.as_dict().unwrap();
        assert_eq!(dict.get("AP").cloned(), appearance, "the same picture");
        assert_eq!(dict.get("Rect").and_then(|o| o.as_rect()), Some([150.0, 90.0, 350.0, 190.0]));
    }

    #[test]
    fn several_markups_on_one_page_all_arrive() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        for i in 0..5 {
            let mut m = beam();
            m.set_line([0.0, i as f64 * 10.0], [100.0, i as f64 * 10.0]);
            placer.add(0, &mut m).unwrap();
        }
        let saved = placer.finish().apply(&doc);
        let again = Document::from_bytes(saved);
        assert_eq!(read_page(&again, 0).len(), 5);
    }

    #[test]
    fn markups_land_on_the_pages_they_were_put_on() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        placer.add(0, &mut beam());
        placer.add(1, &mut beam());
        placer.add(1, &mut beam());
        let saved = placer.finish().apply(&doc);
        let again = Document::from_bytes(saved);
        assert_eq!(read_page(&again, 0).len(), 1);
        assert_eq!(read_page(&again, 1).len(), 2);
    }

    #[test]
    fn saving_twice_keeps_the_first_markups() {
        let doc = Document::from_bytes(sample());
        let mut first = Placer::new(&doc);
        first.add(0, &mut beam());
        let once = first.finish().apply(&doc);

        let doc = Document::from_bytes(once);
        let mut second = Placer::new(&doc);
        let mut other = beam();
        other.set_subject("HSS6x6x1/4");
        second.add(0, &mut other);
        let twice = second.finish().apply(&doc);

        let again = Document::from_bytes(twice);
        let found = read_page(&again, 0);
        assert_eq!(found.len(), 2);
        let subjects: Vec<String> = found.iter().map(|(_, m)| m.subject()).collect();
        assert!(subjects.contains(&"W12x26".to_string()));
        assert!(subjects.contains(&"HSS6x6x1/4".to_string()));
    }

    #[test]
    fn links_and_popups_are_not_mistaken_for_someones_markup() {
        let doc = Document::from_bytes(sample());
        let mut update = Update::new(&doc);
        let link = update.add(Object::Dict(pdf::dict! {
            "Type" => Object::name("Annot"),
            "Subtype" => Object::name("Link"),
        }));
        let mut page = doc.page(0).unwrap();
        page.set(Name::new("Annots"), Object::Array(vec![Object::Ref(link)]));
        update.replace(doc.pages()[0], Object::Dict(page));
        let saved = update.apply(&doc);

        let again = Document::from_bytes(saved);
        assert!(read_page(&again, 0).is_empty());
    }

    #[test]
    fn a_scale_and_a_markup_set_in_one_save_do_not_undo_each_other() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        placer.set_scale(0, Some(crate::measure::imperial(48.0, "1/4\" = 1'-0\"", 16)));
        placer.add(0, &mut beam());
        let saved = placer.finish().apply(&doc);

        let again = Document::from_bytes(saved);
        let page = again.page(0).unwrap();
        assert!(
            crate::viewport::has_scale(&again, &page),
            "the scale survived the markup being written"
        );
        assert_eq!(read_page(&again, 0).len(), 1, "and the markup survived the scale");
    }

    #[test]
    fn a_markup_placed_after_the_scale_is_set_measures_against_it() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        placer.set_scale(0, Some(crate::measure::imperial(48.0, "1/4\" = 1'-0\"", 16)));
        let mut m = beam();
        m.set("MeasurementTypes", Object::Int(130));
        placer.add(0, &mut m).unwrap();
        let saved = placer.finish().apply(&doc);

        let again = Document::from_bytes(saved);
        let (_, back) = read_page(&again, 0).remove(0);
        let measure = back
            .dict
            .get("Measure")
            .and_then(|o| o.as_dict())
            .and_then(crate::measure::Measure::read)
            .expect("the markup carries the sheet's scale");
        assert_eq!(measure.length(360.0), "20'-0\"");
    }

    #[test]
    fn a_markup_taken_off_a_page_stops_being_on_it() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        let first = placer.add(0, &mut beam()).unwrap();
        placer.add(0, &mut beam()).unwrap();
        let doc = Document::from_bytes(placer.finish().apply(&doc));
        assert_eq!(read_page(&doc, 0).len(), 2);

        let mut placer = Placer::new(&doc);
        placer.remove(0, first);
        let doc = Document::from_bytes(placer.finish().apply(&doc));
        assert_eq!(read_page(&doc, 0).len(), 1);
    }

    #[test]
    fn a_markup_changed_in_place_keeps_its_object_number() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        let reference = placer.add(0, &mut beam()).unwrap();
        let doc = Document::from_bytes(placer.finish().apply(&doc));

        let mut placer = Placer::new(&doc);
        let (_, mut markup) = read_page(&doc, 0).remove(0);
        markup.set_subject("HSS6x6x1/4");
        assert!(placer.replace(reference, &mut markup));
        let doc = Document::from_bytes(placer.finish().apply(&doc));

        let found = read_page(&doc, 0);
        assert_eq!(found.len(), 1, "changing one must not add another");
        assert_eq!(found[0].0, reference);
        assert_eq!(found[0].1.subject(), "HSS6x6x1/4");
    }

    #[test]
    fn clearing_the_scale_and_keeping_a_detail_both_work_in_one_save() {
        let doc = Document::from_bytes(sample());
        let mut placer = Placer::new(&doc);
        placer.set_scale(0, Some(crate::measure::imperial(48.0, "1/4\" = 1'-0\"", 16)));
        let doc = Document::from_bytes(placer.finish().apply(&doc));
        assert!(crate::viewport::has_scale(&doc, &doc.page(0).unwrap()));

        let mut placer = Placer::new(&doc);
        placer.set_scale(0, None);
        let doc = Document::from_bytes(placer.finish().apply(&doc));
        assert!(!crate::viewport::has_scale(&doc, &doc.page(0).unwrap()));
    }

    #[test]
    fn the_date_stamp_is_a_pdf_date() {
        let stamp = now();
        assert!(stamp.starts_with("D:"), "{stamp}");
        assert_eq!(stamp.len(), 17, "{stamp}");
        let year: i32 = stamp[2..6].parse().unwrap();
        assert!((2020..2100).contains(&year), "{stamp}");
    }

    #[test]
    fn a_known_instant_converts_to_the_right_calendar_date() {
        // 2026-09-17T12:00:00Z
        assert_eq!(civil(1_789_646_400), (2026, 9, 17, 12, 0, 0));
    }
}

#[cfg(test)]
mod turned_sheets {
    // WHAT: a text box placed or edited on a sheet the PDF turns for showing. WHY: it takes the sheet's turn, so its words read upright (Creede, 2026-11-11)
    use super::*;
    use crate::markup::Subtype;

    fn turned_doc() -> Document {
        let bodies = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R 4 0 R]/Count 2>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 2160 3024]/Rotate 90>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]>>",
        ]; // WHAT: a 42 x 30 sheet stored sideways and turned, and an upright one. WHY: the sheet in Creede's screenshot, and the ordinary case
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes());
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n", bodies.len() + 1).as_bytes());
        Document::from_bytes(file)
    }

    fn note() -> Markup {
        let mut m = Markup::new(Subtype::FreeText);
        m.set_box([100.0, 100.0, 200.0, 400.0]);
        m.set_contents("SEE NOTE 1");
        m
    } // WHAT: a text box. WHY: the markup that has words

    fn turn_in(saved: Vec<u8>, reference: Ref) -> Option<i64> {
        let doc = Document::from_bytes(saved);
        let back = doc.get(reference);
        back.as_dict().unwrap().get("Rotate").and_then(Object::as_i64) // WHAT: the saved box's turn. WHY: compared below
    }

    #[test]
    fn a_text_box_put_on_a_turned_sheet_is_saved_with_the_sheets_turn() {
        let doc = turned_doc();
        let mut placer = Placer::new(&doc);
        let turned = placer.add(0, &mut note()).unwrap();
        let upright = placer.add(1, &mut note()).unwrap();
        let mut cloud = Markup::new(Subtype::Square);
        cloud.set_box([10.0, 10.0, 50.0, 50.0]);
        let square = placer.add(0, &mut cloud).unwrap();
        let saved = placer.finish().apply(&doc);
        assert_eq!(turn_in(saved.clone(), turned), Some(90), "the turned sheet's box reads upright");
        assert_eq!(turn_in(saved.clone(), upright), None, "an upright sheet's box is written as before");
        assert_eq!(turn_in(saved, square), None, "a cloud is not turned");
    }

    #[test]
    fn a_sideways_box_saved_by_an_older_view_comes_upright_when_it_is_edited() {
        let doc = turned_doc();
        let mut first = Placer::new(&doc);
        let mut old = note();
        old.set("Rotate", Object::Int(0));
        let reference = first.add(0, &mut old).unwrap(); // WHAT: a box written unturned. WHY: stands in for one written by 0.6.31
        let once = Document::from_bytes(first.finish().apply(&doc));
        let mut back = read_page(&once, 0).into_iter().find(|(r, _)| *r == reference).unwrap().1; // WHAT: the box as read back. WHY: as the app reads it before an edit
        back.dict.remove("Rotate"); // WHAT: as 0.6.31 left it, with no turn at all. WHY: the case on Creede's sheet
        let mut edit = Placer::new(&once);
        assert!(edit.replace(reference, &mut back));
        assert_eq!(turn_in(edit.finish().apply(&once), reference), Some(90), "edited, it takes the sheet's turn");
    }
}
