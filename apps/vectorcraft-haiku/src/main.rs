//! VectorCraft for air/OS (and Haiku).
//!
//! The same interface as the desktop app (`vectorcraft-ui-egui`), hosted natively: a
//! `BApplication` and a `BWindow` per egui viewport from the Be API (`native/host.cpp`,
//! `viewports`, `platform`) take the input and show the frames, which egui's output becomes with
//! Vulkan (`gpu`, shown by `present`) or, without a hardware Vulkan adapter, through a software
//! rasteriser (`raster`), with the system's menu bar, file panels, clipboard, cursors, file
//! types, printers, pen tablets and locale. eframe and winit don't run on Haiku; nothing below the
//! interface changes.
//!
//! Usage: `VectorCraft [--control PORT [--automation-read-root DIR]… [--automation-write-root DIR]…] [FILE…]`,
//! as the desktop app.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
// Elsewhere only the platform-neutral parts build (and are tested).
#![cfg_attr(not(target_os = "haiku"), allow(dead_code))]

mod damage;
mod keys;
mod native_menu;
mod print;
mod raster;
mod tablet;
mod viewports;

#[cfg(target_os = "haiku")]
#[path = "../../vectorcraft/src/automation_args.rs"]
mod automation_args;
#[cfg(target_os = "haiku")]
#[path = "../../vectorcraft/src/control_server.rs"]
mod control_server;
#[cfg(target_os = "haiku")]
mod gpu;
#[cfg(target_os = "haiku")]
mod host;
#[cfg(target_os = "haiku")]
#[path = "../../vectorcraft/src/logging.rs"]
mod logging;
#[cfg(target_os = "haiku")]
mod platform;
// The desktop app's preferences file (`ui.json`) and the folders beside it, shared by path: the
// same merge of concurrent saves, the same `VECTORCRAFT_CONFIG_DIR` and `VECTORCRAFT_NO_PREFS`.
#[cfg(target_os = "haiku")]
#[path = "../../vectorcraft/src/prefs.rs"]
mod prefs;
#[cfg(target_os = "haiku")]
#[path = "../../vectorcraft/src/prefs_dir.rs"]
mod prefs_dir;
#[cfg(target_os = "haiku")]
mod present;
#[cfg(target_os = "haiku")]
mod scanout;
#[cfg(target_os = "haiku")]
mod services;
#[cfg(target_os = "haiku")]
mod shell;

// What the shared `prefs.rs` reaches through `super::`.
#[cfg(target_os = "haiku")]
use vectorcraft_engine::cmd::fileio;
#[cfg(target_os = "haiku")]
use vectorcraft_ui_egui::VectorcraftApp;

#[cfg(not(target_os = "haiku"))]
fn main() {
    let _ = std::io::Write::write_all(&mut std::io::stderr(), b"VectorCraft for air/OS runs on Haiku; elsewhere run `vectorcraft`.\n");
    std::process::exit(1);
}

#[cfg(target_os = "haiku")]
fn main() -> std::process::ExitCode {
    match native_main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            log::error!(target: services::LOG, "{e}");
            let _ = std::io::Write::write_all(&mut std::io::stderr(), format!("VectorCraft: {e}\n").as_bytes());
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "haiku")]
const USAGE: &str = "\
Usage: VectorCraft [options] [files…]

Opens VectorCraft for air/OS with the given files. While VectorCraft runs, the files open in it.

Options:
  --control <port>               Start the localhost JSON-lines control server on <port>
                                 (or VECTORCRAFT_CONTROL_PORT)
  --automation-read-root <dir>   Confine the files control requests read to <dir>
                                 (may be given more than once)
  --automation-write-root <dir>  Confine the files control requests write to <dir>
                                 (may be given more than once)
  --version                      Print the version and exit
  -h, --help                     Print this help and exit

Environment: VECTORCRAFT_RENDERER=auto|cpu|vulkan, VECTORCRAFT_PRESENT=readback|full|direct,
VECTORCRAFT_NATIVE_MENU=0 (in-window menus), VECTORCRAFT_CONFIG_DIR, VECTORCRAFT_NO_PREFS.
";

/// The arguments as the desktop app takes them.
#[cfg(target_os = "haiku")]
struct Args {
    files: Vec<String>,
    control_port: Option<u16>,
    read_roots: Vec<std::path::PathBuf>,
    write_roots: Vec<std::path::PathBuf>,
}

