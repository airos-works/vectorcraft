//! The interface's platform services (`vectorcraft_ui_egui::Services`) the Haiku way: file panels
//! (`BFilePanel`, answered asynchronously as the desktop app's dialogs off the UI thread are),
//! files typed for Tracker after they are written, the clipboard with all its flavours
//! (`BClipboard`), Tracker for Show in Folder and Edit Original, the print kit for File › Print
//! (`print.rs`), the native screen colour picker and the pen tablet's stylus feed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use vectorcraft_engine::cmd::clipboard::{BITMAP, Flavour, PNG, SVG, TEXT};
use vectorcraft_engine::cmd::fileio;
use vectorcraft_ui_egui::picks::{PickAnswer, PickRequest};
use vectorcraft_ui_egui::print::{PrintJob, PrintService, Printer, SharedPrintFn};
use vectorcraft_ui_egui::screen_color::Answer;
use vectorcraft_ui_egui::stylus::StylusFeed;
use vectorcraft_ui_egui::{FilePick, Services, SystemClipboard};

use crate::{host, print};

pub const SIGNATURE: &str = "application/x-vnd.storytold-VectorCraft";
pub const LOG: &str = "vectorcraft_haiku";

/// The document types VectorCraft claims when nothing else opens them (Tracker's preferred
/// application), as the post-install script does for the package.
const DOCUMENT_TYPES: &[&str] =
    &["application/x-vectorcraft", "application/x-vectorcraft-template", "application/illustrator", "image/x-eps", "application/x-affinity"];

pub fn claim_document_types() {
    let claimed = host::claim_types(SIGNATURE, DOCUMENT_TYPES);
    if claimed > 0 {
        log::info!(target: LOG, "preferred application of {claimed} document type(s)");
    }
}

/// air/OS slows down past about four threads doing heavy work at once (the scheduler and the
/// kernel's locks): the canvas rasterizer's workers and rayon's pool stay at four.
pub fn limit_threads() {
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    // The renderer leaves a core free: five usable cores make four workers.
    vectorcraft_render::set_usable_cores(Some(cores.min(5)));
    let workers = cores.clamp(1, 4);
    if let Err(e) = rayon::ThreadPoolBuilder::new().num_threads(workers).thread_name(|i| format!("vectorcraft worker {i}")).build_global() {
        log::debug!(target: LOG, "worker pool: {e}");
    }
}

pub fn clipboard_text() -> Option<String> {
    host::clipboard_get("text/plain").map(|v| String::from_utf8_lossy(&v).trim_end_matches('\0').to_string()).filter(|t| !t.is_empty())
}

// ----------------------------------------------------------------------------- files

/// A file VectorCraft wrote gets its MIME type (`BEOS:TYPE`) from its name and contents, which
/// Tracker opens it by; files written with plain file calls have none.
fn typed(path: &str, result: Result<(), String>) -> Result<(), String> {
    if result.is_ok() && !host::type_file(Path::new(path)) {
        log::debug!(target: LOG, "couldn't set the type of {path}");
    }
    result
}

fn read_file(path: &str) -> Result<Vec<u8>, String> {
    if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err(format!("{path}: not a file"));
    }
    std::fs::read(path).map_err(|e| e.to_string())
}

fn read_file_head(path: &str, max: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;
    if !std::fs::metadata(path).map_err(|e| e.to_string())?.is_file() {
        return Err(format!("{path}: not a file"));
    }
    let mut out = vec![];
    std::fs::File::open(path).and_then(|f| f.take(max as u64).read_to_end(&mut out)).map_err(|e| e.to_string())?;
    Ok(out)
}

fn write_file(path: &str, bytes: &[u8]) -> Result<(), String> {
    typed(path, fileio::write_atomic(Path::new(path), bytes).map_err(|e| e.to_string()))
}

/// The file panels in view, by id: where their answer goes.
static PANELS: Mutex<Option<HashMap<i32, (Sender<PickAnswer>, egui::Context)>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);

