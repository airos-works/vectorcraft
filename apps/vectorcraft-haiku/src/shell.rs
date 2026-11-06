//! The interface thread's event loop: Haiku's input goes to the window it happened in
//! (`viewports`, which gives every egui viewport a window of its own: the floating panels', the
//! documents' and the dialogs' windows), VectorcraftApp runs its frame in the root viewport's
//! pass as the desktop app's `eframe::App` does (`raw_input_hook`, then `logic` and `ui`), and
//! egui's output goes back to the windows and the system.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{ViewportCommand, ViewportId};
use vectorcraft_engine::Session;
use vectorcraft_engine::file_access::{self, AutomationRoots};
use vectorcraft_ui_egui::stylus::{PenSample, StylusFeed};
use vectorcraft_ui_egui::{UiState, VectorcraftApp};

use crate::host::{self, RawEvent, kind};
use crate::platform::Haiku;
use crate::services::{self, LOG};
use crate::viewports::{Viewports, WindowInput};
use crate::{control_server, prefs, prefs_dir};

pub struct Shell {
    app: VectorcraftApp,
    ctx: egui::Context,
    viewports: Viewports<Haiku>,
    /// The main window (the root viewport's), with the menu bar.
    root: i32,
    /// The pen tablet's samples, the app's stylus feed too.
    stylus: StylusFeed,
    /// The pen's last sample, while the last mouse event was a pen's.
    pen: Option<PenSample>,
    /// Native menu clicks for its backend (`native_menu.rs`).
    menu_clicks: crate::native_menu::Clicks,
    /// The interface scale (the system font against 12 pt) and device pixels per logical pixel
    /// ([`host::device_scale`]), and when the latter was last asked.
    ui_scale: f32,
    dev: f32,
    dev_checked: Instant,
    /// Documents Tracker or a later launch handed over, opened once no file panel is up.
    to_open: Vec<String>,
    /// Bring the main window forward at the next frame (documents arrived).
    activate: bool,
    /// The adapter the windows were last drawn with (`None`: the software rasteriser).
    adapter: Option<String>,
    quit: bool,
}

impl Shell {
    /// The app, as the desktop app's `main` sets it up, in the main window `root`.
    pub fn start(
        ctx: egui::Context,
        root: i32,
        ui_scale: f32,
        saved: Option<UiState>,
        files: Vec<String>,
        control_port: Option<u16>,
        roots: Option<Arc<AutomationRoots>>,
    ) -> Result<Self, String> {
        let dev = host::device_scale();
        let viewports = Viewports::new(&ctx, Haiku::new()?, root, ui_scale * dev, dev);
        VectorcraftApp::install_ime_reset_hooks(&ctx);
        let mut session = Session::new();
        // The profiles loaded before come back before the preferences name any of them.
        prefs_dir::attach_profile_store(&mut session);
        prefs_dir::attach_plugin_store(&mut session);
        let stylus = StylusFeed::on_ui_thread();
        let mut app = VectorcraftApp::new(session, services::native(&ctx, stylus.clone()));
        // A package: SoftwareUpdater updates it (craft-update has no air/OS release to offer).
        app.set_startup_update_check(false);
        prefs::load_prefs(&mut app, saved);
        prefs_dir::attach_pdf_preset_store(&mut app.session);
        // The user's libraries live next to the preferences, as in the desktop app.
        let beside = |name: &str| prefs::prefs_path().and_then(|p| Some(p.parent()?.join(name).to_string_lossy().to_string()));
        app.session.swatch_libraries.set_user_dir(beside("Swatches"));
        app.session.style_libraries.set_user_dir(beside("Graphic Styles"));
        app.session.symbol_libraries.set_user_dir(beside("Symbols"));
        app.session.brush_libraries.set_user_dir(beside("Brushes"));
        vectorcraft_ui_egui::panels::library_panel::restore(&mut app);
        if prefs::prefs_enabled() {
            app.session.libraries.set_dir(beside("Libraries"));
        }
        if std::env::var_os("VECTORCRAFT_NO_PREFS").is_none() {
            app.session.recovery.set_default_folder(beside("Data Recovery"));
        }
        // The window has the system's title bar.
        app.custom_titlebar = false;
        let adapter = viewports.platform(|p| p.adapter());
        app.graphics_adapter = Some(adapter.clone().unwrap_or_else(|| "software rasteriser (CPU)".into()));
        log::info!(target: LOG, "rendering with {}", app.graphics_adapter.as_deref().unwrap_or_default());
        if let Some(port) = control_port {
            let rx = control_server::start(port, ctx.clone());
            app = app.with_control(rx).with_automation_roots(roots.clone());
            if let Some(roots) = &roots {
                file_access::confine_process(roots.clone());
                log::info!(target: LOG, "control channel confined to the automation roots: {}", roots.describe());
            }
        }
        let menu_clicks = crate::native_menu::Clicks::default();
        if std::env::var("VECTORCRAFT_NATIVE_MENU").as_deref() != Ok("0") {
            let backend = crate::native_menu::HaikuMenu::new(crate::native_menu::HostSink, menu_clicks.clone());
            app.services.native_menu = Some(vectorcraft_ui_egui::native_menu::NativeMenu::new(Box::new(backend)));
        }
        file_access::unconfined(|| crate::open_files(&mut app, files));
        Ok(Self {
            app,
            ctx,
            viewports,
            root,
            stylus,
            pen: None,
            menu_clicks,
            ui_scale,
            dev,
            dev_checked: Instant::now(),
            to_open: Vec::new(),
            activate: false,
            adapter,
            quit: false,
        })
    }

