//! Page scale.
//!
//! A scale belongs to the sheet, not to the tool. A tool saved in the Tool
//! Chest carries whatever scale was in force the day it was made, and using
//! that would quietly report the same 60 foot run as 25 feet, 40 feet or 80
//! feet depending on which shape was picked. So the scale is read from the page
//! and applied on placement, every time.
//!
//! PDF keeps it in `/VP`, an array of viewports each with a `/BBox` and a
//! `/Measure`. That allows what a drawing set actually does: a sheet at quarter
//! inch with a detail in the corner at one inch. Hyperview honours that by asking
//! for the scale **at a point** rather than for the sheet as a whole.

use pdf::{dict, Dict, Document, Name, Object, Update};

use crate::measure::Measure;

/// One scaled region of a sheet.
pub struct Region {
    pub area: [f64; 4],
    pub name: String,
    pub measure: Measure,
}

impl Region {
    fn holds(&self, at: [f64; 2]) -> bool {
        at[0] >= self.area[0] && at[0] <= self.area[2] && at[1] >= self.area[1] && at[1] <= self.area[3]
    }

    fn size(&self) -> f64 {
        (self.area[2] - self.area[0]).max(0.0) * (self.area[3] - self.area[1]).max(0.0)
    }
}

/// The most `/VP` entries read from a page. A real sheet has a handful of
/// viewports; a file listing thousands is read no further than this.
pub const MOST_REGIONS: usize = 64; // WHAT: sixty-four viewports a sheet. WHY: security review M1: a hostile list of a million entries can't cost a million reads

/// Every scaled region on a page, in the order the file lists them, up to
/// [`MOST_REGIONS`].
pub fn regions(doc: &Document, page: &Dict) -> Vec<Region> {
    let list = doc.at(page, "VP");
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    let fallback = doc.page_box(page);
    items
        .iter()
        .take(MOST_REGIONS) // WHAT: the first sixty-four. WHY: security review M1
        .filter_map(|item| {
            let object = doc.follow(item);
            let dict = object.as_dict()?;
            let measure = Measure::read_in(doc, doc.at(dict, "Measure").as_dict()?)?; // WHAT: the measure, its number formats read through any references, bounded. WHY: some exporters keep each format as its own object, and a detail's scale was lost
            Some(Region {
                area: doc.at(dict, "BBox").as_rect().unwrap_or(fallback),
                name: doc.at(dict, "Name").as_text().unwrap_or_default(),
                measure,
            })
        })
        .collect()
}

/// Whether `area` covers the whole of `sheet`, give or take a point.
fn covers(area: [f64; 4], sheet: [f64; 4]) -> bool {
    area[0] <= sheet[0] + 1.0 && area[1] <= sheet[1] + 1.0 && area[2] >= sheet[2] - 1.0 && area[3] >= sheet[3] - 1.0 // WHAT: every edge at or past the sheet's. WHY: the region a scale set here writes
}

/// The region that stands for the whole sheet: the one covering the sheet
/// (what [`write_scale`] writes), else the first listed, exactly as before.
fn sheet_wide(regions: &[Region], sheet: [f64; 4]) -> Option<&Region> {
    regions.iter().find(|r| covers(r.area, sheet)).or_else(|| regions.first()) // WHAT: the covering region, else the first. WHY: a scale just set wins; a drawing nobody re-scales keeps exactly the quantities it had (security review M2)
}

