# VectorCraft on air/OS

This independent fork of VectorCraft 0.21.0 (upstream `36ef34dfd`) runs the full Rust app on
air/OS / Haiku (x86_64 and arm64). The document engine, the canvas renderer, every tool, panel,
dialog, file format, Data Recovery, the preferences and the control channel are upstream's,
unchanged; a native host (`apps/vectorcraft-haiku`) takes the place of eframe and winit, which
don't run on Haiku. It is built the way the other Craft apps' air/OS ports are (PhotoCraft's and
WordCraft's hosts are its templates), and lives in Deskbar's Media folder.

## What the host does

- **Windows.** A `BApplication` and a `BDirectWindow` per egui viewport (`native/host.cpp`,
  `src/viewports.rs`, `src/platform.rs`): the main window, and the floating panels, documents'
  extra windows and dialogs upstream opens in OS windows of their own. The interface runs on a
  thread of its own and drives `VectorcraftApp` as the desktop app's `eframe::App` does
  (`raw_input_hook`, then `tick` and `ui` in the root viewport's pass; `src/shell.rs`).
- **Rendering.** egui is drawn with wgpu on Vulkan (`src/gpu.rs`) into a presenter's frame
  texture (`src/present.rs`). `VECTORCRAFT_PRESENT` picks how the frame reaches the window:
  `readback` (the default: only the 64-pixel tiles that changed are read back and drawn into the
  window's `BBitmap`, `src/damage.rs`), `full`, or `direct` (experimental: the frame is copied
  into the screen's frame buffer imported with `VkImportScanoutMemoryHAIKU`, `src/scanout.rs`).
  `VECTORCRAFT_RENDERER=auto|cpu|vulkan` chooses the renderer: `auto` (the default) uses a
  discrete or integrated Vulkan adapter and falls back to the software rasteriser
  (`src/raster.rs`) when there is none (software Vulkan is never chosen); `vulkan` fails without
  one; `cpu` never starts Vulkan. A device lost or failing while VectorCraft runs switches every
  window to the rasteriser at the next frame; the documents stay open and the status bar says so.
  Help › System Info names the adapter in use.
- **What runs on the GPU, and what doesn't.** Upstream's canvas is rasterised on the processor
  (vello_cpu, `crates/render`): on every desktop the GPU only draws the interface and composites
  the canvas image it is given, so here too the GPU draws egui (panels, the canvas texture,
  overlays) and nothing else. VectorCraft has no GPU effects, compositor or export path to hand the
  device to (`apps/vectorcraft/src/compositor_gpu.rs` only asks a Wayland compositor which GPU it
  draws with, to pick the adapter). The canvas rasteriser's worker threads are capped at four
  (`VECTORCRAFT_RENDER_THREADS` still overrides it, as does Preferences › Performance), and so is
  rayon's pool: air/OS gets slower past about four threads doing heavy work at once.
- **Menus.** A native `BMenuBar` (`src/native_menu.rs`) shows upstream's menus, which it hands
  over as a Mac menu bar: About goes to the end of Help, Settings (the Preferences pages) to Edit,
  Language and Appearance to Window, Quit to File; Hide, Services and Check for Updates are left
  out. Keys stay with the interface (the window gives Command keys to the canvas before
  `BWindow` looks for menu shortcuts), so the menus only show them. `VECTORCRAFT_NATIVE_MENU=0`
  keeps the in-window menus.
- **Files.** Open, Save, Place, Relink and folder pickers are `BFilePanel`s, answered
  asynchronously (upstream's `Services::start_pick`: what asked runs again with the path). Files
  VectorCraft writes get their MIME type for Tracker. Tracker's Open/Open With and drops on the
  icon, and later launches from Terminal, go to the running copy (`B_SINGLE_LAUNCH`), which opens
  them as documents. VectorCraft becomes the preferred application of its own document types
  (`.vectorcraft`, `.vctemplate`) and of `.ai`/`.eps`/`.afdesign` when nothing else claims them
  (`resources/vectorcraft-post-install.sh`, and at each start). Show in Folder selects the file in
  Tracker; Edit Original opens a file with its preferred application, or in PhotoCraft where that
  is installed.
- **Clipboard.** Copy puts every flavour upstream makes (text, SVG, PDF, PNG) in one `BClipboard`
  message under their MIME types; Paste takes SVG, PDF, EPS, text and pictures other programs
  copied, including a Haiku `image/bitmap` (ShowImage, screenshots), read as PNG. The clipboard's
  system count tells whether the last Copy is still ours.
- **Printing.** File › Print goes to the Be print kit (`src/print.rs`, `native/print.cpp`):
  upstream's print-ready PDF (every page and copy in it) is rasterised by hayro at the printer's
  resolution (at most 300 dpi) and spooled with `BPrintJob`; the printer list is the print
  server's. Shift held while printing (or `VECTORCRAFT_PRINT_SETUP=1`) opens the driver's page and
  job setup; the answers are kept per printer in the settings folder's `printers`. Setup… opens the
  Printers preferences.
- **Pen tablets.** Haiku's tablet input add-ons put pressure, tilt and the eraser end in mouse
  messages (`be:tablet_*`); the host feeds them to the canvas's stylus feed with each move
  (`src/tablet.rs`), as the macOS reader does, so pressure, tilt and the eraser end work as on
  the other desktops. `tools/haiku/tablet-probe.cpp` sends a stroke as the add-ons would.
- **Input methods.** The canvas view is `B_INPUT_METHOD_AWARE`: compositions reach the Type tool
  as egui's IME pre-edit and commit, the candidate window follows the caret, and the Type tool's
  IME resets are answered.
- **Fonts.** Haiku's font folders (`/boot/system/data/fonts`, the non-packaged one and the user's
  `~/config/data/fonts`, `~/config/non-packaged/data/fonts`) are scanned like the other
  desktops' (`crates/text/src/fontdb/dirs.rs`), and the release embeds craft-fonts.
- **Screen colours.** Sample Color from Screen covers every monitor with a snapshot under a loupe
  (`PickerWindow` in host.cpp); a click picks, Escape cancels.
- **Settings** live in `~/config/settings/VectorCraft` (`ui.json`, logs, Data Recovery, the
  user's libraries, Fonts), as `apps/vectorcraft/src/profile.rs` now says for Haiku;
  `VECTORCRAFT_CONFIG_DIR` and `VECTORCRAFT_NO_PREFS` work as on the other desktops. Data Recovery's
  area lock uses `flock` on Haiku, where the standard library has no file locks.
- **Updates.** The package is updated by SoftwareUpdater; Help › Check for Updates is left out of
  the menu bar and the command answers that SoftwareUpdater updates VectorCraft. The community links
  stay.

Not done: the system's colour management (Haiku keeps no monitor profiles; the canvas is sRGB),
pinch zoom (air/OS touchpads have no pinch gesture), and the Media Kit (VectorCraft plays and
records nothing).

## Build and package

```sh
JOBS=24 bash tools/haiku/build.sh
bash tools/haiku/package.sh --no-build
```

The scripts cross-compile the tier-3 Rust target `x86_64-unknown-haiku` with its standard
library (`-Zbuild-std`) and the C++ Be API boundary. `tools/haiku/env.sh` supplies the defaults
for `RUSTUP_HOME`, `CARGO_HOME`, `HAIKU_SYSROOT`, `HAIKU_CROSS` and `CARGO_TARGET_DIR`. The release
embeds craft-fonts: `build.sh` takes `CRAFT_FONTS_DIR` (default `../craft-fonts`; check out the
commit upstream's workflows pin, `b076417`), `CRAFT_FONTS_DIR=` builds without. air/OS CI builds
the x86_64 and arm64 packages with airos-ci's `build_vectorcraft` (`.github/workflows/airos.yml`).

`vendor/` holds wgpu 30.0.1 with its Vulkan backend enabled for Haiku (`vendor/README.md`); the
host prints with craft-libs' hayro, matching the hayro-interpret upstream already uses.

## Testing

`VectorCraft --control PORT --automation-read-root DIR --automation-write-root DIR` starts the
control channel as on the other desktops (`docs/control-protocol.md`); a `--control` launch needs
a copy of its own (one copy runs per executable file). `RUST_LOG=warn,vectorcraft_haiku=debug`
logs each frame's timings.

## Hardware results

### 0.21.0 on the X399 — 7 November 2026

NVIDIA GTX 1070 with the system's NVK (Mesa 25.3.0-devel), 3840×2160 at 200 %, a test copy with
its resources (`rc` + `xres`), settings in a test `VECTORCRAFT_CONFIG_DIR`:

- Started on Vulkan ("NVIDIA GeForce GTX 1070 (NVK GP104-A)", changed-tile readback) and drew the
  start screen, then a document, with the native menu bar.
- Through the control channel: opened an SVG (system font "Noto Sans" for its text), drew a
  rectangle, exported a PNG and saved a `.vectorcraft` (both typed for Tracker), copied (the SVG
  code reached the system clipboard) and pasted.
- A `tablet-probe` stroke (the HID add-on's messages) drew with the Blob Brush.
- Layers floated into a native window of its own (`window.panel.float`).
- A second launch with a relative path from Terminal handed the file to the running copy, which
  opened it as a tab, and ended.
- `VECTORCRAFT_RENDERER=cpu` drew everything with the software rasteriser; Data Recovery wrote its
  copies under its `flock`ed `.lock` after a minute.
- Quit (`hey VectorCraft quit`) asked about each unsaved document; Don't Save quit. Quitting with
  nothing unsaved quit at once. No crash and nothing in the syslog.
- File › Print to the "PS-File" printer (PostScript driver to a local `nc`): the driver's Paper
  setup opened once (answered with `tools/haiku/window-probe.cpp`'s `enter`) and the job spooled
  one sheet of PostScript in 9 s.

Not tried: `VECTORCRAFT_PRESENT=direct`, a lost device at run time, a real tablet and an input
method (x399 has neither).