fn next_id() -> i32 {
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// The extensions a dialog's file types offer, for the Open panel's filter (`*`: any file).
fn extensions(filters: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out: Vec<String> = filters.iter().flat_map(|(_, e)| e.iter().cloned()).collect();
    out.sort();
    out.dedup();
    if out.is_empty() || out.iter().any(|e| e == "*") { vec!["*".into()] } else { out }
}

/// `Services::start_pick`: a Be file panel, which runs in its own window thread; its answer comes
/// back as a `DIALOG_ANSWER` event ([`answer`]) and what asked for it runs again with the path.
fn start_pick(request: PickRequest, ctx: &egui::Context) -> Option<Receiver<PickAnswer>> {
    let mut panels = PANELS.lock().ok()?;
    let panels = panels.get_or_insert_default();
    if !panels.is_empty() {
        // One panel at a time, as the desktop's modal dialogs.
        return None;
    }
    let t = vectorcraft_ui_egui::i18n::t;
    let (mode, name, exts, title) = match &request {
        PickRequest::Open(pick) => (0, pick.folder.clone().unwrap_or_default(), extensions(&pick.filters), t("Open")),
        PickRequest::OpenMany => {
            let filters: Vec<(String, Vec<String>)> = FilePick::filters(fileio::place_filters());
            (3, String::new(), extensions(&filters), t("Place"))
        }
        PickRequest::Save(pick) => {
            let name = match &pick.folder {
                Some(folder) => {
                    let _ = std::fs::create_dir_all(folder);
                    Path::new(folder).join(&pick.name).to_string_lossy().into_owned()
                }
                None => pick.name.clone(),
            };
            (1, name, Vec::new(), t("Save"))
        }
        PickRequest::Folder => (2, String::new(), Vec::new(), t("Choose")),
    };
    let id = next_id();
    let (tx, rx) = channel();
    panels.insert(id, (tx, ctx.clone()));
    host::file_panel(id, mode, &name, &exts.iter().map(String::as_str).collect::<Vec<_>>(), &format!("VectorCraft: {title}"));
    Some(rx)
}

/// A file panel's answer (`picked` false: cancelled), the paths one per line.
pub fn answer(id: i32, picked: bool, text: &str) {
    let Some((tx, ctx)) = PANELS.lock().ok().and_then(|mut p| p.as_mut()?.remove(&id)) else { return };
    let paths: Vec<String> = if picked { text.lines().filter(|l| !l.is_empty()).map(str::to_owned).collect() } else { Vec::new() };
    let _ = tx.send(paths);
    ctx.request_repaint();
}

/// A dialog shown in line (outside an entry that can run again): the panel can't block the
/// interface thread, whose events it would need, so nothing is picked and the log says so.
fn pick_now(request: &PickRequest) -> Option<String> {
    log::warn!(target: LOG, "no file panel shown in line for {request:?}: panels answer asynchronously here");
    None
}

// ----------------------------------------------------------------------------- clipboard

/// The system clipboard with every flavour (`BClipboard`): each one a `B_MIME_TYPE` field of its
/// MIME type in the clipboard's message, as Haiku applications exchange them. A picture copied
/// by a Haiku program as an archived `BBitmap` (`image/bitmap`) is read as PNG (`host.cpp`).
#[derive(Default)]
struct Clipboard {
    /// The clipboard's write count right after our last write.
    ours: Option<u64>,
}

/// The largest picture read off the clipboard, per side.
const MAX_SIDE: u32 = 32_768;

/// A bitmap of another type (JPEG, BMP, TIFF…) as the PNG the paste takes.
fn as_png(data: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        return None;
    }
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
}

fn is_bitmap(format: &str) -> bool {
    format == "image/bitmap" || (format.starts_with("image/") && format != SVG && format != "image/svg+xml-compressed")
}

impl SystemClipboard for Clipboard {
    fn write(&mut self, flavours: &[Flavour]) -> Result<(), String> {
        let list: Vec<(&str, &[u8])> = flavours.iter().map(|f| (f.mime, f.data.as_slice())).collect();
        if !host::clipboard_set_many(&list) {
            return Err("the clipboard couldn't be written".into());
        }
        self.ours = Some(host::clipboard_count());
        Ok(())
    }

    fn holds_ours(&mut self) -> bool {
        self.ours.is_some_and(|n| n == host::clipboard_count())
    }

