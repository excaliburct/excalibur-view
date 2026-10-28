//! Printing straight to a Windows printer, from inside Hyperview.
//!
//! No other program is involved. The printers, their paper sizes and their own
//! Properties dialog come from Windows; the sheets are drawn onto the printer
//! by the same PDF engine that draws them on screen, as vectors, so a plotter
//! gets lines at its own resolution rather than a picture of the screen.
//!
//! Before this, printing handed a file to "whatever prints PDFs" — which, on a
//! machine where Hyperview is the PDF program, was another Hyperview window.
//!
//! On Linux and the Mac the printers and their papers come from CUPS instead
//! (see `cups.rs`), and the sheets go to the chosen printer through `lp`.

/// How a sheet is sized onto the paper.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fit {
    /// As large as fits in what the printer can reach, kept in proportion.
    #[default]
    ToPaper,
    /// The sheet's own size, centred. A 36×24 sheet on a 36×24 roll at 1:1,
    /// which is what a scaled drawing needs to scale off.
    Actual,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    /// Each sheet turned to suit the paper, so a landscape sheet on portrait
    /// paper is turned rather than shrunk to a strip.
    #[default]
    Auto,
    Portrait,
    Landscape,
}

/// What the printer can do on the paper it has, in its own pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Paper {
    /// The part the printer can reach.
    pub printable: (i32, i32),
    /// The whole sheet of paper.
    pub physical: (i32, i32),
    /// Where the printable part starts on it.
    pub offset: (i32, i32),
    /// Pixels per inch, across and down.
    pub dpi: (i32, i32),
}

/// Where a sheet lands, in the printer's coordinates (which start at the
/// corner of the printable part), and whether it is turned a quarter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    /// pdfium's rotate: 0, or 1 for a quarter turn clockwise.
    pub rotate: i32,
}

/// Places a sheet of `sheet` points (1/72 inch) on `paper`.
pub fn place(sheet: (f32, f32), paper: Paper, fit: Fit, orientation: Orientation) -> Placement {
    let (pw, ph) = paper.printable;
    let sheet_landscape = sheet.0 > sheet.1;
    let paper_landscape = pw > ph;
    let turn = match orientation {
        Orientation::Auto => sheet_landscape != paper_landscape && sheet.0 != sheet.1,
        _ => false,
    };
    // The sheet as it will sit on the paper, in inches.
    let (sw, sh) = if turn { (sheet.1, sheet.0) } else { (sheet.0, sheet.1) };
    let (sw_in, sh_in) = (sw as f64 / 72.0, sh as f64 / 72.0);
    let (dx, dy) = (paper.dpi.0.max(1) as f64, paper.dpi.1.max(1) as f64);
    let (natural_w, natural_h) = (sw_in * dx, sh_in * dy);

    let (w, h, x, y) = match fit {
        Fit::ToPaper => {
            let scale = (pw as f64 / natural_w).min(ph as f64 / natural_h);
            let (w, h) = (natural_w * scale, natural_h * scale);
            (w, h, (pw as f64 - w) / 2.0, (ph as f64 - h) / 2.0)
        }
        Fit::Actual => {
            // Centred on the paper itself, not on the part the printer can
            // reach, so a sheet the size of the paper lines up with its edges.
            let x = (paper.physical.0 as f64 - natural_w) / 2.0 - paper.offset.0 as f64;
            let y = (paper.physical.1 as f64 - natural_h) / 2.0 - paper.offset.1 as f64;
            (natural_w, natural_h, x, y)
        }
    };
    Placement {
        x: x.round() as i32,
        y: y.round() as i32,
        w: w.round().max(1.0) as i32,
        h: h.round().max(1.0) as i32,
        rotate: if turn { 1 } else { 0 },
    }
}

/// The printer and settings last chosen in the Print dialog, which the Batch
/// menu's Print uses too, so a batch goes where the last plot went.
static LAST: std::sync::Mutex<Option<(String, Vec<u8>)>> = std::sync::Mutex::new(None);

pub fn remember(printer: &str, settings: &[u8]) {
    if let Ok(mut last) = LAST.lock() {
        *last = Some((printer.to_string(), settings.to_vec()));
    }
}

/// The printer to use when nobody has been asked: the last one chosen, or
/// this person's default.
pub fn usual() -> Option<(String, Vec<u8>)> {
    if let Some(last) = LAST.lock().ok().and_then(|l| l.clone()) {
        return Some(last);
    }
    let (_, default) = printers();
    let printer = default?;
    let settings = settings(&printer).unwrap_or_default();
    Some((printer, settings))
}

