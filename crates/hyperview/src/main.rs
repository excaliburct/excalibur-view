#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use std::path::PathBuf;

/// Anything that goes badly wrong is written down next to the program and shown
/// to the user, rather than the window simply disappearing.
fn report_crashes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let note = format!(
            "Excalibur View stopped unexpectedly.\n\n{info}\n\nVersion {}\n",
            env!("CARGO_PKG_VERSION")
        );
        let beside = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("hyperview-problem.txt")));
        if let Some(path) = beside.as_ref() {
            let _ = std::fs::write(path, &note);
        }
        rfd::MessageDialog::new()
            .set_title("Excalibur View")
            .set_description(&note)
            .set_level(rfd::MessageLevel::Error)
            .show();
        default(info);
    }));
}

/// One program, three things it can be.
///
/// This is what "self-contained" means in practice. A company downloads one
/// file. On five desks it is the viewer. On the sixth it is also the server,
/// because somebody ticked a box — and the box starts a second copy of this
/// same program in service mode rather than installing anything else.
enum Mode {
    /// The ordinary one: the window, with whatever drawings were double-clicked.
    /// The flag is `--new-window`: a separate window even when one is open.
    Viewer(Vec<PathBuf>, Flags),
    /// The company's server, with no window at all. What the Windows service
    /// runs, and what a Linux box would run from systemd.
    Serve(Settings),
    /// Settings → Apps → Uninstall.
    Uninstall,
    /// `--benchmark drawing.pdf`: measure, write it down, close.
    Benchmark(PathBuf),
}

/// How the window was asked to open.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Flags {
    /// `--new-window`: a separate window even when one is open.
    new_window: bool,
    /// `--combine`: Explorer's "Combine in Excalibur View", for the files
    /// that came with it.
    combine: bool,
}

#[derive(Clone, Debug, Default)]
struct Settings {
    data: Option<PathBuf>,
    port: Option<u16>,
    name: Option<String>,
}

fn read_arguments(args: Vec<std::ffi::OsString>) -> Mode {
    let mut settings = Settings::default();
    let mut mode: Option<&str> = None;
    let mut open = Vec::new();
    let mut flags = Flags::default();
    let mut rest = args.into_iter().peekable();

    while let Some(argument) = rest.next() {
        let text = argument.to_string_lossy().to_string();
        match text.as_str() {
            "--serve" => mode = Some("serve"),
            "--uninstall" => mode = Some("uninstall"),
            "--new-window" => flags.new_window = true,
            "--combine" => flags.combine = true,
            "--benchmark" => {
                mode = Some("benchmark");
                if let Some(drawing) = rest.next() {
                    open.push(PathBuf::from(drawing));
                }
            }
            "--data" => settings.data = rest.next().map(PathBuf::from),
            "--port" => {
                settings.port = rest
                    .next()
                    .and_then(|p| p.to_string_lossy().parse::<u16>().ok())
            }
            "--name" => settings.name = rest.next().map(|n| n.to_string_lossy().to_string()),
            // Anything else is a drawing somebody double-clicked. Several of
            // them when they selected three and pressed Enter, which is why
            // this is a list.
            _ => open.push(PathBuf::from(argument)),
        }
    }

    match mode {
        Some("serve") => Mode::Serve(settings),
        Some("uninstall") => Mode::Uninstall,
        Some("benchmark") => Mode::Benchmark(open.into_iter().next().unwrap_or_default()),
        _ => Mode::Viewer(open, flags),
    }
}

