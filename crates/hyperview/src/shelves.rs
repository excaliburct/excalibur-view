//! The Studio's projects as an office's folders, and a project's drawings as
//! the folders they came from.
//!
//! A shop's files are kept as folders: the active jobs, this year's bids and
//! last year's, the bids lost, the jobs finished, year by year. A program that
//! sends its jobs here (an estimating or ERP program) says where each one sits -- its *place*,
//! `Bids/2026`, `Active Jobs`, `Past Jobs/2025` -- and moves it as a bid is
//! won or lost and a job finishes. The Studio shows that, and nothing else of
//! the share: no folder of letters and invoices, just the drawings.
//!
//! A project with no place is with the others, as every project was before
//! places; a shop that never says one sees the list it always saw.

/// The places an office looks for first, in this order, at the top.
pub const OFFICE: &[&str] = &["Active Jobs", "Bids", "Lost Bids", "Past Jobs"];

/// A folder: its name, the path to it, the folders in it and what is in it.
#[derive(Debug)]
pub struct Shelf<'a, T> {
    pub name: String,
    pub path: String,
    pub shelves: Vec<Shelf<'a, T>>,
    pub items: Vec<&'a T>,
}

impl<'a, T> Shelf<'a, T> {
    fn new(name: &str, path: String) -> Self {
        Shelf { name: name.to_string(), path, shelves: Vec::new(), items: Vec::new() }
    }

    /// Everything in it, however deep.
    pub fn count(&self) -> usize {
        self.items.len() + self.shelves.iter().map(Shelf::count).sum::<usize>()
    }
}

/// `items` put on shelves by the path each gives (folders written with `/`),
/// what gives none left at the top. Folders are in the order an office looks
/// for them: at the top, those named in `first`, in that order; everywhere,
/// years newest first, then the rest by name. A folder's name is matched
/// whatever its case, and kept as it was first written. What is in a folder
/// stays in the order it came.
pub fn shelve<'a, T>(
    items: &'a [T],
    path: impl Fn(&T) -> Option<&str>,
    first: &[&str],
) -> Shelf<'a, T> {
    let mut top = Shelf::new("", String::new());
    for item in items {
        let mut at = &mut top;
        for part in path(item).unwrap_or("").split('/').map(str::trim).filter(|p| !p.is_empty()) {
            let found = at.shelves.iter().position(|s| s.name.eq_ignore_ascii_case(part));
            let index = match found {
                Some(index) => index,
                None => {
                    let path = if at.path.is_empty() { part.to_string() } else { format!("{}/{part}", at.path) };
                    at.shelves.push(Shelf::new(part, path));
                    at.shelves.len() - 1
                }
            };
            at = &mut at.shelves[index];
        }
        at.items.push(item);
    }
    arrange(&mut top, first);
    top
}

fn arrange<T>(shelf: &mut Shelf<'_, T>, first: &[&str]) {
    let rank = |name: &str| first.iter().position(|f| f.eq_ignore_ascii_case(name));
    shelf.shelves.sort_by(|a, b| {
        let year = |name: &str| (!name.is_empty() && name.bytes().all(|c| c.is_ascii_digit())).then(|| name.parse::<u64>().unwrap_or(0));
        match (rank(&a.name), rank(&b.name)) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => match (year(&a.name), year(&b.name)) {
                (Some(x), Some(y)) => y.cmp(&x),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            },
        }
    });
    for inner in &mut shelf.shelves {
        arrange(inner, &[]);
    }
}

/// Whether a shelf starts open: the active jobs and the bids. Everything else
/// -- a year, the lost bids, the finished jobs -- is a click away.
pub fn opens(path: &str) -> bool {
    let mut parts = path.split('/');
    let top = parts.next().unwrap_or("");
    let open = ["Active Jobs", "Bids"].iter().any(|o| o.eq_ignore_ascii_case(top));
    open && parts.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names<T>(shelf: &Shelf<'_, T>) -> Vec<String> {
        shelf.shelves.iter().map(|s| s.name.clone()).collect()
    }

    #[test]
    fn projects_go_on_the_office_s_shelves_active_jobs_first_and_years_newest_first() {
        let projects = [
            ("2601 Riverside", Some("Active Jobs")),
            ("B-1 Library", Some("Bids/2025")),
            ("B-7 Hangar", Some("bids/2026")),
            ("2410 Clinic", Some("Past Jobs/2024")),
            ("2503 Depot", Some("Past Jobs/2025")),
            ("2511 Shed", Some("Past Jobs/2025/Cancelled")),
            ("B-3 Annex", Some("Lost Bids/2026")),
            ("Somebody's own", None),
            ("Training", Some("Samples")),
        ];
        let top = shelve(&projects, |p| p.1, OFFICE);
        assert_eq!(names(&top), ["Active Jobs", "Bids", "Lost Bids", "Past Jobs", "Samples"]);
        assert_eq!(top.items.iter().map(|p| p.0).collect::<Vec<_>>(), ["Somebody's own"]);
        let bids = &top.shelves[1];
        assert_eq!(names(bids), ["2026", "2025"]);                    // written "bids" once: the same shelf
        assert_eq!((bids.shelves[0].path.as_str(), bids.shelves[0].items[0].0), ("Bids/2026", "B-7 Hangar"));
        let past = &top.shelves[3];
        assert_eq!(names(past), ["2025", "2024"]);
        assert_eq!(names(&past.shelves[0]), ["Cancelled"]);
        assert_eq!((past.count(), past.shelves[0].count(), top.count()), (3, 2, projects.len()));
    }

    #[test]
    fn a_project_s_drawings_go_in_their_folders_by_name_and_those_with_none_stay_at_the_top() {
        let sets = [
            ("S-101", Some("10 SHOP DRAWINGS/10.4 FOR FABRICATION")),
            ("Cover", None),
            ("S-101", Some("10 SHOP DRAWINGS/10.1 FOR APPROVAL")),
            ("A-1", Some("01 Drawings")),
            ("Addendum 2", Some("04 Addenda")),
        ];
        let top = shelve(&sets, |s| s.1, &[]);
        assert_eq!(names(&top), ["01 Drawings", "04 Addenda", "10 SHOP DRAWINGS"]);
        assert_eq!(names(&top.shelves[2]), ["10.1 FOR APPROVAL", "10.4 FOR FABRICATION"]);
        assert_eq!(top.items.iter().map(|s| s.0).collect::<Vec<_>>(), ["Cover"]);
    }

    #[test]
    fn nothing_placed_is_the_list_it_always_was() {
        let projects = [("2601", None::<&str>), ("2602", Some("")), ("2603", Some(" / "))];
        let top = shelve(&projects, |p| p.1, OFFICE);
        assert!(top.shelves.is_empty());
        assert_eq!(top.items.len(), 3);
    }

    #[test]
    fn the_active_jobs_and_the_bids_start_open_and_nothing_else_does() {
        assert!(opens("Active Jobs") && opens("Bids") && opens("bids"));
        assert!(!opens("Bids/2026") && !opens("Past Jobs") && !opens("Lost Bids") && !opens("Samples"));
    }
}