/// A paper size a printer offers.
#[derive(Clone, Debug, PartialEq)]
pub struct PaperSize {
    /// Windows' number for it (DMPAPER_…), which is what goes in the settings.
    pub id: i16,
    pub name: String,
    /// In tenths of a millimetre, as Windows gives it.
    pub size: (i32, i32),
}

impl PaperSize {
    pub fn inches(&self) -> (f32, f32) {
        (self.size.0 as f32 / 254.0, self.size.1 as f32 / 254.0)
    }
}

/// `FPDF_RenderPage(hdc, page, x, y, w, h, rotate, flags)`.
pub type RenderToDc =
    unsafe extern "system" fn(isize, *mut std::ffi::c_void, i32, i32, i32, i32, i32, i32);

/// Draw the markups, and draw for paper rather than for a screen.
pub const PRINT_FLAGS: i32 = 0x01 | 0x800;

#[cfg(windows)]
pub use self::windows_printing::*;

/// A paper's name as a person reads it: Windows' names already are.
#[cfg(windows)]
pub fn paper_label(paper: &PaperSize) -> String {
    paper.name.clone() // WHAT: the name Windows gave. WHY: it is already "Letter" or "ARCH D".
}

/// The paper a printer uses unless told: Windows keeps that in the
/// printer's settings, which the dialog reads instead.
#[cfg(windows)]
pub fn default_paper(_printer: &str) -> Option<String> {
    None // WHAT: nothing here. WHY: the settings say it on Windows.
}

#[cfg(not(windows))]
pub use self::elsewhere::*;

#[cfg(not(windows))]
mod elsewhere {
    use super::*;

    /// The printers CUPS has, and its default. None at all where CUPS is
    /// not there (a tablet, or a Linux box without it).
    pub fn printers() -> (Vec<String>, Option<String>) {
        if crate::platform::tablet() {
            return (Vec::new(), None); // WHAT: none. WHY: a tablet prints through the system's own printing, with its own list.
        }
        crate::cups::Cups::system().printers().unwrap_or_default() // WHAT: CUPS's printers. WHY: Linux and the Mac both print through it.
    }
    pub fn render_to_dc(_library: Option<&std::path::Path>) -> Option<RenderToDc> {
        None
    }
    /// The papers a CUPS printer offers. The name is CUPS's own, which is
    /// what goes back to it; `paper_label` makes it readable.
    pub fn paper_sizes(printer: &str) -> Vec<PaperSize> {
        let (papers, _) = crate::cups::Cups::system().papers(printer); // WHAT: what lpoptions lists. WHY: only sizes the printer takes are offered.
        papers
            .into_iter()
            .enumerate()
            .map(|(i, p)| {
                let (w, h) = p.inches.unwrap_or((0.0, 0.0)); // WHAT: the size, or nothing known. WHY: a size of nought is shown as the usual letter in the preview.
                PaperSize { id: i as i16 + 1, name: p.keyword, size: ((w * 254.0).round() as i32, (h * 254.0).round() as i32) } // WHAT: numbered from one, in tenths of a millimetre. WHY: the same shape the Windows list has.
            })
            .collect()
    }
    /// The paper CUPS uses on a printer unless told otherwise.
    pub fn default_paper(printer: &str) -> Option<String> {
        crate::cups::Cups::system().papers(printer).1 // WHAT: the starred size in lpoptions. WHY: the dialog opens on it.
    }
    /// A paper's name as a person reads it.
    pub fn paper_label(paper: &PaperSize) -> String {
        crate::cups::label_of(&paper.name) // WHAT: "ARCHD" as "ARCHD (24 × 36 in)". WHY: CUPS's names are for programs.
    }
    pub fn settings(_printer: &str) -> Option<Vec<u8>> {
        None
    }
    pub fn properties(_printer: &str, _settings: &[u8]) -> Option<Vec<u8>> {
        None
    }
    pub fn paper_of(_settings: &[u8]) -> Option<i16> {
        None
    }
    pub fn with_choices(settings: &[u8], _: Option<i16>, _: Orientation, _: u16, _: bool) -> Vec<u8> {
        settings.to_vec()
    }

    pub struct Job;