/// The scale that applies at a point. The smallest region containing the point
/// wins, so a detail drawn at a larger scale inside a sheet measures correctly.
pub fn scale_at(doc: &Document, page: &Dict, at: [f64; 2]) -> Option<Measure> {
    let regions = regions(doc, page);
    regions
        .iter()
        .filter(|r| r.holds(at))
        .min_by(|a, b| {
            a.size()
                .partial_cmp(&b.size())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .or_else(|| sheet_wide(&regions, doc.page_box(page))) // WHAT: outside every region, the sheet's own scale. WHY: the same rule as scale_of
        .map(|r| r.measure.clone())
}

/// The scale for the sheet as a whole: the region covering the sheet, which
/// is the one a scale set here writes, else the first the file lists.
pub fn scale_of(doc: &Document, page: &Dict) -> Option<Measure> {
    let regions = regions(doc, page); // WHAT: every readable region. WHY: chosen from below
    sheet_wide(&regions, doc.page_box(page)).map(|r| r.measure.clone()) // WHAT: the sheet-wide one. WHY: the scale shown and totalled for the sheet
}

/// Whether a page has any scale at all. A page without one has its
/// measurements left out of totals rather than counted as zero.
pub fn has_scale(doc: &Document, page: &Dict) -> bool {
    !regions(doc, page).is_empty()
}

/// Sets or clears the sheet-wide scale on a page dictionary that the caller is
/// already holding, so one save can change the scale and the markups together.
/// Only a region covering the sheet is replaced; every other entry is kept as
/// it was, whatever its measure. The sheet's own region goes first, so every
/// reader finds it, an older one reading the first region included.
pub fn write_scale(doc: &Document, page: &mut Dict, measure: Option<&Measure>) {
    let area = doc.page_box(page);
    let existing = doc.at(page, "VP");
    let mut list: Vec<Object> = Vec::new();
    if let Some(measure) = measure {
        list.push(Object::Dict(dict! {
            "Type" => Object::name("Viewport"),
            "BBox" => Object::Array(area.iter().map(|v| Object::real(*v)).collect()),
            "Name" => Object::text("Sheet"),
            "Measure" => Object::Dict(measure.write()),
        })); // WHAT: the sheet's own region, first. WHY: inside the regions read, and what older builds read as the sheet's scale
    }
    if let Some(items) = existing.as_array() {
        for (index, item) in items.iter().enumerate() {
            if index >= MOST_REGIONS {
                list.push(item.clone()); // WHAT: kept unexamined. WHY: never followed past the cap (security review M1), never lost
                continue;
            }
            let object = doc.follow(item);
            let its_area = object
                .as_dict()
                .map(|d| doc.at(d, "BBox").as_rect().unwrap_or(area))
                .unwrap_or(area);
            // A region smaller than the sheet is a detail with its own scale.
            if !covers(its_area, area) {
                list.push(item.clone()); // WHAT: kept, ruler or not (a /GEO measure too). WHY: only the sheet's own scale is replaced
            }
        }
    }
    if list.is_empty() {
        page.remove("VP");
    } else {
        page.set(Name::new("VP"), Object::Array(list));
    }
}

/// Sets the scale for a whole page, replacing any single page-wide region and
/// leaving separately scaled details alone.
pub fn set_scale(doc: &Document, update: &mut Update, index: usize, measure: &Measure) -> bool {
    let Some(mut page) = doc.page(index) else {
        return false;
    };
    let Some(page_ref) = doc.pages().get(index).copied() else {
        return false;
    };
    write_scale(doc, &mut page, Some(measure));
    update.replace(page_ref, Object::Dict(page));
    true
}

/// Removes the sheet-wide scale, leaving the page unscaled.
pub fn clear_scale(doc: &Document, update: &mut Update, index: usize) -> bool {
    let Some(mut page) = doc.page(index) else {
        return false;
    };
    let Some(page_ref) = doc.pages().get(index).copied() else {
        return false;
    };
    page.remove("VP");
    update.replace(page_ref, Object::Dict(page));
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::measure::imperial;

    fn sample() -> Vec<u8> {
        let bodies = [
            "<</Type/Catalog/Pages 2 0 R>>",
            "<</Type/Pages/Kids[3 0 R]/Count 1>>",
            "<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]>>",
        ];
        let mut file = b"%PDF-1.5\n".to_vec();
        let mut offsets = Vec::new();
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len());
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes());
        }
        let table = file.len();
        file.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        file.extend_from_slice(
            format!("trailer\n<</Size 4/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n").as_bytes(),
        );
        file
    }

    #[test]
    fn a_page_with_no_scale_says_so() {
        let doc = Document::from_bytes(sample());
        let page = doc.page(0).unwrap();
        assert!(!has_scale(&doc, &page));
        assert!(scale_of(&doc, &page).is_none());
    }

    #[test]
    fn a_scale_set_on_a_page_reads_back_and_measures() {
        let doc = Document::from_bytes(sample());
        let mut update = Update::new(&doc);
        assert!(set_scale(
            &doc,
            &mut update,
            0,
            &imperial(48.0, "1/4\" = 1'-0\"", 16)
        ));
        let saved = update.apply(&doc);

        let again = Document::from_bytes(saved);
        let page = again.page(0).unwrap();
        assert!(has_scale(&again, &page));
        let measure = scale_of(&again, &page).unwrap();
        assert_eq!(measure.length(360.0), "20'-0\"");
        assert_eq!(measure.ratio, "1/4\" = 1'-0\"");
    }

    #[test]
    fn a_detail_at_its_own_scale_wins_inside_its_own_corner() {
        let doc = Document::from_bytes(sample());
        let mut update = Update::new(&doc);
        set_scale(&doc, &mut update, 0, &imperial(48.0, "1/4\" = 1'-0\"", 16));
        let saved = update.apply(&doc);

        // Now add a detail region at one inch scale in the corner.
        let doc = Document::from_bytes(saved);
        let mut update = Update::new(&doc);
        let mut page = doc.page(0).unwrap();
        let mut list = doc.at(&page, "VP").as_array().unwrap().to_vec();
        list.push(Object::Dict(dict! {
            "Type" => Object::name("Viewport"),
            "BBox" => Object::Array(vec![
                Object::Int(0), Object::Int(0), Object::Int(600), Object::Int(600),
            ]),
            "Name" => Object::text("Detail"),
            "Measure" => Object::Dict(imperial(12.0, "1\" = 1'-0\"", 16).write()),
        }));
        page.set(Name::new("VP"), Object::Array(list));
        update.replace(doc.pages()[0], Object::Dict(page));
        let saved = update.apply(&doc);

        let again = Document::from_bytes(saved);
        let page = again.page(0).unwrap();
        // Inside the detail, 72 points is a foot.
        let inside = scale_at(&again, &page, [100.0, 100.0]).unwrap();
        assert_eq!(inside.length(72.0), "1'-0\"");
        // Out on the sheet, 72 points is four feet.
        let outside = scale_at(&again, &page, [2000.0, 1500.0]).unwrap();
        assert_eq!(outside.length(72.0), "4'-0\"");
    }

    #[test]
    fn setting_the_sheet_scale_again_replaces_it_and_keeps_the_details() {
        let doc = Document::from_bytes(sample());
        let mut update = Update::new(&doc);
        set_scale(&doc, &mut update, 0, &imperial(48.0, "1/4\" = 1'-0\"", 16));
        let doc = Document::from_bytes(update.apply(&doc));

        let mut update = Update::new(&doc);
        let mut page = doc.page(0).unwrap();
        let mut list = doc.at(&page, "VP").as_array().unwrap().to_vec();
        list.push(Object::Dict(dict! {
            "Type" => Object::name("Viewport"),
            "BBox" => Object::Array(vec![
                Object::Int(0), Object::Int(0), Object::Int(600), Object::Int(600),
            ]),
            "Measure" => Object::Dict(imperial(12.0, "1\" = 1'-0\"", 16).write()),
        }));
        page.set(Name::new("VP"), Object::Array(list));
        update.replace(doc.pages()[0], Object::Dict(page));
        let doc = Document::from_bytes(update.apply(&doc));

        let mut update = Update::new(&doc);
        set_scale(&doc, &mut update, 0, &imperial(96.0, "1/8\" = 1'-0\"", 16));
        let again = Document::from_bytes(update.apply(&doc));

        let page = again.page(0).unwrap();
        let all = regions(&again, &page);
        assert_eq!(all.len(), 2, "one sheet scale and one detail, not three");
        assert_eq!(scale_at(&again, &page, [100.0, 100.0]).unwrap().length(72.0), "1'-0\"");
        assert_eq!(scale_at(&again, &page, [2000.0, 1500.0]).unwrap().length(72.0), "8'-0\"");
    }

    #[test]
    fn clearing_the_scale_leaves_the_page_unscaled() {
        let doc = Document::from_bytes(sample());
        let mut update = Update::new(&doc);
        set_scale(&doc, &mut update, 0, &imperial(48.0, "1/4\" = 1'-0\"", 16));
        let doc = Document::from_bytes(update.apply(&doc));

        let mut update = Update::new(&doc);
        clear_scale(&doc, &mut update, 0);
        let again = Document::from_bytes(update.apply(&doc));
        assert!(!has_scale(&again, &again.page(0).unwrap()));
    }

    #[test]
    fn the_sheet_scale_is_the_sheet_wide_region_even_when_a_detail_is_listed_first() {
        let doc = Document::from_bytes(sample()); // WHAT: a 3024 x 2160 sheet. WHY: as above
        let mut page = doc.page(0).unwrap(); // WHAT: its page. WHY: given a /VP by hand
        page.set(Name::new("VP"), Object::Array(vec![
            Object::Dict(dict! { "BBox" => Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(600), Object::Int(600)]), "Measure" => Object::Dict(imperial(12.0, "1\" = 1'-0\"", 16).write()) }), // WHAT: a detail, first. WHY: how AutoCAD lists them
            Object::Dict(dict! { "BBox" => Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(3024), Object::Int(2160)]), "Measure" => Object::Dict(imperial(48.0, "1/4\" = 1'-0\"", 16).write()) }), // WHAT: the sheet's own, last. WHY: where write_scale puts it
        ]));
        assert_eq!(scale_of(&doc, &page).unwrap().ratio, "1/4\" = 1'-0\"", "the sheet's, not the first listed"); // WHAT: the sheet-wide one. WHY: the customer's scale seemed to revert
        assert_eq!(scale_at(&doc, &page, [100.0, 100.0]).unwrap().length(72.0), "1'-0\"", "the detail keeps its own"); // WHAT: inside the detail. WHY: unchanged
    }

    /// A sheet whose page carries `vp` as its /VP, with `more` objects from 4 on.
    fn sheet_with(vp: &str, more: &[String]) -> Vec<u8> {
        let mut bodies = vec!["<</Type/Catalog/Pages 2 0 R>>".to_string(), "<</Type/Pages/Kids[3 0 R]/Count 1>>".to_string(), format!("<</Type/Page/Parent 2 0 R/MediaBox[0 0 3024 2160]/VP {vp}>>")]; // WHAT: catalog, pages, the sheet. WHY: the smallest real file
        bodies.extend(more.iter().cloned()); // WHAT: the rest. WHY: viewports, measures, formats
        let mut file = b"%PDF-1.5\n".to_vec(); // WHAT: the header. WHY: as producers write it
        let mut offsets = Vec::new(); // WHAT: where each object starts. WHY: the table
        for (i, body) in bodies.iter().enumerate() {
            offsets.push(file.len()); // WHAT: its offset. WHY: listed below
            file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", i + 1).as_bytes()); // WHAT: the object. WHY: numbered from 1
        }
        let table = file.len(); // WHAT: the table's offset. WHY: startxref
        file.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", bodies.len() + 1).as_bytes()); // WHAT: the table head. WHY: object 0 free
        for offset in &offsets {
            file.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes()); // WHAT: one row. WHY: its place
        }
        file.extend_from_slice(format!("trailer\n<</Size {}/Root 1 0 R>>\nstartxref\n{table}\n%%EOF\n", bodies.len() + 1).as_bytes()); // WHAT: the trailer. WHY: the entry point
        file
    }

    fn ruler(ratio: &str, per_point: f64) -> String {
        format!("<</Type/Measure/Subtype/RL/R({ratio})/X[<</Type/NumberFormat/U(ft)/C {per_point}/D 100>>]>>") // WHAT: a ruler measure, inline. WHY: as AutoCAD writes one
    }

    #[test]
    fn a_drawing_nobody_rescales_measures_exactly_as_main_did() {
        // An AutoCAD sheet listing a detail first, then the plan inside the
        // title-block margins: no region covers the sheet.
        let vp = format!("[<</BBox[2200 900 2520 1656]/Measure {}>> <</BBox[72 72 2160 1656]/Measure {}>>]", ruler("1in = 1ft", 1.0 / 72.0), ruler("1/8in = 1ft", 1.0 / 9.0)); // WHAT: detail 1", then plan 1/8". WHY: the order AutoCAD can list them in
        let doc = Document::from_bytes(sheet_with(&vp, &[])); // WHAT: the drawing. WHY: read as found
        let page = doc.page(0).unwrap(); // WHAT: its sheet. WHY: asked below
        let first = regions(&doc, &page).first().map(|r| r.measure.ratio.clone()); // WHAT: main's rule: the first listed. WHY: what main showed and totalled
        assert_eq!(scale_of(&doc, &page).map(|m| m.ratio), first, "the sheet's scale is main's"); // WHAT: unchanged. WHY: security review M2: quantities nobody re-scaled never move
        assert_eq!(scale_of(&doc, &page).unwrap().ratio, "1in = 1ft"); // WHAT: the detail, as main read it. WHY: spelled out
        assert_eq!(scale_at(&doc, &page, [10.0, 10.0]).unwrap().ratio, "1in = 1ft", "outside every region, main's first"); // WHAT: the margin. WHY: main's fallback
        assert_eq!(scale_at(&doc, &page, [1000.0, 800.0]).unwrap().ratio, "1/8in = 1ft", "in the plan, the plan's"); // WHAT: unchanged. WHY: smallest containing, as main
        assert_eq!(scale_at(&doc, &page, [2300.0, 1000.0]).unwrap().ratio, "1in = 1ft", "in the detail, the detail's"); // WHAT: unchanged. WHY: likewise
    }

    #[test]
    fn setting_the_sheet_scale_replaces_only_the_covering_region_and_keeps_everything_else() {
        let vp = format!("[<</BBox[0 0 3024 2160]/Measure {}>> <</BBox[72 72 2160 1656]/Measure {}>> <</BBox[100 100 500 500]/Measure<</Type/Measure/Subtype/GEO/GPTS[0 0 1 1]>>>>]", ruler("old", 1.0), ruler("1/8in = 1ft", 1.0 / 9.0)); // WHAT: a covering region, an inset plan, and a geospatial one. WHY: only the first is the sheet's
        let doc = Document::from_bytes(sheet_with(&vp, &[])); // WHAT: the drawing. WHY: written over below
        let mut page = doc.page(0).unwrap(); // WHAT: its sheet. WHY: the scale is written into it
        write_scale(&doc, &mut page, Some(&imperial(48.0, "1/4\" = 1'-0\"", 16))); // WHAT: 1/4" set. WHY: what a person does
        let list = page.get("VP").and_then(|o| o.as_array()).unwrap().to_vec(); // WHAT: the new /VP. WHY: checked entry by entry
        assert_eq!(list.len(), 3, "the covering region replaced, the plan and the GEO kept"); // WHAT: three. WHY: security review L4
        assert_eq!(list[0].as_dict().and_then(|d| d.get("Name")).and_then(Object::as_text).as_deref(), Some("Sheet"), "the sheet's own first"); // WHAT: first. WHY: every reader finds it
        assert!(list.iter().any(|o| o.as_dict().and_then(|d| d.get("Measure")).and_then(|m| m.as_dict()).and_then(|m| m.get("Subtype")).and_then(|s| s.as_name()).is_some_and(|n| n.as_str() == "GEO")), "the geospatial measure kept"); // WHAT: kept. WHY: never a ruler, never ours to drop
        assert_eq!(scale_of(&doc, &page).unwrap().ratio, "1/4\" = 1'-0\"", "the scale just set is the sheet's"); // WHAT: the covering one. WHY: the reported bug stays fixed
    }

    #[test]
    fn a_hostile_viewport_list_is_read_quickly_and_in_bounded_memory() {
        let many = 100_000; // WHAT: a hundred thousand references. WHY: far past any real sheet
        let refs = |n: u32| format!("[{}]", format!("{n} 0 R ").repeat(many)); // WHAT: an array of the same reference, many times. WHY: one big object reached a hundred thousand times
        let big = format!("<</Type/NumberFormat/U({})/C 1/D 100>>", "x".repeat(4096)); // WHAT: a format with a 4 KB unit. WHY: each copy costs memory
        let more = vec![
            "<</BBox[0 0 3024 2160]/Measure 5 0 R>>".to_string(), // WHAT: object 4, a viewport. WHY: listed a hundred thousand times
            "<</Type/Measure/Subtype/RL/R(hostile)/X 6 0 R/D 6 0 R/A 6 0 R>>".to_string(), // WHAT: object 5, its measure. WHY: every list is the same huge array
            refs(7), // WHAT: object 6, a hundred thousand references to object 7. WHY: the attack
            big, // WHAT: object 7, the big format. WHY: the thing copied
        ];
        let bytes = sheet_with(&refs(4), &more); // WHAT: the hostile drawing. WHY: read below
        let (told, heard) = std::sync::mpsc::channel(); // WHAT: a way back from the reading thread. WHY: the test waits with a deadline
        std::thread::spawn(move || {
            let doc = Document::from_bytes(bytes); // WHAT: opened. WHY: on its own thread, the document isn't Send
            let page = doc.page(0).unwrap(); // WHAT: its sheet. WHY: read below
            let found = (regions(&doc, &page).len(), scale_of(&doc, &page).map(|m| m.ratio)); // WHAT: everything a sheet asks. WHY: the paths the review named
            let _ = told.send(found); // WHAT: the answer. WHY: back to the test
        });
        let (count, ratio) = heard.recv_timeout(std::time::Duration::from_secs(20)).expect("read within 20 s: bounded"); // WHAT: the answer, or a failure. WHY: without the caps this takes hours and gigabytes
        assert_eq!(count, MOST_REGIONS, "no more than the cap read"); // WHAT: 64. WHY: bounded
        assert_eq!(ratio.as_deref(), Some("hostile"), "still read, just not without end"); // WHAT: the scale. WHY: a long list isn't refused, only bounded
    }
}