    /// Runs until the app quits.
    pub fn run(&mut self) {
        while !self.quit {
            host::wait(self.viewports.timeout());
            self.drain();
            if self.dev_checked.elapsed() > Duration::from_secs(2) {
                self.check_density();
            }
            for viewport in self.viewports.take_due() {
                if viewport == ViewportId::ROOT {
                    self.frame();
                } else if self.viewports.run_deferred(viewport).close_root {
                    self.quit = true;
                }
            }
            self.viewports.chores();
        }
    }

    /// Asks for the desktop's density again (the monitors may have changed).
    fn check_density(&mut self) {
        self.dev_checked = Instant::now();
        let dev = host::device_scale();
        if (dev - self.dev).abs() > f32::EPSILON {
            log::info!(target: LOG, "display density {} -> {dev}", self.dev);
            self.dev = dev;
            self.viewports.set_density(self.ui_scale * dev, dev);
        }
    }

    /// A pen tablet's sample with a mouse event goes to the stylus feed before the pointer move
    /// it belongs to (see `tablet.rs`), as AppKit's event monitor feeds it in the desktop app:
    /// each move keeps its own pressure and tilt.
    fn tablet(&mut self, ev: &RawEvent) {
        let raw = crate::tablet::Raw {
            tablet: ev.tablet,
            pressure: ev.pressure,
            tilt: (ev.has_tilt != 0).then_some((ev.tilt_x, ev.tilt_y)),
            eraser: ev.eraser != 0,
            subtype: ev.subtype,
        };
        match crate::tablet::update(raw) {
            crate::tablet::Update::Pen(sample) => {
                self.pen = Some(sample);
                if ev.kind == kind::MOUSE_MOVED {
                    self.stylus.set_move(sample);
                } else {
                    self.stylus.set_sample(Some(sample));
                }
            }
            crate::tablet::Update::Keep => {
                // The HID add-on's button messages carry no sample: a press from hover touches
                // at the pen's lightest pressure.
                if ev.kind == kind::MOUSE_DOWN
                    && let Some(touch) = crate::tablet::contact(self.pen)
                {
                    self.pen = Some(touch);
                    self.stylus.set_sample(Some(touch));
                }
            }
            crate::tablet::Update::Mouse => {
                if self.pen.take().is_some() {
                    self.stylus.set_sample(None);
                }
            }
        }
    }