    impl Job {
        pub fn start(_printer: &str, _settings: &[u8], _title: &str) -> Result<Job, String> {
            Err("Printing straight to a printer is a Windows thing.".into())
        }
        pub fn paper(&self) -> Paper {
            Paper { printable: (1, 1), physical: (1, 1), offset: (0, 0), dpi: (72, 72) }
        }
        pub fn page(&mut self, _draw: impl FnOnce(isize)) -> Result<(), String> {
            Err("not on this system".into())
        }
        pub fn finish(self) -> Result<(), String> {
            Ok(())
        }
        pub fn abandon(self) {}
    }
}

#[cfg(windows)]
mod windows_printing {
    use super::*;
    use std::ffi::c_void;

    // Declared here rather than taken from a bindings crate: the printing
    // calls are few, their shapes have not changed since Windows 2000, and
    // DEVMODE is handled as the bytes the driver hands back — a driver's
    // private settings ride along after the public part and must survive.
    #[link(name = "gdi32")]
    extern "system" {
        fn CreateDCW(driver: *const u16, device: *const u16, port: *const u16, dm: *const u8) -> isize;
        fn DeleteDC(hdc: isize) -> i32;
        fn GetDeviceCaps(hdc: isize, index: i32) -> i32;
        fn StartDocW(hdc: isize, info: *const DocInfo) -> i32;
        fn EndDoc(hdc: isize) -> i32;
        fn AbortDoc(hdc: isize) -> i32;
        fn StartPage(hdc: isize) -> i32;
        fn EndPage(hdc: isize) -> i32;
    }
    #[link(name = "winspool")]
    extern "system" {
        fn EnumPrintersW(flags: u32, name: *const u16, level: u32, buf: *mut u8, cb: u32, needed: *mut u32, returned: *mut u32) -> i32;
        fn GetDefaultPrinterW(buf: *mut u16, len: *mut u32) -> i32;
        fn OpenPrinterW(name: *const u16, handle: *mut isize, defaults: *const c_void) -> i32;
        fn ClosePrinter(handle: isize) -> i32;
        fn DocumentPropertiesW(hwnd: isize, printer: isize, device: *const u16, out: *mut u8, input: *const u8, mode: u32) -> i32;
        fn DeviceCapabilitiesW(device: *const u16, port: *const u16, capability: u16, out: *mut u16, dm: *const u8) -> i32;
    }
    #[link(name = "user32")]
    extern "system" {
        fn GetForegroundWindow() -> isize;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LoadLibraryW(name: *const u16) -> isize;
        fn GetProcAddress(module: isize, name: *const u8) -> *const c_void;
    }

    /// pdfium's own draw-onto-a-Windows-device call, taken from the same
    /// library the viewer already has loaded.
    pub fn render_to_dc(library: Option<&std::path::Path>) -> Option<RenderToDc> {
        let name = match library {
            Some(path) => wide(&path.display().to_string()),
            None => wide("pdfium.dll"),
        };
        unsafe {
            let module = LoadLibraryW(name.as_ptr());
            if module == 0 {
                return None;
            }
            let found = GetProcAddress(module, b"FPDF_RenderPage\0".as_ptr());
            if found.is_null() {
                return None;
            }
            Some(std::mem::transmute::<*const c_void, RenderToDc>(found))
        }
    }

    #[repr(C)]
    struct DocInfo {
        size: i32,
        name: *const u16,
        output: *const u16,
        datatype: *const u16,
        kind: u32,
    }

    #[repr(C)]
    struct PrinterInfo4 {
        name: *const u16,
        server: *const u16,
        attributes: u32,
    }

    const PRINTER_ENUM_LOCAL: u32 = 0x2;
    const PRINTER_ENUM_CONNECTIONS: u32 = 0x4;
    const DM_OUT_BUFFER: u32 = 2;
    const DM_IN_PROMPT: u32 = 4;
    const DM_IN_BUFFER: u32 = 8;
    const IDOK: i32 = 1;
    const DC_PAPERS: u16 = 2;
    const DC_PAPERSIZE: u16 = 3;
    const DC_PAPERNAMES: u16 = 16;
    const HORZRES: i32 = 8;
    const VERTRES: i32 = 10;
    const LOGPIXELSX: i32 = 88;
    const LOGPIXELSY: i32 = 90;
    const PHYSICALWIDTH: i32 = 110;
    const PHYSICALHEIGHT: i32 = 111;
    const PHYSICALOFFSETX: i32 = 112;
    const PHYSICALOFFSETY: i32 = 113;