fn main() -> eframe::Result {
    let mut arguments: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    // The assistant app's connector: no window, no installing, nothing on
    // standard output but answers.
    if arguments.iter().any(|a| a == "--mcp") {
        std::process::exit(hyperview::assistant::serve());
    }
    // The same check as Help → Test Assistant Connection, for a script or a
    // support call: written beside the program, printed, and shown.
    if arguments.iter().any(|a| a == "--mcp-test") {
        let report = hyperview::assistant::diagnose();
        let written = hyperview::assistant::write_check(&report);
        println!("{report}");
        if !arguments.iter().any(|a| a == "--quiet") {
            let mut shown = report.clone();
            if let Some(path) = written {
                shown.push_str(&format!("\nSaved to {}", path.display()));
            }
            rfd::MessageDialog::new()
                .set_title("Excalibur View — assistant") // WHAT: the title of the connection test's message box. WHY: says what the report is about in plain words.
                .set_description(shown)
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        let good = report.starts_with("The assistant can use Excalibur View");
        std::process::exit(if good { 0 } else { 1 });
    }
    // For somebody writing a plugin: `--plugin-check plugin.wasm [sheet.json]`
    // runs it in the sandbox once and says whether it is ready to be signed.
    // Nothing is installed.
    if let Some(at) = arguments.iter().position(|a| a == "--plugin-check") {
        if !hyperview::edition::runs_plugins_itself() {
            println!("{}", hyperview::edition::not_in_this_copy("Checking a plugin"));
            std::process::exit(1);
        }
        let files: Vec<PathBuf> = arguments[at + 1..]
            .iter()
            .filter(|a| !a.to_string_lossy().starts_with("--"))
            .map(PathBuf::from)
            .collect();
        let checkup = match files.first() {
            Some(wasm) => hyperview::plugins::checkup(wasm, files.get(1).map(PathBuf::as_path)),
            None => hyperview::plugins::Checkup {
                passed: false,
                report: "Usage: Hyperview.exe --plugin-check plugin.wasm [sheet.json]".into(),
                written: None,
            },
        };
        println!("{}", checkup.report);
        if !arguments.iter().any(|a| a == "--quiet") {
            rfd::MessageDialog::new()
                .set_title("Excalibur View — plugin check")
                .set_description(&checkup.report)
                .set_level(if checkup.passed { rfd::MessageLevel::Info } else { rfd::MessageLevel::Warning })
                .set_buttons(rfd::MessageButtons::Ok)
                .show();
        }
        std::process::exit(if checkup.passed { 0 } else { 1 });
    }
    // Started by "Restart now": the window that started it is still closing.
    let restarted = arguments.iter().any(|a| a == hyperview::install::RESTARTED);
    arguments.retain(|a| a != hyperview::install::RESTARTED);
    match read_arguments(arguments) {
        Mode::Viewer(open, flags) => {
            // Wherever it was double-clicked from, it installs itself for this
            // person and opens from there. Nothing to ask anybody.
            let mut args: Vec<std::ffi::OsString> =
                open.iter().map(|p| p.as_os_str().to_os_string()).collect();
            if flags.new_window {
                args.push("--new-window".into());
            }
            if flags.combine {
                args.push("--combine".into());
            }
            if hyperview::install::settle_in(&args) {
                return Ok(());
            }
            // One window per person: a drawing double-clicked while
            // Hyperview is open becomes a tab in it.
            let started = if restarted {
                hyperview::instance::start_when_free(&open, std::time::Duration::from_secs(30))
            } else if flags.new_window {
                hyperview::instance::Start::First
            } else {
                hyperview::instance::start(&open, flags.combine)
            };
            if started == hyperview::instance::Start::HandedOver {
                return Ok(());
            }
            // The first of Explorer's starts to arrive is the window. Its own
            // file joins the others in the inbox, so they are gathered as one.
            if flags.combine && hyperview::instance::combine_here(&open) {
                return viewer(Vec::new());
            }
            viewer(open)
        }
        Mode::Uninstall => {
            hyperview::install::uninstall();
            Ok(())
        }
        Mode::Benchmark(drawing) => {
            let _ = hyperview::bench::REQUESTED.set(drawing.clone());
            viewer(vec![drawing])
        }
        Mode::Serve(settings) => {
            serve(settings);
            Ok(())
        }
    }
}

// ---- the log ----------------------------------------------------------------

/// What the program did, written down where somebody can find it when it
/// did not do what it should: `hyperview.log` beside the installed program, in
/// the person's own app data. On Windows the window has nowhere else to say
/// it. Kept to a couple of megabytes, the one before kept as
/// `hyperview.old.log`.
fn keep_a_log() {
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("warn,hyperview=info,hub=info"),
    );
    if cfg!(windows) {
        if let Some(file) = log_file() {
            builder.target(env_logger::Target::Pipe(Box::new(file)));
        }
    }
    let _ = builder.try_init();
    log::info!("Excalibur View {} started", env!("CARGO_PKG_VERSION"));
}