/// `Ok(None)`: `--version` or `--help` answered.
#[cfg(target_os = "haiku")]
fn parse_args() -> Result<Option<Args>, String> {
    let mut control = automation_args::ControlArg::default();
    let root_env = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(std::path::PathBuf::from);
    let mut roots = automation_args::AutomationArgs::new(root_env("VECTORCRAFT_AUTOMATION_READ_ROOT"), root_env("VECTORCRAFT_AUTOMATION_WRITE_ROOT"));
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if roots.consume(&a, &mut args).and_then(|root| if root { Ok(true) } else { control.consume(&a, &mut args) })? {
            continue;
        }
        match a.as_str() {
            "--version" => {
                let sha = option_env!("VECTORCRAFT_BUILD_SHA").map(|s| format!(" ({})", &s[..s.len().min(12)])).unwrap_or_default();
                let _ = std::io::Write::write_all(&mut std::io::stdout(), format!("vectorcraft {}{sha} for air/OS\n", env!("CARGO_PKG_VERSION")).as_bytes());
                return Ok(None);
            }
            "--help" | "-h" => {
                let _ = std::io::Write::write_all(&mut std::io::stdout(), USAGE.as_bytes());
                return Ok(None);
            }
            // Options the desktop app takes that mean nothing here.
            "--new-instance" | "--in-window-menus" => {}
            _ if a.starts_with("--") => return Err(format!("unknown option {a} (see --help)")),
            _ => files.push(a),
        }
    }
    let control_port = match control.port {
        Some(port) => Some(port),
        None => automation_args::env_control_port(std::env::var_os("VECTORCRAFT_CONTROL_PORT"))?,
    };
    let automation_args::AutomationArgs { read_roots, write_roots, .. } = roots;
    Ok(Some(Args { files, control_port, read_roots, write_roots }))
}

#[cfg(target_os = "haiku")]
fn native_main() -> Result<(), String> {
    use vectorcraft_engine::file_access::AutomationRoots;

    host::export_locale();
    let Some(args) = parse_args()? else { return Ok(()) };
    // Before the log file, the preferences or the recovery records are touched: a second launch
    // (from Terminal; Tracker hands documents to the running copy itself) passes its documents to
    // the running copy and ends, as the desktop app's `single_instance` does.
    if host::init(services::SIGNATURE)? == host::Launch::HandedOver {
        if args.control_port.is_some() {
            return Err("VectorCraft is already running from this file; --control needs a copy of its own (run it from another file)".into());
        }
        return Ok(());
    }
    let logger = logging::install();
    if let Some(logger) = logger {
        match prefs::log_dir().filter(|_| prefs::prefs_enabled()) {
            Some(dir) => match logger.attach_dir(&dir) {
                Ok(path) => log::info!(target: services::LOG, "VectorCraft {} for air/OS, log file {}", env!("CARGO_PKG_VERSION"), path.display()),
                Err(e) => log::warn!(target: services::LOG, "no log file: {e}"),
            },
            None => logger.no_file(),
        }
    }
    let roots = match args.control_port {
        None => None,
        Some(_) => {
            let read: Vec<&std::path::Path> = args.read_roots.iter().map(std::path::PathBuf::as_path).collect();
            let write: Vec<&std::path::Path> = args.write_roots.iter().map(std::path::PathBuf::as_path).collect();
            AutomationRoots::new(&read, &write).map_err(|e| e.to_string())?
        }
    };
    services::claim_document_types();
    services::limit_threads();
    vectorcraft_ui_egui::i18n::detect_system_lang_in_background();
    // VectorCraft's own Fonts folder, which text.addFontFiles copies fonts into.
    prefs_dir::install_fonts();
    let saved = prefs::read_prefs();
    let scale = host::ui_scale();
    // The size saved with the preferences (points), or the desktop app's first size; the system
    // centres the window on the active monitor.
    let size = saved.as_ref().and_then(|ui| ui.window).map_or([1440.0, 900.0], |g| g.size).map(|v| if v.is_finite() { v.clamp(400.0, 16000.0) } else { 800.0 } * scale);
    // The main window opens before the interface starts, so it shows at once.
    let root = host::open_window(&host::WindowSpec {
        title: "VectorCraft".into(),
        position: None,
        size,
        min_size: Some([640.0 * scale, 420.0 * scale]),
        max_size: None,
        decorations: true,
        resizable: true,
        visible: true,
        zoomed: saved.as_ref().and_then(|ui| ui.window).is_some_and(|g| g.maximized),
        fullscreen: false,
        floating: false,
        buttons: [true; 3],
    })
    .ok_or("couldn't open the window")?;
    let worker = std::thread::Builder::new()
        .name("VectorCraft interface".into())
        .stack_size(32 << 20)
        .spawn(move || {
            let result = vectorcraft_engine::guard::catch_panic(|| {
                let ctx = egui::Context::default();
                shell::Shell::start(ctx, root, scale, saved, args.files, args.control_port, roots).map(|mut shell| {
                    shell.run();
                    shell.finish();
                })
            })
            .and_then(|r| r);
            if let Err(e) = &result {
                log::error!(target: services::LOG, "{e}");
                host::alert("VectorCraft", &format!("VectorCraft stopped: {e}"));
            }
            host::quit();
            result
        })
        .map_err(|e| e.to_string())?;
    host::run();
    worker.join().map_err(|_| "the interface thread stopped unexpectedly".to_string())?
}

/// Open files handed to the app (the command line, Tracker, later launches) as documents. A file
/// that can't be opened is reported to the user and in the log; the others still open.
#[cfg(target_os = "haiku")]
pub(crate) fn open_files(app: &mut VectorcraftApp, files: Vec<String>) {
    for f in files {
        if let Err(e) = vectorcraft_ui_egui::io::open_reporting(app, &f) {
            log::warn!(target: services::LOG, "{f}: {e}");
        }
    }
}