    /// Hands the queued native events to their windows; the application's own are handled here.
    fn drain(&mut self) {
        while let Some((ev, text)) = host::next_event() {
            if matches!(ev.kind, kind::MOUSE_MOVED | kind::MOUSE_DOWN | kind::MOUSE_UP) {
                self.tablet(&ev);
            }
            if let Some(input) = window_input(&ev, &text) {
                if !self.viewports.input(ev.window, input) {
                    log::debug!(target: LOG, "event {} for window {}, which is gone", ev.kind, ev.window);
                }
                continue;
            }
            match ev.kind {
                // Quit (Command+Q, the Deskbar's Quit) closes the main window, which asks about
                // unsaved documents first, as the desktop app's window close does.
                kind::QUIT_REQUESTED => {
                    self.viewports.input(self.root, WindowInput::Close);
                }
                // Tracker's Open, Open With and drops on the icon, and the documents of a later
                // launch (one copy runs: B_SINGLE_LAUNCH): they open as documents, as the desktop
                // app opens a later launch's files.
                kind::OPEN_FILES => {
                    self.to_open.extend(text.lines().filter(|l| !l.is_empty()).map(str::to_string));
                    self.activate = true;
                    self.viewports.request(ViewportId::ROOT, Duration::ZERO);
                }
                kind::MENU => {
                    self.menu_clicks.borrow_mut().push(ev.id);
                    self.viewports.request(ViewportId::ROOT, Duration::ZERO);
                }
                kind::SCREEN_PICK => {
                    services::pick_answer(ev.id, ev.x != 0.0, ev.buttons as u32);
                    self.viewports.request(ViewportId::ROOT, Duration::ZERO);
                }
                kind::DIALOG_ANSWER => {
                    services::answer(ev.id, ev.x != 0.0, &text);
                    self.viewports.request(ViewportId::ROOT, Duration::ZERO);
                }
                kind::COLORS => {
                    self.check_density();
                    self.viewports.request(ViewportId::ROOT, Duration::ZERO);
                }
                _ => {}
            }
        }
    }

    /// The windows went back to the software rasteriser (the GPU device was lost or failed):
    /// the documents stay open, and Help › System Info says what draws now.
    fn check_gpu(&mut self) {
        let adapter = self.viewports.platform(|p| p.adapter());
        if adapter == self.adapter {
            return;
        }
        if self.adapter.is_some() && adapter.is_none() {
            self.app.status("The graphics device failed: VectorCraft draws with the processor now; your documents are as they were");
        }
        self.adapter = adapter;
        self.app.graphics_adapter = Some(self.adapter.clone().unwrap_or_else(|| "software rasteriser (CPU)".into()));
    }

    /// The desktop app's `App::logic`: documents handed over, the app's tick, and quitting.
    fn logic(&mut self, ctx: &egui::Context) {
        // Files from Tracker or later launches wait while a file panel is open: what asked for it
        // runs again on its document when it answers.
        if !self.app.file_dialog_open() && !self.to_open.is_empty() {
            let files = std::mem::take(&mut self.to_open);
            file_access::unconfined(|| crate::open_files(&mut self.app, files));
        }
        if std::mem::take(&mut self.activate) {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(ViewportCommand::Focus);
        }
        file_access::unconfined(|| self.app.tick(ctx));
        self.app.finish_ime_logic(ctx);
        track_window(ctx, &mut self.app.ui.window);
        if self.app.ui.status == "quit" {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    /// The root viewport's pass: the app's frame, which draws every other window's viewport on
    /// the way.
    fn frame(&mut self) {
        self.check_gpu();
        let Some(mut raw) = self.viewports.root_input() else { return };
        file_access::unconfined(|| self.app.raw_input_hook(&self.ctx, &mut raw));
        let closing = raw.viewport().close_requested();
        let ctx = self.ctx.clone();
        let t0 = Instant::now();
        let out = ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            self.logic(&ctx);
            file_access::unconfined(|| self.app.ui(ui));
            // Preferences are written when confirmed, and the UI settings every few seconds when
            // they changed, as the desktop app does (#1747, #4625).
            if self.app.take_prefs_changed() {
                file_access::unconfined(|| prefs::save_prefs(&self.app));
            } else {
                file_access::unconfined(|| prefs::save_prefs_soon(&self.app));
            }
        });
        let t1 = Instant::now();
        // The main window's close button, Close or Quit: the app asks about unsaved documents
        // and cancels the close while it does (`CancelClose`), as eframe honours.
        let cancelled = out.viewport_output.get(&ViewportId::ROOT).is_some_and(|vp| vp.commands.iter().any(|c| matches!(c, ViewportCommand::CancelClose)));
        if closing && !cancelled {
            self.quit = true;
        }
        if self.viewports.finish(ViewportId::ROOT, out).close_root {
            self.quit = true;
        }
        log::debug!(target: LOG, "frame: interface {:.1} ms, output and drawing {:.1} ms", (t1 - t0).as_secs_f64() * 1e3, t1.elapsed().as_secs_f64() * 1e3);
    }

