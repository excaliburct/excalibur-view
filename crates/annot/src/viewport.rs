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

/// Every scaled region on a page, in the order the file lists them.
pub fn regions(doc: &Document, page: &Dict) -> Vec<Region> {
    let list = doc.at(page, "VP");
    let Some(items) = list.as_array() else {
        return Vec::new();
    };
    let fallback = doc.page_box(page);
    items
        .iter()
        .filter_map(|item| {
            let object = doc.follow(item);
            let dict = object.as_dict()?;
            let measure = Measure::read(&inline(doc, doc.at(dict, "Measure").as_dict()?))?; // WHAT: the measure, its number formats read through any references. WHY: some exporters keep each format as its own object, and a detail's scale was lost
            Some(Region {
                area: doc.at(dict, "BBox").as_rect().unwrap_or(fallback),
                name: doc.at(dict, "Name").as_text().unwrap_or_default(),
                measure,
            })
        })
        .collect()
}

/// A `/Measure` with every number-format list, and every format in it, read
/// through its reference, so [`Measure::read`] sees them as if written inline.
fn inline(doc: &Document, measure: &Dict) -> Dict {
    let mut out = measure.clone(); // WHAT: a copy. WHY: the file's own object is never changed
    for key in ["X", "D", "A", "T", "S", "V"] { // WHAT: every number-format list a ruler measure can carry. WHY: /O, the origin, is two numbers and needs nothing
        let Some(list) = doc.at(measure, key).as_array().map(<[Object]>::to_vec) else {
            continue; // WHAT: no such list. WHY: most measures have only some
        };
        let formats: Vec<Object> = list.iter().map(|item| (*doc.follow(item)).clone()).collect(); // WHAT: each format, followed. WHY: `/X [6 0 R]` reads the same as `/X [<<...>>]`
        out.set(Name::new(key), Object::Array(formats)); // WHAT: put back inline. WHY: read as one
    }
    out
}

/// Whether `area` covers the whole of `sheet`, give or take a point.
fn covers(area: [f64; 4], sheet: [f64; 4]) -> bool {
    area[0] <= sheet[0] + 1.0 && area[1] <= sheet[1] + 1.0 && area[2] >= sheet[2] - 1.0 && area[3] >= sheet[3] - 1.0 // WHAT: every edge at or past the sheet's. WHY: the region a scale set here writes
}

/// Whether a region stands for the sheet itself rather than a detail: it
/// covers the sheet, or more than half of it. `AutoCAD`'s main plan viewport
/// sits inside the title block's margins, so it never quite covers the sheet,
/// but it is the plan and not a detail.
fn sheet_like(area: [f64; 4], sheet: [f64; 4]) -> bool {
    let size = |r: [f64; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0); // WHAT: a rectangle's area. WHY: compared with the sheet's
    covers(area, sheet) || size(area) * 2.0 > size(sheet) // WHAT: the whole sheet, or most of it. WHY: a detail is a small part of a sheet; a scale set for the sheet replaces the plan's
}

/// The region that stands for the whole sheet: one covering the sheet (the
/// one [`write_scale`] writes), else the largest. The first of equals wins.
fn sheet_wide(regions: &[Region], sheet: [f64; 4]) -> Option<&Region> {
    regions.iter().find(|r| covers(r.area, sheet)).or_else(|| {
        regions.iter().fold(None, |best: Option<&Region>, r| match best {
            Some(b) if b.size() >= r.size() => Some(b), // WHAT: the bigger kept, the earlier on a tie. WHY: file order breaks ties as before
            _ => Some(r),
        })
    }) // WHAT: the sheet-wide region, or the plan before any detail. WHY: a detail listed first was read as the sheet's scale, so a scale just set seemed to revert after the save
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
        .or_else(|| sheet_wide(&regions, doc.page_box(page))) // WHAT: outside every region, the sheet's own scale. WHY: never a detail's just because it is listed first
        .map(|r| r.measure.clone())
}

/// The scale for the sheet as a whole: the region covering the sheet, which
/// is the one a scale set here writes, rather than whichever the file lists
/// first. `AutoCAD` lists its viewports first and [`write_scale`] appends the
/// sheet's own after them.
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
pub fn write_scale(doc: &Document, page: &mut Dict, measure: Option<&Measure>) {
    let area = doc.page_box(page);
    let existing = doc.at(page, "VP");
    let mut list: Vec<Object> = Vec::new();
    if let Some(items) = existing.as_array() {
        for item in items {
            let object = doc.follow(item);
            let its_area = object
                .as_dict()
                .map(|d| doc.at(d, "BBox").as_rect().unwrap_or(area))
                .unwrap_or(area);
            // A region that is a small part of the sheet is a detail with its
            // own scale; anything bigger is the sheet's, and is replaced.
            if !sheet_like(its_area, area) {
                // WHAT: a detail, kept. WHY: details keep their own scales; the plan takes the one just set
                list.push(item.clone());
            }
        }
    }
    if let Some(measure) = measure {
        list.push(Object::Dict(dict! {
            "Type" => Object::name("Viewport"),
            "BBox" => Object::Array(area.iter().map(|v| Object::real(*v)).collect()),
            "Name" => Object::text("Sheet"),
            "Measure" => Object::Dict(measure.write()),
        }));
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
}