    fn read(&mut self, mimes: &[&'static str]) -> Option<Flavour> {
        let formats = host::clipboard_formats();
        for &mime in mimes {
            if mime == BITMAP {
                // PNG first (a native bitmap is converted to it), then any other picture.
                if let Some(data) = formats.iter().any(|f| is_bitmap(f)).then(|| host::clipboard_get(PNG)).flatten() {
                    return Some(Flavour { mime: PNG, data });
                }
                for f in formats.iter().filter(|f| is_bitmap(f) && f.as_str() != PNG && f.as_str() != "image/bitmap") {
                    if let Some(data) = host::clipboard_get(f).and_then(|d| as_png(&d)) {
                        return Some(Flavour { mime: PNG, data });
                    }
                }
                continue;
            }
            if !formats.iter().any(|f| f == mime) {
                continue;
            }
            let Some(mut data) = host::clipboard_get(mime) else { continue };
            if mime == TEXT {
                // Haiku programs sometimes copy text with its terminating NUL.
                while data.last() == Some(&0) {
                    data.pop();
                }
                if data.is_empty() {
                    continue;
                }
            }
            return Some(Flavour { mime, data });
        }
        None
    }

    fn has(&mut self, mimes: &[&'static str]) -> bool {
        let formats = host::clipboard_formats();
        mimes.iter().any(|&m| if m == BITMAP { formats.iter().any(|f| is_bitmap(f)) } else { formats.iter().any(|f| f == m) })
    }

    fn change_count(&mut self) -> Option<u64> {
        Some(host::clipboard_count())
    }
}

// ----------------------------------------------------------------------------- applications

/// PhotoCraft for Edit Original, where it is installed.
fn find_photocraft() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map_or_else(|| std::path::PathBuf::from("/boot/home"), std::path::PathBuf::from);
    [
        std::path::PathBuf::from("/boot/system/apps/PhotoCraft"),
        std::path::PathBuf::from("/boot/system/non-packaged/apps/PhotoCraft"),
        home.join("config/apps/PhotoCraft"),
        home.join("config/non-packaged/apps/PhotoCraft"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}

fn open_in_photocraft(path: &str) -> Result<bool, String> {
    let Some(app) = find_photocraft() else { return Ok(false) };
    // One PhotoCraft runs (B_SINGLE_LAUNCH): a running one takes the file and this launch ends.
    std::process::Command::new(&app).arg(path).spawn().map(|_| true).map_err(|e| format!("can't open {path} in {}: {e}", app.display()))
}

// ----------------------------------------------------------------------------- printing

/// File › Print through the Be print kit: the engine's print-ready PDF (every page and copy in
/// it) rasterised by hayro at the printer's resolution and spooled with `BPrintJob` (`print.rs`).
struct PrintKit;

fn print_job(printer: Option<&str>, title: &str, pdf: &[u8]) -> Result<String, String> {
    let options = print::Options {
        title: title.to_string(),
        // The copies are in the PDF already.
        copies: 1,
        // Artwork prints in colour: libprint's drivers start in monochrome, and their job setup's
        // choice (Shift+Print) stays once made.
        color: true,
        landscape: None,
        setup: print::shift_down() || std::env::var_os("VECTORCRAFT_PRINT_SETUP").is_some_and(|v| v != "0"),
        ..print::Options::default()
    };
    let store = print::Store::new(crate::prefs::prefs_path().and_then(|p| Some(p.parent()?.join("printers"))));
    let printer = printer.map(str::to_string).or_else(|| print::printers().into_iter().find(|p| p.default).map(|p| p.name));
    let started = std::time::Instant::now();
    let result = print::print_pdf(printer.as_deref(), pdf.to_vec(), &options, &store).map_err(|e| e.to_string());
    match &result {
        Ok(receipt) => log::info!(target: LOG, "printed on {printer:?} in {:.1} s: {receipt}", started.elapsed().as_secs_f32()),
        Err(e) => log::warn!(target: LOG, "printing on {printer:?}: {e}"),
    }
    let to = printer.unwrap_or_else(|| "the default printer".into());
    result.map(|receipt| format!("Sent “{title}” to {to} ({receipt})"))
}

impl PrintService for PrintKit {
    fn printers(&mut self) -> Vec<Printer> {
        print::printers().into_iter().map(|p| Printer { name: p.name, default: p.default }).collect()
    }

    fn print(&mut self, job: &PrintJob) -> Result<String, String> {
        print_job(job.printer, job.title, job.pdf)
    }

    fn shared_print(&self) -> Option<SharedPrintFn> {
        Some(Arc::new(|job: &PrintJob<'_>| print_job(job.printer, job.title, job.pdf)))
    }

    fn has_setup(&self) -> bool {
        true
    }

    /// The Printers preferences, where printers are added and set up.
    fn setup(&mut self, _printer: Option<&str>) -> Result<(), String> {
        std::process::Command::new("/boot/system/preferences/Printers").spawn().map(|_| ()).map_err(|e| format!("can't open the Printers preferences: {e}"))
    }
}

// ----------------------------------------------------------------------------- screen colour

/// The screen colour pick in progress: its id and where its answer goes.
static PICK: Mutex<Option<(i32, Sender<Answer>, egui::Context)>> = Mutex::new(None);

/// `Services::screen_color`: the native picker (`PickerWindow` in host.cpp) over a snapshot of
/// every monitor, with a loupe; a click answers the pixel's colour, Escape cancels.
fn screen_sampler(ctx: &egui::Context) -> Result<Receiver<Answer>, String> {
    let (tx, rx) = channel();
    let id = next_id();
    let t = vectorcraft_ui_egui::i18n::t;
    if !host::screen_pick(id, t("Click to pick · Esc to cancel"), t("Scroll to zoom")) {
        return Err("the screen couldn't be read".into());
    }
    if let Ok(mut pick) = PICK.lock() {
        *pick = Some((id, tx, ctx.clone()));
    }
    Ok(rx)
}

/// The picker's answer: the pixel `rgb` (0xRRGGBB, the screen's sRGB) or nothing.
pub fn pick_answer(id: i32, picked: bool, rgb: u32) {
    let Some((_, tx, ctx)) = PICK.lock().ok().and_then(|mut p| p.take_if(|p| p.0 == id)) else { return };
    let [_, r, g, b] = rgb.to_be_bytes();
    let _ = tx.send(Ok(picked.then(|| [r, g, b].map(|c| f32::from(c) / 255.0))));
    ctx.request_repaint();
}

// ----------------------------------------------------------------------------- all of them

pub fn native(ctx: &egui::Context, stylus: StylusFeed) -> Services {
    let (pick_ctx, sample_ctx) = (ctx.clone(), ctx.clone());
    Services {
        pick_open: Some(Box::new(|pick: &FilePick| pick_now(&PickRequest::Open(pick.clone())))),
        pick_open_multi: Some(Box::new(|| pick_now(&PickRequest::OpenMany).into_iter().collect())),
        pick_save: Some(Box::new(|pick: &FilePick| pick_now(&PickRequest::Save(pick.clone())))),
        pick_folder: Some(Box::new(|| pick_now(&PickRequest::Folder))),
        start_pick: Some(Box::new(move |request: PickRequest| start_pick(request, &pick_ctx))),
        read: Some(Box::new(read_file)),
        read_head: Some(Box::new(read_file_head)),
        write: Some(Box::new(write_file)),
        exists: Some(Box::new(|p: &str| Path::new(p).exists())),
        write_shared: Some(Arc::new(write_file)),
        stream_shared: Some(Arc::new(|p: &str, fill: &mut dyn FnMut(&mut dyn std::io::Write) -> std::io::Result<()>| {
            typed(p, fileio::write_file_with(p, fill).map_err(|e| e.to_string()))
        })),
        clipboard_read: Some(Box::new(clipboard_text)),
        system_clipboard: Some(Box::new(Clipboard::default())),
        open_url: Some(Box::new(|url: &str| {
            if !host::open_url(url) {
                log::warn!(target: LOG, "couldn't open {url}");
            }
        })),
        reveal: Some(Box::new(|path: &str| host::reveal(path).then_some(()).ok_or_else(|| format!("can't show {path} in Tracker")))),
        open_file: Some(Box::new(|path: &str| host::open_file(path).then_some(()).ok_or_else(|| format!("can't open {path}")))),
        open_in_photocraft: Some(Box::new(open_in_photocraft)),
        screen_color: Some(Box::new(move || screen_sampler(&sample_ctx))),
        print: Some(Box::new(PrintKit)),
        stylus: Some(stylus),
        ..Default::default()
    }
}