fn log_file() -> Option<std::fs::File> {
    let folder = hyperview::install::home()?;
    std::fs::create_dir_all(&folder).ok()?;
    let path = folder.join("hyperview.log");
    const LARGEST: u64 = 2 * 1024 * 1024;
    if std::fs::metadata(&path).map(|m| m.len() > LARGEST).unwrap_or(false) {
        let _ = std::fs::rename(&path, folder.join("hyperview.old.log"));
    }
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

// ---- the window -----------------------------------------------------------

/// Which renderer to try first, and what to fall back to.
///
/// Two of these are built in because neither works everywhere. On Windows,
/// OpenGL is often not hardware at all — a server-class box with stock display
/// drivers, or anything over remote desktop, gets Microsoft's *software*
/// implementation, and a drawing pans like treacle. So Windows tries Direct3D
/// first. But a machine can also have no working Direct3D and no Vulkan, and
/// the honest answer there is OpenGL, slow or not.
///
/// The one thing that must not happen is what happened the first time this was
/// switched: the program refusing to start at all because the renderer it was
/// told to use did not exist on that machine. Hence a list rather than a
/// choice, and hence [`try_renderers`] below, which treats a renderer that
/// panics on start-up as a renderer to stop using rather than as the end of the
/// program.
fn renderers() -> Vec<eframe::Renderer> {
    // An escape hatch, because the person in front of the machine can see
    // things this cannot: HYPERVIEW_RENDERER=glow or =wgpu.
    match std::env::var("HYPERVIEW_RENDERER")
        .unwrap_or_default()
        .trim()
        .to_lowercase()
        .as_str()
    {
        "glow" | "opengl" | "gl" => return vec![eframe::Renderer::Glow],
        "wgpu" | "dx12" | "d3d" | "directx" | "vulkan" => return vec![eframe::Renderer::Wgpu],
        _ => {}
    }
    // Asked, not assumed. wgpu is only a renderer if a backend was compiled
    // into it, and if none was it does not return an error — it panics on the
    // first instance, before there is a window to put a message in. This is the
    // same question its own panic message tells you to ask, and asking it costs
    // nothing because the answer is settled at compile time.
    let wgpu_works = !wgpu::Instance::enabled_backend_features().is_empty();
    if !wgpu_works {
        log::warn!("this build has no wgpu backend compiled in; using OpenGL");
        return vec![eframe::Renderer::Glow];
    }
    if cfg!(windows) {
        vec![eframe::Renderer::Wgpu, eframe::Renderer::Glow]
    } else {
        vec![eframe::Renderer::Glow, eframe::Renderer::Wgpu]
    }
}

/// Which graphics adapter draws the window.
///
/// The best one there is: a graphics card, then the one built into the
/// processor, and — on a Windows Server reached over Remote Desktop, where
/// there is usually no graphics card at all — Windows' own software Direct3D.
/// That last one matters. Left to choose for itself, a program can decide
/// there is nothing suitable and fall back to OpenGL, and OpenGL over Remote
/// Desktop on a server is version 1.1 from 1997, which this window cannot use:
/// the program would not start. Software Direct3D is slower than a card and
/// perfectly usable for a drawing.
fn graphics_adapter(benchmarking: bool) -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    let mut setup = WgpuSetupCreateNew::default();
    setup.native_adapter_selector = Some(std::sync::Arc::new(|adapters, surface| {
        let rank = |adapter: &wgpu::Adapter| match adapter.get_info().device_type {
            wgpu::DeviceType::DiscreteGpu => 0,
            wgpu::DeviceType::IntegratedGpu => 1,
            wgpu::DeviceType::VirtualGpu => 2,
            wgpu::DeviceType::Other => 3,
            wgpu::DeviceType::Cpu => 4,
        };
        adapters
            .iter()
            .filter(|a| surface.is_none_or(|s| a.is_surface_supported(s)))
            .min_by_key(|a| rank(a))
            .cloned()
            .ok_or_else(|| "no graphics adapter can draw into this window".to_string())
    }));
    WgpuConfiguration {
        wgpu_setup: WgpuSetup::CreateNew(setup),
        present_mode: if benchmarking {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::AutoVsync
        },
        // One frame queued, not the default two or three: a sheet being
        // dragged follows the mouse instead of trailing a frame or two behind
        // it, which is most of what "laggy" feels like.
        desired_maximum_frame_latency: Some(1),
        ..Default::default()
    }
}