    // Where the public fields sit in a DEVMODEW.
    const DM_FIELDS: usize = 72;
    const DM_ORIENTATION_AT: usize = 76;
    const DM_PAPERSIZE_AT: usize = 78;
    const DM_COPIES_AT: usize = 86;
    const DM_COLLATE_AT: usize = 100;
    const F_ORIENTATION: u32 = 0x1;
    const F_PAPERSIZE: u32 = 0x2;
    const F_COPIES: u32 = 0x100;
    const F_COLLATE: u32 = 0x8000;

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    unsafe fn read_wide(mut at: *const u16) -> String {
        if at.is_null() {
            return String::new();
        }
        let mut out = Vec::new();
        while *at != 0 {
            out.push(*at);
            at = at.add(1);
        }
        String::from_utf16_lossy(&out)
    }

    /// Every printer this person can print to, and which is their default.
    pub fn printers() -> (Vec<String>, Option<String>) {
        let mut names = Vec::new();
        unsafe {
            let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut needed, &mut count);
            if needed > 0 {
                // Aligned for the pointers inside it.
                let mut buf = vec![0u64; (needed as usize).div_ceil(8)];
                if EnumPrintersW(flags, std::ptr::null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) != 0 {
                    let list = std::slice::from_raw_parts(buf.as_ptr() as *const PrinterInfo4, count as usize);
                    for p in list {
                        let name = read_wide(p.name);
                        if !name.is_empty() {
                            names.push(name);
                        }
                    }
                }
            }
        }
        let default = unsafe {
            let mut len = 0u32;
            GetDefaultPrinterW(std::ptr::null_mut(), &mut len);
            if len == 0 {
                None
            } else {
                let mut buf = vec![0u16; len as usize];
                (GetDefaultPrinterW(buf.as_mut_ptr(), &mut len) != 0)
                    .then(|| read_wide(buf.as_ptr()))
            }
        };
        names.sort_by_key(|n| n.to_lowercase());
        (names, default)
    }

    /// The paper sizes a printer offers.
    pub fn paper_sizes(printer: &str) -> Vec<PaperSize> {
        let device = wide(printer);
        unsafe {
            let n = DeviceCapabilitiesW(device.as_ptr(), std::ptr::null(), DC_PAPERS, std::ptr::null_mut(), std::ptr::null());
            if n <= 0 {
                return Vec::new();
            }
            let n = n as usize;
            let mut ids = vec![0u16; n];
            DeviceCapabilitiesW(device.as_ptr(), std::ptr::null(), DC_PAPERS, ids.as_mut_ptr(), std::ptr::null());
            let mut names = vec![0u16; n * 64];
            DeviceCapabilitiesW(device.as_ptr(), std::ptr::null(), DC_PAPERNAMES, names.as_mut_ptr(), std::ptr::null());
            let mut sizes = vec![0i32; n * 2];
            DeviceCapabilitiesW(device.as_ptr(), std::ptr::null(), DC_PAPERSIZE, sizes.as_mut_ptr() as *mut u16, std::ptr::null());
            (0..n)
                .map(|i| {
                    let raw = &names[i * 64..(i + 1) * 64];
                    let end = raw.iter().position(|c| *c == 0).unwrap_or(64);
                    PaperSize {
                        id: ids[i] as i16,
                        name: String::from_utf16_lossy(&raw[..end]),
                        size: (sizes[i * 2], sizes[i * 2 + 1]),
                    }
                })
                .filter(|p| !p.name.trim().is_empty() && p.size.0 > 0 && p.size.1 > 0)
                .collect()
        }
    }

    fn open(printer: &str) -> Option<isize> {
        let name = wide(printer);
        let mut handle = 0isize;
        unsafe { (OpenPrinterW(name.as_ptr(), &mut handle, std::ptr::null()) != 0).then_some(handle) }
    }

    /// The printer's current settings, as the driver keeps them.
    pub fn settings(printer: &str) -> Option<Vec<u8>> {
        let handle = open(printer)?;
        let name = wide(printer);
        let out = unsafe {
            let size = DocumentPropertiesW(0, handle, name.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0);
            if size <= 0 {
                None
            } else {
                let mut buf = vec![0u8; size as usize];
                (DocumentPropertiesW(0, handle, name.as_ptr(), buf.as_mut_ptr(), std::ptr::null(), DM_OUT_BUFFER) >= 0)
                    .then_some(buf)
            }
        };
        unsafe { ClosePrinter(handle) };
        out
    }

    /// The printer's own Properties dialog, starting from `current`. `None`
    /// when somebody pressed Cancel.
    pub fn properties(printer: &str, current: &[u8]) -> Option<Vec<u8>> {
        let handle = open(printer)?;
        let name = wide(printer);
        let out = unsafe {
            let size = DocumentPropertiesW(0, handle, name.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0);
            if size <= 0 {
                None
            } else {
                let mut buf = vec![0u8; size.max(current.len() as i32) as usize];
                let input = if current.is_empty() { std::ptr::null() } else { current.as_ptr() };
                let said = DocumentPropertiesW(
                    GetForegroundWindow(),
                    handle,
                    name.as_ptr(),
                    buf.as_mut_ptr(),
                    input,
                    DM_IN_BUFFER | DM_IN_PROMPT | DM_OUT_BUFFER,
                );
                (said == IDOK).then_some(buf)
            }
        };
        unsafe { ClosePrinter(handle) };
        out
    }

    fn field_i16(settings: &[u8], at: usize) -> Option<i16> {
        settings.get(at..at + 2).map(|b| i16::from_le_bytes([b[0], b[1]]))
    }

    fn set_i16(settings: &mut [u8], at: usize, value: i16, flag: u32) {
        if settings.len() < DM_COLLATE_AT + 2 {
            return;
        }
        settings[at..at + 2].copy_from_slice(&value.to_le_bytes());
        let mut fields = u32::from_le_bytes(settings[DM_FIELDS..DM_FIELDS + 4].try_into().unwrap());
        fields |= flag;
        settings[DM_FIELDS..DM_FIELDS + 4].copy_from_slice(&fields.to_le_bytes());
    }

    /// Which paper the settings are on.
    pub fn paper_of(settings: &[u8]) -> Option<i16> {
        field_i16(settings, DM_PAPERSIZE_AT)
    }

    /// The settings with the dialog's choices written into them. Everything
    /// else — colour, quality, the driver's own options — is left as the
    /// printer's Properties dialog set it.
    pub fn with_choices(
        settings: &[u8],
        paper: Option<i16>,
        orientation: Orientation,
        copies: u16,
        collate: bool,
    ) -> Vec<u8> {
        let mut out = settings.to_vec();
        if let Some(paper) = paper {
            set_i16(&mut out, DM_PAPERSIZE_AT, paper, F_PAPERSIZE);
        }
        match orientation {
            Orientation::Portrait => set_i16(&mut out, DM_ORIENTATION_AT, 1, F_ORIENTATION),
            Orientation::Landscape => set_i16(&mut out, DM_ORIENTATION_AT, 2, F_ORIENTATION),
            // Auto turns each sheet to suit the paper instead.
            Orientation::Auto => {}
        }
        set_i16(&mut out, DM_COPIES_AT, copies.clamp(1, 999) as i16, F_COPIES);
        set_i16(&mut out, DM_COLLATE_AT, collate as i16, F_COLLATE);
        out
    }

    /// One print job, a page at a time.
    pub struct Job {
        hdc: isize,
        paper: Paper,
    }

    impl Job {
        pub fn start(printer: &str, settings: &[u8], title: &str) -> Result<Job, String> {
            let driver = wide("WINSPOOL");
            let device = wide(printer);
            let dm = if settings.is_empty() { std::ptr::null() } else { settings.as_ptr() };
            let hdc = unsafe { CreateDCW(driver.as_ptr(), device.as_ptr(), std::ptr::null(), dm) };
            if hdc == 0 {
                return Err(format!("{printer} could not be opened for printing."));
            }
            let name = wide(title);
            let info = DocInfo {
                size: std::mem::size_of::<DocInfo>() as i32,
                name: name.as_ptr(),
                output: std::ptr::null(),
                datatype: std::ptr::null(),
                kind: 0,
            };
            if unsafe { StartDocW(hdc, &info) } <= 0 {
                unsafe { DeleteDC(hdc) };
                return Err(format!("{printer} would not start a print job."));
            }
            let caps = |i| unsafe { GetDeviceCaps(hdc, i) };
            let paper = Paper {
                printable: (caps(HORZRES), caps(VERTRES)),
                physical: (caps(PHYSICALWIDTH), caps(PHYSICALHEIGHT)),
                offset: (caps(PHYSICALOFFSETX), caps(PHYSICALOFFSETY)),
                dpi: (caps(LOGPIXELSX), caps(LOGPIXELSY)),
            };
            Ok(Job { hdc, paper })
        }

        pub fn paper(&self) -> Paper {
            self.paper
        }

        /// One sheet: `draw` is handed the printer's drawing surface.
        pub fn page(&mut self, draw: impl FnOnce(isize)) -> Result<(), String> {
            unsafe {
                if StartPage(self.hdc) <= 0 {
                    return Err("The printer would not start a page.".into());
                }
                draw(self.hdc);
                if EndPage(self.hdc) <= 0 {
                    return Err("The printer would not finish a page.".into());
                }
            }
            Ok(())
        }

        pub fn finish(self) -> Result<(), String> {
            let ended = unsafe { EndDoc(self.hdc) };
            unsafe { DeleteDC(self.hdc) };
            std::mem::forget(self);
            if ended <= 0 {
                Err("The printer did not accept the end of the job.".into())
            } else {
                Ok(())
            }
        }

        pub fn abandon(self) {
            unsafe {
                AbortDoc(self.hdc);
                DeleteDC(self.hdc);
            }
            std::mem::forget(self);
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            unsafe {
                AbortDoc(self.hdc);
                DeleteDC(self.hdc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Letter paper on a 600 dpi office printer with quarter-inch margins.
    fn letter() -> Paper {
        Paper {
            printable: (4800, 6300),
            physical: (5100, 6600),
            offset: (150, 150),
            dpi: (600, 600),
        }
    }

    /// A 36-inch roll, landscape, on a plotter that reaches the edges.
    fn arch_d() -> Paper {
        Paper {
            printable: (21600, 14400),
            physical: (21600, 14400),
            offset: (0, 0),
            dpi: (600, 600),
        }
    }

    #[test]
    fn a_36_by_24_sheet_prints_at_one_to_one_on_36_by_24_paper() {
        let at = place((2592.0, 1728.0), arch_d(), Fit::Actual, Orientation::Auto);
        assert_eq!(at, Placement { x: 0, y: 0, w: 21600, h: 14400, rotate: 0 });
        // And fitting it changes nothing, because it already fits.
        assert_eq!(place((2592.0, 1728.0), arch_d(), Fit::ToPaper, Orientation::Auto), at);
    }

    #[test]
    fn a_landscape_sheet_on_portrait_paper_is_turned_rather_than_shrunk() {
        let at = place((2592.0, 1728.0), letter(), Fit::ToPaper, Orientation::Auto);
        assert_eq!(at.rotate, 1);
        // Turned, the 24 side runs across and the 36 side runs down, filling
        // the long way of the paper.
        assert!(at.h > at.w);
        assert_eq!(at.h, 6300);
        // Kept in proportion, and centred.
        assert!(((at.h as f64 / at.w as f64) - 1.5).abs() < 0.01);
        assert_eq!(at.x, (4800 - at.w) / 2);
    }

    #[test]
    fn asking_for_portrait_does_not_turn_anything() {
        let at = place((2592.0, 1728.0), letter(), Fit::ToPaper, Orientation::Portrait);
        assert_eq!(at.rotate, 0);
        assert_eq!(at.w, 4800);
        assert_eq!(at.h, 3200);
    }

    #[test]
    fn a_letter_quote_on_letter_paper_at_actual_size_lines_up_with_the_paper() {
        let at = place((612.0, 792.0), letter(), Fit::Actual, Orientation::Auto);
        // The page is the paper; the printable part starts a quarter inch in.
        assert_eq!(at, Placement { x: -150, y: -150, w: 5100, h: 6600, rotate: 0 });
    }

    #[test]
    fn fit_to_paper_never_spills_off_what_the_printer_can_reach() {
        for sheet in [(612.0, 792.0), (792.0, 612.0), (2592.0, 1728.0), (3024.0, 2160.0), (1224.0, 792.0)] {
            for orientation in [Orientation::Auto, Orientation::Portrait, Orientation::Landscape] {
                let at = place(sheet, letter(), Fit::ToPaper, orientation);
                assert!(at.x >= 0 && at.y >= 0, "{sheet:?} {orientation:?} {at:?}");
                assert!(at.x + at.w <= 4800 + 1 && at.y + at.h <= 6300 + 1, "{sheet:?} {at:?}");
            }
        }
    }
}