    /// What quitting does besides closing the windows (`on_exit` in the desktop app).
    pub fn finish(&mut self) {
        vectorcraft_ui_egui::background::wait_all(&mut self.app);
        file_access::unconfined(|| prefs::save_prefs(&self.app));
    }
}

/// The main window's size and maximized state for the preferences (`UiState::window`), as the
/// desktop app's `window::track` keeps them: in points at 100 % interface zoom. Haiku centres the
/// window when it opens, so no position is kept.
fn track_window(ctx: &egui::Context, geometry: &mut Option<vectorcraft_ui_egui::state::WindowGeometry>) {
    let zoom = ctx.zoom_factor();
    ctx.input(|i| {
        let v = i.viewport();
        if v.minimized == Some(true) || v.fullscreen == Some(true) {
            return;
        }
        if v.maximized == Some(true) {
            if let Some(g) = geometry {
                g.maximized = true;
            }
            return;
        }
        let Some(inner) = v.inner_rect else { return };
        *geometry = Some(vectorcraft_ui_egui::state::WindowGeometry { pos: None, size: [inner.width() * zoom, inner.height() * zoom], maximized: false });
    });
}

/// A native event that happened in a window, as that window's input.
fn window_input<'a>(ev: &RawEvent, text: &'a str) -> Option<WindowInput<'a>> {
    Some(match ev.kind {
        kind::MOUSE_MOVED | kind::MOUSE_DOWN | kind::MOUSE_UP => WindowInput::Pointer { x: ev.x, y: ev.y, buttons: ev.buttons, modifiers: ev.modifiers },
        kind::POINTER_GONE => WindowInput::PointerGone,
        // A wheel's whole steps, or (`repeat` 1) a touchpad's scroll gesture with its unrounded
        // steps and its phase in `clicks` (`host.cpp`).
        kind::WHEEL => WindowInput::Wheel { dx: ev.dx, dy: ev.dy, modifiers: ev.modifiers, gesture: (ev.repeat != 0).then_some(ev.clicks) },
        kind::KEY_DOWN | kind::KEY_UP => {
            WindowInput::Key { pressed: ev.kind == kind::KEY_DOWN, code: ev.key, raw_char: ev.raw_char, modifiers: ev.modifiers, text }
        }
        kind::MODIFIERS => WindowInput::Modifiers(host::modifiers()),
        kind::FOCUS => WindowInput::Focus(ev.x != 0.0),
        kind::CLOSE => WindowInput::Close,
        // Files dropped on a window go to the interface as the desktop app's do: on the canvas
        // they are placed, elsewhere opened.
        kind::DROPPED_FILES => WindowInput::Dropped { x: ev.x, y: ev.y, paths: text.lines().filter(|l| !l.is_empty()).map(PathBuf::from).collect() },
        // The next frame picks up the new size, position or state.
        kind::RESIZED | kind::MOVED | kind::MINIMIZED => WindowInput::Changed,
        // An input method's composition: `id` says what, the pre-edit's selected clause is
        // bytes `key` to `raw_char` (-1: none).
        kind::IME => match ev.id {
            host::ime::PREEDIT => WindowInput::Preedit { text, selection: (ev.key >= 0 && ev.raw_char >= 0).then_some((ev.key, ev.raw_char)) },
            host::ime::COMMIT => WindowInput::Commit(text),
            host::ime::STOPPED => WindowInput::ImeStopped,
            _ => return None,
        },
        _ => return None,
    })
}