/// Starts the window with the first renderer that works.
///
/// A renderer that is missing on a machine does not politely return an error —
/// wgpu panics with "no wgpu backend feature ... was enabled", and a driver that
/// is present but broken can panic in its own ways further in. So the attempt
/// is caught, and a failure moves to the next renderer instead of showing
/// somebody a crash box for a program that would have run perfectly.
///
/// Only the *start-up* attempt is caught. Once a window is up and the person is
/// working, a panic is a real fault and goes to the crash handler, which writes
/// it down where it can be sent on. Silently restarting a program somebody has
/// unsaved markups in would be worse than telling them.
fn try_renderers(
    open: Vec<PathBuf>,
    build: impl Fn(Vec<PathBuf>, eframe::Renderer) -> eframe::Result,
) -> eframe::Result {
    let choices = renderers();
    let mut last: Option<eframe::Error> = None;

    for (n, renderer) in choices.iter().enumerate() {
        let first_try = n == 0;
        let more_to_try = n + 1 < choices.len();

        // The crash box belongs to a program that is running, not to one that
        // is still deciding how to draw. While there is another renderer to
        // try, a panic here is information, not something to interrupt anybody
        // with.
        let hook = more_to_try.then(|| {
            let quiet = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            quiet
        });

        let open_now = open.clone();
        let renderer = *renderer;
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build(open_now, renderer)));

        if let Some(quiet) = hook {
            std::panic::set_hook(quiet);
        }

        match outcome {
            Ok(Ok(())) => return Ok(()),
            Ok(Err(e)) => {
                log::warn!("{renderer:?} would not start: {e}");
                last = Some(e);
            }
            Err(_) => {
                log::warn!("{renderer:?} panicked on start-up; trying the next renderer");
            }
        }
        if first_try && more_to_try {
            log::info!("falling back to {:?}", choices[n + 1]);
        }
    }

    // Everything was tried. Say which machine problem this actually is, rather
    // than handing somebody a panic message about a graphics crate.
    let note = "Excalibur View could not start its display.\n\n\
                Neither Direct3D nor OpenGL would start on this computer. That usually \
                means the display driver needs updating, or this is a remote session with \
                no graphics available.\n\n\
                Setting HYPERVIEW_RENDERER=glow before starting forces the OpenGL path, \
                which sometimes works where the other does not.";
    rfd::MessageDialog::new()
        .set_title("Excalibur View")
        .set_description(note)
        .set_level(rfd::MessageLevel::Error)
        .show();
    match last {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

fn viewer(open: Vec<PathBuf>) -> eframe::Result {
    keep_a_log();
    report_crashes();

    let benchmarking = hyperview::bench::REQUESTED.get().is_some();
    // The icon is scan-filled from the same vector the program draws on screen,
    // so the taskbar and the About box can never show two different marks. It
    // is made inside the closure because a second attempt needs its own.
    try_renderers(open, move |open, renderer| {
        let icon = egui::IconData {
            rgba: ui::mark::raster(256),
            width: 256,
            height: 256,
        };
        let options = eframe::NativeOptions {
            renderer,
            // Say out loud that a real graphics card is wanted, so a machine
            // with one does not get handed a software context by default.
            hardware_acceleration: eframe::HardwareAcceleration::Preferred,
            wgpu_options: graphics_adapter(benchmarking),
            // Measured without waiting for the screen, so the numbers are the
            // program's and not the monitor's.
            vsync: !benchmarking,
            viewport: egui::ViewportBuilder::default()
                .with_title(concat!("Excalibur View ", env!("CARGO_PKG_VERSION")))
                .with_app_id("excalibur-hyperview")
                .with_icon(icon)
                .with_inner_size([1500.0, 950.0])
                .with_min_inner_size([900.0, 600.0]),
            ..Default::default()
        };
        eframe::run_native(
            "Excalibur View",
            options,
            Box::new(|cc| {
                egui_extras::install_image_loaders(&cc.egui_ctx);
                Ok(Box::new(hyperview::app::App::new(cc, open)))
            }),
        )
    })
}

// ---- the server -----------------------------------------------------------

fn serve(settings: Settings) {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "hyperview_server=info,tower_http=warn".into()),
        )
        .init();

    let mut config = hyperview_server::Config::from_environment();
    if let Some(data) = settings.data {
        config.data = data;
    } else {
        config.data = hyperview::host::default_data_folder();
    }
    if let Some(port) = settings.port {
        config.listen = std::net::SocketAddr::from(([0, 0, 0, 0], port));
    }
    if let Some(name) = settings.name.filter(|n| !n.trim().is_empty()) {
        config.name = name;
    }

    // Deliberately *not* making an administrator and printing a password. A
    // server started this way was started by the program, and the person who
    // started it is sitting in front of Hyperview about to fill in their own
    // details. Making one here would claim the server out from under them.
    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("Could not start: {e}");
            std::process::exit(1);
        }
    };
    let served = runtime.block_on(hyperview_server::run::serve(
        config,
        hyperview_server::run::on_a_signal(),
        |serving| {
            println!("{} is serving at {}", serving.name, serving.url());
        },
    ));
    if let Err(e) = served {
        eprintln!("{e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<std::ffi::OsString> {
        list.iter().map(std::ffi::OsString::from).collect()
    }

    #[test]
    fn started_with_nothing_it_is_the_viewer() {
        assert!(matches!(read_arguments(args(&[])), Mode::Viewer(open, flags) if open.is_empty() && flags == Flags::default()));
    }

    #[test]
    fn a_drawing_double_clicked_in_explorer_opens_in_the_viewer() {
        match read_arguments(args(&["Structural.pdf"])) {
            Mode::Viewer(open, _) => {
                assert_eq!(open.len(), 1);
                assert_eq!(open[0], PathBuf::from("Structural.pdf"));
            }
            _ => panic!("that is a drawing, not an instruction"),
        }
    }

    #[test]
    fn three_drawings_selected_at_once_all_open() {
        match read_arguments(args(&["a.pdf", "b.pdf", "c.pdf"])) {
            Mode::Viewer(open, _) => assert_eq!(open.len(), 3),
            _ => panic!("still the viewer"),
        }
    }

    #[test]
    fn the_same_program_is_the_server_when_it_is_asked_to_be() {
        match read_arguments(args(&[
            "--serve",
            "--data",
            "somewhere/server",
            "--port",
            "8714",
            "--name",
            "Example Steel",
        ])) {
            Mode::Serve(settings) => {
                assert_eq!(settings.port, Some(8714));
                assert_eq!(settings.name.as_deref(), Some("Example Steel"));
                assert!(settings.data.unwrap().ends_with("server"));
            }
            _ => panic!("that is the server"),
        }
    }

    #[test]
    fn a_company_name_with_a_space_in_it_survives_being_a_service_argument() {
        // Because "Example Steel, Inc." is a name and "Example" is not.
        match read_arguments(args(&["--serve", "--name", "Example Steel, Inc."])) {
            Mode::Serve(settings) => assert_eq!(settings.name.as_deref(), Some("Example Steel, Inc.")),
            _ => panic!("the server"),
        }
    }

    #[test]
    fn a_port_that_is_not_a_port_does_not_stop_it_starting() {
        match read_arguments(args(&["--serve", "--port", "eight thousand"])) {
            // None, so the default is used, rather than refusing to run.
            Mode::Serve(settings) => assert_eq!(settings.port, None),
            _ => panic!("the server"),
        }
    }

    #[test]
    fn new_window_is_a_flag_not_a_drawing() {
        match read_arguments(args(&["--new-window", "S-101.pdf"])) {
            Mode::Viewer(open, flags) if flags.new_window && !flags.combine => {
                assert_eq!(open, vec![PathBuf::from("S-101.pdf")])
            }
            _ => panic!("a separate window with one drawing in it"),
        }
    }

    #[test]
    fn combine_from_explorer_is_a_flag_and_the_file_is_still_the_file() {
        match read_arguments(args(&["--combine", "S-102.pdf"])) {
            Mode::Viewer(open, flags) if flags.combine && !flags.new_window => {
                assert_eq!(open, vec![PathBuf::from("S-102.pdf")])
            }
            _ => panic!("one file, to be combined"),
        }
    }

    #[test]
    fn settings_apps_uninstall_is_its_own_mode_and_never_opens_a_drawing() {
        assert!(matches!(read_arguments(args(&["--uninstall"])), Mode::Uninstall));
    }
}
