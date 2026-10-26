//! Application root: the lifecycle state machine, the single command-dispatch
//! path and the eframe event loop.
//!
//! One shared machine, identical on both OSes: `Idle` (no visible window —
//! the app is resident in the macOS menu bar / Windows tray), `Bar` (floating
//! capture bar), `Capturing` (selection in progress) and `Viewer` (tabbed
//! session window). `Bar` and `Viewer` are strictly exclusive. Every user
//! action is a [`Command`] — from bar buttons, the resident-icon menu, the
//! app menus, the viewer, or later global hotkeys — queued and dispatched at
//! the top of whichever viewport pass runs.
//!
//! Viewport topology: egui/winit never repaints a window that was never
//! shown, so the root viewport is a permanent 1×1 transparent "anchor"
//! window (`taskbar(false)`, no decorations) that keeps the pump alive.
//! The real windows — bar, viewer, settings — are deferred child viewports
//! registered each frame while their state requires them.
//!
//! Launch shows no window (`Idle`): the user picks "Take a capture" or "Open
//! PixMe" from the resident-icon menu. Closing the bar opens the viewer when
//! the session has captures, else returns to `Idle`. Closing the viewer
//! returns to `Idle` (session kept). Quitting only happens through the menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use rust_i18n::t;

use crate::config::Settings;
use crate::editor::Document;
use crate::i18n::{self, Language};
use crate::ui::capture_overlay::OverlaySession;
use crate::ui::toolbar::Toolbar;
use crate::ui::viewer::Viewer;
use crate::{platform, ui};

/// The lifecycle states (one shared machine, both OSes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppState {
    /// No visible window — the app is resident in the menu bar / tray.
    #[default]
    Idle,
    /// The floating capture bar is on screen.
    Bar,
    /// A capture/selection is in progress (phase 3 fills this state in).
    Capturing,
    /// The tabbed session viewer is the visible window.
    Viewer,
}

/// Which capture the user asked for. Distinct from [`crate::capture`]'s
/// backend contract: these are the *commands* the bar, the Capture menu and
/// future hotkeys dispatch; the pipeline maps them to `xcap` calls.
#[derive(Clone, Copy, Debug)]
pub enum CaptureMode {
    /// The frontmost window.

    /// A window or object picked under the cursor.
    WindowOrObject,
    /// A user-drawn rectangle.
    Region,
    /// The whole screen.
    FullScreen,
    /// A fixed-size region that stays resizable before capture.
    FixedRegion,
    /// Capture pinned to the screen as an autonomous floating window.
    Pin,
}

/// Every user action flows through this enum and [`Shared::dispatch`].
#[derive(Clone, Debug)]
pub enum Command {
    /// Start a capture (all modes) — applies the configured delay first.
    Capture(CaptureMode),
    /// Internal: the delay elapsed, run the capture for real.
    CaptureNow(CaptureMode),
    /// The capture overlay confirmed its selection.
    ConfirmCapture,
    /// The capture overlay was cancelled (Esc / window close).
    CancelCapture,
    /// Show the floating bar (icon menu, app menu, viewer's "+").
    ShowBar,
    /// Show the viewer (icon menu's "Open PixMe").
    OpenViewer,
    /// The bar's X was clicked: viewer if the session has captures, else `Idle`.
    CloseBar,
    /// The viewer window's close was requested.
    ViewerClosed,
    /// Open the settings window.
    OpenSettings,
    /// The settings window was closed.
    CloseSettings,
    /// Application quit (menu only — never a window-close side effect).
    Quit,
    /// Create a capture document from the clipboard.
    ImportClipboard,
    /// Open an image file into the viewer.
    OpenFile,
    /// File picker pre-opened in the last used folder (Ouvrir ▾ menu).
    OpenLastDir,
    /// A blank white document (Ouvrir ▾ → Nouveau).
    NewDocument,
    /// Save the active capture.
    SaveActive,
    /// Save all captures.
    SaveAll,
    /// Rename the active capture.
    RenameActive,
    /// Copy the active document to the clipboard.
    CopyActive,
    /// Open the Draw (Dessiner) editor on the active document.
    OpenDraw,
    /// Revert the newest pixel mutation of the active document.
    UndoActive,
    /// Re-apply the newest reverted mutation.
    RedoActive,
    /// Print the active document on the OS default printer.
    PrintActive,
    /// Unsaved-session confirmation: save then close.
    ConfirmSave,
    /// Unsaved-session confirmation: discard then close.
    ConfirmDiscard,
    /// Unsaved-session confirmation: cancel.
    ConfirmCancel,
    /// Close a pinned capture window.
    ClosePin(u32),
    /// Copy a pinned capture's image to the clipboard.
    CopyPin(u32),
    /// Save a pinned capture via a file dialog.
    SavePin(u32),
}

/// `egui::ViewportId` of the child windows (`from_hash_of` is not a const fn
/// in egui 0.36, hence the functions).
fn bar_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.bar")
}
fn viewer_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.viewer")
}
fn overlay_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.capture_overlay")
}
/// One id per pinned window — autonomous floating captures.
fn pin_id(id: u32) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("pixme.pin", id))
}
fn save_dialog_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.save_dialog")
}
/// The Draw (Dessiner) editor window.
fn draw_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.draw")
}

/// Lock helper: recover a poisoned mutex rather than crash the UI thread.
pub fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// State shared between the anchor pass and the deferred-viewport callbacks
/// (they are `'static` and cannot borrow the app).
pub struct Shared {
    /// Current lifecycle state.
    pub state: Mutex<AppState>,
    /// Persisted user settings.
    pub settings: Mutex<Settings>,
    /// Viewer session state (tabs, dirty flag, confirmation).
    pub viewer: Mutex<Viewer>,
    /// The floating toolbar's transient state.
    pub toolbar: Mutex<Toolbar>,
    /// Last on-screen position of the bar viewport (restored on reopen).
    pub bar_pos: Mutex<Option<egui::Pos2>>,
    /// Whether the settings window is open.
    pub settings_open: AtomicBool,
    /// The live capture session while in `Capturing` (`None` otherwise).
    pub capture: Mutex<Option<OverlaySession>>,
    /// The last confirmed fixed-region rect, in monitor-logical space:
    /// the next fixed capture opens at the same size and position.
    pub last_fixed_region: Mutex<Option<egui::Rect>>,
    /// The last confirmed capture rect of ANY mode (monitor-logical
    /// space): the `Pin` capture's selection opens there.
    pub last_capture_rect: Mutex<Option<egui::Rect>>,
    /// Pinned captures — autonomous always-on-top windows, each backed
    /// by a deferred viewport. They outlive every state transition and
    /// die with the app.
    pub pins: Mutex<Vec<crate::ui::pin::PinWindow>>,
    /// Id source for pinned windows.
    next_pin_id: Mutex<u32>,
    /// The pin save-as window, open while Some.
    pub save_dialog: Mutex<Option<crate::ui::save_dialog::SaveDialog>>,
    /// The Draw editor session, open while Some — modal to the viewer.
    pub draw: Mutex<Option<crate::ui::draw::DrawSession>>,
    /// The session database — documents, annotations and undo stacks
    /// written at quit, read back on the first viewer open.
    pub store: Mutex<crate::store::Store>,
    /// Viewports that must grab focus once they exist and are visible
    /// (`Focus` is a no-op on a not-yet-visible window, so the child callback
    /// re-sends it until focused; the counter caps retries).
    pub pending_focus: Mutex<Vec<(egui::ViewportId, u8)>>,
    /// Commands waiting for dispatch (UI widgets push; OS menus queue into
    /// `platform`'s static, drained alongside).
    pending: Mutex<Vec<Command>>,
    /// Language applied at last check — detects changes made in the
    /// settings window so menus and `t!` can be refreshed.
    last_language: Mutex<Language>,
}

impl Shared {
    /// Queues a command for dispatch on the next pump.
    pub fn push(&self, command: Command) {
        lock(&self.pending).push(command);
    }

    /// Settings guard for UI code that edits preferences.
    pub fn lock_settings(&self) -> MutexGuard<'_, Settings> {
        lock(&self.settings)
    }

    /// Whether the OS shows a native application menu bar (macOS) or the
    /// viewer must draw its own menu strip (Windows).
    pub fn has_native_app_menu(&self) -> bool {
        platform::current().has_native_app_menu()
    }

    /// A modal child viewport is up (save-as, settings) — other windows
    /// dim themselves and refuse input while one is open.
    pub fn modal_child_open(&self) -> bool {
        lock(&self.save_dialog).is_some()
            || self.settings_open.load(Ordering::Relaxed)
            || lock(&self.draw).is_some()
    }

    /// The concrete theme for the current appearance setting and OS theme.
    pub fn appearance_kind(&self, ctx: &egui::Context) -> ui::theme::ThemeKind {
        lock(&self.settings).appearance.kind(ctx.system_theme())
    }

    /// Drains OS menu events plus queued commands, dispatches them, then
    /// reapplies language side effects if the setting changed. Called at the
    /// top of every viewport pass so any visible window keeps dispatch alive.
    pub fn pump(&self, ctx: &egui::Context) {
        let commands: Vec<Command> = platform::take_pending_commands()
            .into_iter()
            .chain(std::mem::take(&mut *lock(&self.pending)))
            .collect();
        for command in &commands {
            self.dispatch(command.clone(), ctx);
        }
        if !commands.is_empty() {
            // Wake the anchor so it (de)registers the child viewports whose
            // state just changed.
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
        let language = lock(&self.settings).language;
        let mut last = lock(&self.last_language);
        if *last != language {
            *last = language;
            i18n::apply(language);
            platform::relabel_menus();
        }
    }

    /// The one dispatch path for every entry point.
    fn dispatch(&self, command: Command, ctx: &egui::Context) {
        match command {
            Command::Capture(mode) => {
                let (delay, auto_reset) = {
                    let s = lock(&self.settings);
                    (s.capture_delay_secs, s.delay_auto_reset)
                };
                if delay > 0 {
                    // Post the real capture after the delay; the ctx clone
                    // wakes the (possibly windowless) pump.
                    let wake = ctx.clone();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_secs(delay as u64));
                        platform::queue_menu_command(Command::CaptureNow(mode));
                        wake.request_repaint_of(egui::ViewportId::ROOT);
                    });
                    if auto_reset {
                        let mut s = lock(&self.settings);
                        s.capture_delay_secs = 0;
                        s.save();
                    }
                } else {
                    self.begin_capture(mode, ctx);
                }
            }
            Command::CaptureNow(mode) => self.begin_capture(mode, ctx),
            Command::ConfirmCapture => self.confirm_capture(ctx),
            Command::CancelCapture => self.cancel_capture(ctx),
            Command::ShowBar => self.enter(AppState::Bar, ctx),
            Command::OpenViewer => self.enter(AppState::Viewer, ctx),
            Command::CloseBar => {
                // Back where the user came from: the viewer when the session
                // has captures, the resident icon otherwise.
                if lock(&self.viewer).tabs.is_empty() {
                    self.enter(AppState::Idle, ctx);
                } else {
                    self.enter(AppState::Viewer, ctx);
                }
            }
            Command::ViewerClosed => {
                if lock(&self.draw).is_some() {
                    return;
                }
                // Closing the viewer only returns the resident app to
                // Idle — the session stays in memory and nothing is
                // lost, so the dirty warning is Quit-only.
                self.enter(AppState::Idle, ctx);
            }
            Command::OpenSettings => {
                self.settings_open.store(true, Ordering::Relaxed);
                // The settings are a modal of the viewer: make sure the
                // viewer exists (and is restored) before it shows.
                self.enter(AppState::Viewer, ctx);
                // The modal lives in the VIEWER's pass — repainting the
                // anchor alone would leave an idle viewer asleep and the
                // modal would wait for the next input event.
                ctx.request_repaint_of(viewer_id());
            }
            Command::CloseSettings => {
                self.settings_open.store(false, Ordering::Relaxed);
                ctx.request_repaint_of(viewer_id());
            }
            Command::Quit => {
                let mut viewer = lock(&self.viewer);
                if viewer.dirty && !self.session_persists() {
                    viewer.confirm_close = true;
                    viewer.confirm_then_quit = true;
                    drop(viewer);
                    // The confirmation lives in the viewer: surface it.
                    self.enter(AppState::Viewer, ctx);
                } else {
                    // quit_now re-locks the viewer to persist the
                    // session — this guard must be released first.
                    drop(viewer);
                    self.quit_now(ctx);
                }
            }
            Command::ConfirmSave => {
                // "Save" in the unsaved-session dialog = save-all, then close.
                self.save_all();
                self.finish_confirmation(ctx);
            }
            Command::ConfirmDiscard => self.finish_confirmation(ctx),
            Command::ConfirmCancel => {
                let mut viewer = lock(&self.viewer);
                viewer.confirm_close = false;
                viewer.confirm_then_quit = false;
            }
            Command::ClosePin(id) => {
                // Deregistering the viewport next pass destroys the window.
                lock(&self.pins).retain(|p| p.id != id);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Command::CopyPin(id) => {
                let capture = lock(&self.pins)
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| Arc::clone(&p.capture));
                if let Some(capture) = capture
                    && let Err(e) = crate::export::clipboard::copy_capture(&capture)
                {
                    tracing::warn!("pin copy failed: {e}");
                }
            }
            Command::SavePin(id) => {
                // Opens the save-as window; the dialog writes the file.
                let capture = lock(&self.pins)
                    .iter()
                    .find(|p| p.id == id)
                    .map(|p| Arc::clone(&p.capture));
                if let Some(capture) = capture {
                    let folder = lock(&self.settings).last_save_dir.clone().or_else(|| {
                        directories::UserDirs::new()
                            .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
                    });
                    if let Some(folder) = folder {
                        *lock(&self.save_dialog) =
                            Some(crate::ui::save_dialog::SaveDialog::new(capture, folder));
                        self.request_focus(save_dialog_id());
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
            }
            Command::ImportClipboard => match crate::export::clipboard::clipboard_image() {
                // Import opens a NEW tab — it never replaces the current
                // document (the functional reference's rule).
                Ok(Some(image)) => {
                    lock(&self.viewer).push_document(Document::new(synthetic_capture(image)));
                    self.enter(AppState::Viewer, ctx);
                }
                Ok(None) => tracing::info!("clipboard holds no image"),
                Err(e) => tracing::warn!("clipboard import failed: {e}"),
            },
            Command::OpenFile => self.open_files(None, ctx),
            // Same picker, pre-opened in the last used folder.
            Command::OpenLastDir => {
                let dir = lock(&self.settings)
                    .last_open_dir
                    .clone()
                    .or_else(|| lock(&self.settings).last_save_dir.clone());
                self.open_files(dir, ctx);
            }
            Command::NewDocument => {
                // Blank white canvas — the functional reference's "New".
                let image =
                    image::RgbaImage::from_pixel(800, 600, image::Rgba([255, 255, 255, 255]));
                lock(&self.viewer).push_blank_document(Document::new(synthetic_capture(image)));
                self.enter(AppState::Viewer, ctx);
            }
            Command::SaveActive => {
                // Reuses the pin save-as window — `viewer_tab` lets the app
                // mark the tab clean when the write lands.
                let target = lock(&self.viewer).save_target();
                let folder = lock(&self.settings).last_save_dir.clone().or_else(|| {
                    directories::UserDirs::new()
                        .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
                });
                if let (Some((index, title, capture)), Some(folder)) = (target, folder) {
                    let mut dialog =
                        crate::ui::save_dialog::SaveDialog::new(Arc::new(capture), folder);
                    // The tab title seeds the file name (sanitized later).
                    dialog.filename = title;
                    dialog.viewer_tab = Some(index);
                    *lock(&self.save_dialog) = Some(dialog);
                    self.request_focus(save_dialog_id());
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            Command::SaveAll => self.save_all(),
            Command::RenameActive => lock(&self.viewer).open_rename(),
            Command::CopyActive => {
                if let Some(img) = lock(&self.viewer).copy_image()
                    && let Err(e) = crate::export::clipboard::copy_image(&img)
                {
                    tracing::warn!("copy failed: {e}");
                }
            }
            Command::OpenDraw => {
                if lock(&self.draw).is_none()
                    && let Some((tab, title, doc, undo, redo)) = lock(&self.viewer).draw_parts()
                {
                    let prefs = lock(&self.settings).clone();
                    *lock(&self.draw) = Some(crate::ui::draw::DrawSession::open(
                        doc, tab, title, undo, redo, &prefs,
                    ));
                    self.request_focus(draw_id());
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            }
            Command::UndoActive => {
                if let Some(draw) = lock(&self.draw).as_mut() {
                    draw.undo_once();
                    ctx.request_repaint_of(draw_id());
                } else {
                    lock(&self.viewer).undo_active();
                }
            }
            Command::RedoActive => {
                if let Some(draw) = lock(&self.draw).as_mut() {
                    draw.redo_once();
                    ctx.request_repaint_of(draw_id());
                } else {
                    lock(&self.viewer).redo_active();
                }
            }
            Command::PrintActive => {
                if let Some(img) = lock(&self.viewer).active_image()
                    && let Err(e) = crate::export::file::print_image(&img)
                {
                    tracing::warn!("print failed: {e}");
                }
            }
        }
    }

    /// Writes every unsaved tab to the last-used folder (or ~/Pictures) as
    /// timestamped PNGs — the "Save All" / unsaved-confirmation path has no
    /// per-tab dialog, each file is unique via the index suffix.
    /// The File → Open picker (seeded with `dir` when given) — one tab per
    /// image, and the picked folder is persisted for next time.
    fn open_files(&self, dir: Option<std::path::PathBuf>, ctx: &egui::Context) {
        let mut dialog =
            rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]);
        if let Some(dir) = dir {
            dialog = dialog.set_directory(dir);
        }
        let Some(paths) = dialog.pick_files() else {
            return;
        };
        let mut opened = false;
        for path in &paths {
            match crate::export::file::load_image(path) {
                Ok(image) => {
                    let title = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "image".to_owned());
                    let mut doc = Document::new(synthetic_capture(image));
                    // The file's mtime is the best capture/save date we
                    // have for an opened image.
                    let mtime = std::fs::metadata(path)
                        .and_then(|m| m.modified())
                        .ok()
                        .map(chrono::DateTime::<chrono::Local>::from);
                    doc.meta.captured_at = mtime.unwrap_or_else(chrono::Local::now);
                    doc.meta.saved_at = mtime;
                    doc.meta.source_path = Some(path.clone());
                    lock(&self.viewer).push_named_document(
                        doc, title, // From disk = clean until edited.
                        true,
                    );
                    opened = true;
                }
                Err(e) => tracing::warn!(?path, "cannot open image: {e}"),
            }
        }
        if opened {
            if let Some(dir) = paths
                .first()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            {
                let mut s = lock(&self.settings);
                s.last_open_dir = Some(dir);
                s.save();
            }
            self.enter(AppState::Viewer, ctx);
        }
    }

    fn save_all(&self) {
        let Some(folder) = lock(&self.settings).last_save_dir.clone().or_else(|| {
            directories::UserDirs::new().and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
        }) else {
            tracing::warn!("save-all: no writable folder known");
            return;
        };
        let stamp = chrono::Local::now().format("%y%m%d_%H%M%S");
        let mut viewer = lock(&self.viewer);
        for (i, tab) in viewer.tabs.iter_mut().enumerate() {
            if tab.saved {
                continue;
            }
            let path = folder.join(format!("capture_{stamp}_{}.png", i + 1));
            match crate::export::file::save_capture_to(
                &tab.doc.base,
                &path,
                crate::export::ImageFormat::Png,
            ) {
                Ok(()) => {
                    tab.saved = true;
                    tab.doc.meta.source_path = Some(path.clone());
                    tab.doc.meta.saved_at = Some(chrono::Local::now());
                    tracing::info!(?path, "tab saved");
                }
                Err(e) => tracing::warn!(?path, "save-all failed: {e}"),
            }
        }
        viewer.recompute_dirty();
    }

    /// Moves the machine to `next` and wakes the anchor so the child
    /// viewports tracking the state are (de)registered promptly. The
    /// repaint is sent even when `next == current`: a same-state entry
    /// still replaces sub-state (e.g. a new capture session over a live
    /// one) that the anchor pass must re-evaluate.
    fn enter(&self, next: AppState, ctx: &egui::Context) {
        let changed = {
            let mut state = lock(&self.state);
            let changed = *state != next;
            *state = next;
            changed
        };
        if changed {
            tracing::debug!(?next, "lifecycle state");
            // The viewer is a real application window: bring it to the
            // front. The bar is a palette — `always_on_top`, never focus.
            if next == AppState::Viewer {
                self.request_focus(viewer_id());
            }
        }
        // Restoring runs on EVERY viewer entry (guarded to once): a
        // same-state re-entry after reset must not skip the load.
        if next == AppState::Viewer {
            self.restore_session();
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Resolves the dirty-session confirmation: ends the session and either
    /// quits (menu Quit) or returns to `Idle`.
    fn finish_confirmation(&self, ctx: &egui::Context) {
        let quit = {
            let mut viewer = lock(&self.viewer);
            let then_quit = std::mem::replace(&mut viewer.confirm_then_quit, false);
            *viewer = Viewer::default();
            then_quit
        };
        // The session ended deliberately (saved-then-closed or
        // discarded): nothing of it may come back at the next launch.
        lock(&self.store).clear();
        if quit {
            self.quit_now(ctx);
        } else {
            self.enter(AppState::Idle, ctx);
        }
    }

    /// Marks `id` for focus grabbing: the child callback re-sends `Focus`
    /// until the window reports focused (a `Focus` sent before the window is
    /// visible is silently dropped by winit).
    fn request_focus(&self, id: egui::ViewportId) {
        let mut pending = lock(&self.pending_focus);
        if !pending.iter().any(|(v, _)| *v == id) {
            pending.push((id, 0));
        }
    }

    /// Runs inside a child viewport callback: keeps sending `Focus` while the
    /// window exists but has not been focused yet (~1.5 s cap so a window the
    /// user deliberately backgrounds is not re-focused forever).
    fn maybe_focus(&self, ctx: &egui::Context, id: egui::ViewportId) {
        let mut pending = lock(&self.pending_focus);
        let Some(entry) = pending.iter_mut().find(|(v, _)| *v == id) else {
            return;
        };
        if ctx.input(|i| i.focused) || entry.1 >= 90 {
            pending.retain(|(v, _)| *v != id);
        } else {
            entry.1 += 1;
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
    }

    /// Starts `mode`: region modes spawn the selection overlay over the
    /// frozen monitor, full-screen arms an instant capture (the bar must be
    /// torn down first — one warm-up frame inside the overlay/anchor pass).
    /// `Pin` lands in a later phase.
    fn begin_capture(&self, mode: CaptureMode, ctx: &egui::Context) {
        // Chained capture (menu/hotkey while a session is live): Esc must
        // land where the ORIGINAL session came from — `Capturing` itself
        // is never a return destination. Snapshot the state first: a
        // scrutinee-held guard would live for the whole match.
        let current = *lock(&self.state);
        let return_to = match current {
            AppState::Capturing => lock(&self.capture)
                .as_ref()
                .map(|s| s.return_to)
                .unwrap_or(AppState::Idle),
            state => state,
        };
        match mode {
            CaptureMode::Region
            | CaptureMode::FixedRegion
            | CaptureMode::FullScreen
            | CaptureMode::WindowOrObject
            | CaptureMode::Pin => match crate::capture::screen::primary_monitor() {
                Ok(monitor) => {
                    // Fixed region recalls ITS memory; Pin recalls the last
                    // confirmed capture rect whatever the mode was.
                    let remembered = match mode {
                        CaptureMode::FixedRegion => *lock(&self.last_fixed_region),
                        CaptureMode::Pin => *lock(&self.last_capture_rect),
                        _ => None,
                    };
                    let mut session = OverlaySession::new(mode, return_to, remembered);
                    session.monitor = Some(monitor);
                    let instant = session.instant;
                    *lock(&self.capture) = Some(session);
                    self.enter(AppState::Capturing, ctx);
                    if !instant {
                        self.request_focus(overlay_id());
                    }
                }
                Err(err) => {
                    tracing::error!("cannot enumerate monitors: {err}");
                }
            },
        }
    }

    /// The overlay confirmed: crop the frozen frame to the selection (or take
    /// the whole monitor for `FullScreen`), open it as a viewer tab. The
    /// capture mutex is released before any window-capture work — that path
    /// hits the OS window server and must not hold app locks.
    fn confirm_capture(&self, ctx: &egui::Context) {
        let Some(mut session) = lock(&self.capture).take() else {
            return;
        };
        let capture = if session.instant {
            session.frozen.take()
        } else {
            session.take_capture()
        };
        let return_to = session.return_to;
        let Some(frame) = capture else {
            // Nothing to keep — behave like a cancel.
            self.enter(return_to, ctx);
            return;
        };
        // Fixed-region memory: the next fixed capture reopens exactly here
        // (same size, same position — monitor-logical space).
        if matches!(session.mode, CaptureMode::FixedRegion) {
            *lock(&self.last_fixed_region) = session.monitor_space_selection();
        }
        // Pin prefill memory: the confirmed rect whatever the mode — a
        // full screen counts as the whole monitor.
        let confirmed_rect = session.monitor_space_selection().or_else(|| {
            session.monitor.as_ref().map(|m| {
                egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(m.rect.width as f32, m.rect.height as f32),
                )
            })
        });
        if let Some(sel) = confirmed_rect {
            *lock(&self.last_capture_rect) = Some(sel);
        }
        // A Pin capture becomes an autonomous floating window — nothing
        // is added to the viewer.
        if matches!(session.mode, CaptureMode::Pin) {
            self.push_pin(frame, &session, ctx);
            self.enter(return_to, ctx);
            return;
        }
        let title = {
            let mut viewer = lock(&self.viewer);
            viewer.push_document(Document::new(frame))
        };
        tracing::info!(%title, "capture stored");
        self.enter(AppState::Viewer, ctx);
    }

    /// A confirmed Pin becomes an autonomous floating window at the
    /// on-screen spot its source region occupied — no viewer document.
    fn push_pin(
        &self,
        frame: crate::capture::Capture,
        session: &OverlaySession,
        ctx: &egui::Context,
    ) {
        let monitor = frame.monitor.rect;
        let origin = session
            .monitor_space_selection()
            .map(|r| egui::pos2(monitor.x as f32 + r.min.x, monitor.y as f32 + r.min.y))
            .unwrap_or(egui::pos2(monitor.x as f32 + 80.0, monitor.y as f32 + 80.0));
        let id = {
            let mut n = lock(&self.next_pin_id);
            *n += 1;
            *n
        };
        lock(&self.pins).push(crate::ui::pin::PinWindow::new(id, frame, origin));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Cancel leaves the session untouched and returns where capture began.
    fn cancel_capture(&self, ctx: &egui::Context) {
        let return_to = {
            let mut slot = lock(&self.capture);
            let return_to = slot.as_ref().map(|s| s.return_to).unwrap_or(AppState::Idle);
            *slot = None;
            return_to
        };
        self.enter(return_to, ctx);
    }

    /// First entry into the viewer: pulls the persisted session back
    /// into memory — silent and best-effort (a corrupt database was
    /// already parked as `.bak` when the store opened).
    fn restore_session(&self) {
        let needed = {
            let viewer = lock(&self.viewer);
            !viewer.session_restored() && viewer.tabs.is_empty()
        };
        if needed {
            let session = lock(&self.store).load_session();
            let sort = lock(&self.settings).tab_sort;
            lock(&self.viewer).restore_session(session, sort);
        }
    }

    /// `true` when the session is persisted to `session.db` — the
    /// dirty-session warning only makes sense when retention is `Never`
    /// (nothing survives the exit otherwise, so we ask first).
    fn session_persists(&self) -> bool {
        lock(&self.settings).db_retention != crate::config::DbRetention::Never
    }

    /// Mirrors the whole session into the store. An open Draw editor
    /// still owns its tab's document clone — committing it back first
    /// keeps a quit mid-edit from losing the work. `Never` retention
    /// skips persistence entirely.
    fn save_session(&self) {
        if lock(&self.settings).db_retention == crate::config::DbRetention::Never {
            return;
        }
        if let Some(draw) = lock(&self.draw).take() {
            lock(&self.viewer).commit_draw(draw.tab, draw.doc, draw.undo, draw.redo);
        }
        let viewer = lock(&self.viewer);
        let active = viewer.active_index().unwrap_or(0);
        let next_title = viewer.next_title();
        lock(&self.store).save_session(&viewer.tabs, active, next_title);
    }

    /// Persists the session (best-effort — a failure never blocks
    /// quitting), then closes the anchor viewport: eframe exits once
    /// the root window is gone.
    fn quit_now(&self, ctx: &egui::Context) {
        self.save_session();
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }
}

/// Wraps a file/clipboard image in a `Capture` for a viewer document:
/// 1:1 physical-to-logical pixels and a synthetic monitor record (only
/// the image and scale factor reach the canvas).
fn synthetic_capture(image: image::RgbaImage) -> crate::capture::Capture {
    use crate::capture::{MonitorInfo, PhysicalRect};
    crate::capture::Capture {
        scale_factor: 1.0,
        monitor: MonitorInfo {
            id: 0,
            name: String::new(),
            rect: PhysicalRect {
                x: 0,
                y: 0,
                width: image.width(),
                height: image.height(),
            },
            scale_factor: 1.0,
            is_primary: true,
        },
        image,
    }
}

/// Dev-hook capture mode name → enum (`PIXME_CAPTURE` / `PIXME_CAPTURE_AFTER`).
fn debug_mode(name: &str) -> Option<CaptureMode> {
    Some(match name {
        "region" => CaptureMode::Region,
        "fullscreen" => CaptureMode::FullScreen,
        "fixed" => CaptureMode::FixedRegion,
        "window" => CaptureMode::WindowOrObject,
        "pin" => CaptureMode::Pin,
        _ => return None,
    })
}

/// Root application state, created once in [`eframe::App::ui`]'s setup.
pub struct PixMeApp {
    /// Everything the deferred-viewport callbacks reach.
    shared: Arc<Shared>,
}

impl PixMeApp {
    /// Loads settings, installs fonts, image loaders, the OS UI glue (status
    /// item / tray + menus) and applies language and theme.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = Settings::load();
        let db_retention = settings.db_retention;
        i18n::apply(settings.language);
        ui::install(&cc.egui_ctx);
        ui::theme::apply(
            &cc.egui_ctx,
            settings.appearance.kind(cc.egui_ctx.system_theme()),
        );
        platform::install_ui(&cc.egui_ctx);
        // Dev/test hook: `PIXME_CAPTURE=region|fullscreen|fixed` fires that
        // capture at startup without going through the resident menu.
        let debug_capture = std::env::var("PIXME_CAPTURE")
            .ok()
            .and_then(|v| debug_mode(&v));
        let app = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(AppState::Idle),
                last_language: Mutex::new(settings.language),
                settings: Mutex::new(settings),
                viewer: Mutex::new(Viewer::default()),
                toolbar: Mutex::new(Toolbar::default()),
                bar_pos: Mutex::new(None),
                settings_open: AtomicBool::new(false),
                capture: Mutex::new(None),
                last_fixed_region: Mutex::new(None),
                last_capture_rect: Mutex::new(None),
                pins: Mutex::new(Vec::new()),
                next_pin_id: Mutex::new(0),
                save_dialog: Mutex::new(None),
                draw: Mutex::new(None),
                store: Mutex::new({
                    let mut store = crate::store::Store::open();
                    // Launch-time retention: stale captures are pruned
                    // before anything can restore them.
                    store.apply_retention(db_retention);
                    store
                }),
                pending_focus: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
            }),
        };
        // Launch lands on the viewer, not resident-idle: it restores
        // the persisted session and gives a visible landing point.
        // Any viewport left open by eframe's persisted state (e.g. the
        // capture bar showing at a previous quit) is closed explicitly.
        app.shared.enter(AppState::Viewer, &cc.egui_ctx);
        cc.egui_ctx
            .send_viewport_cmd_to(bar_id(), egui::ViewportCommand::Close);
        app.tap_debug_capture(&cc.egui_ctx, debug_capture)
    }

    /// Queues the startup capture requested by `PIXME_CAPTURE`, if any.
    /// `PIXME_CAPTURE_AFTER=<mode>:<secs>` fires one after a delay — used
    /// to reproduce captures triggered while a session is already live.
    /// The queue only stores the command: the loop must be woken too, or
    /// the pending capture sleeps forever while all windows are idle.
    fn tap_debug_capture(self, ctx: &egui::Context, mode: Option<CaptureMode>) -> Self {
        if let Some(mode) = mode {
            self.shared.push(Command::Capture(mode));
        }
        if let Some((mode, secs)) = std::env::var("PIXME_CAPTURE_AFTER").ok().and_then(|v| {
            let (m, s) = v.split_once(':')?;
            Some((debug_mode(m)?, s.parse::<u64>().ok()?))
        }) {
            let wake = ctx.clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(secs));
                platform::queue_menu_command(Command::Capture(mode));
                wake.request_repaint();
            });
        }
        self
    }
}

impl eframe::App for PixMeApp {
    /// Transparent clear: the anchor and the bar are transparent windows, and
    /// child viewports paint their whole surface anyway.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    /// The anchor pass: dispatch queued commands, then (de)register child
    /// viewports to match the state machine. Nothing is drawn — the anchor
    /// is a 1×1 transparent window.
    fn ui(&mut self, _ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = _ui.ctx().clone();
        self.shared.pump(&ctx);

        // Snapshot the state: holding the `state` guard across the match
        // would self-deadlock the main thread — arms call `enter` /
        // `confirm_capture` / `cancel_capture`, which lock `state` again
        // (a fullscreen confirm deadlocked exactly this way).
        let state = *lock(&self.shared.state);
        match state {
            AppState::Bar => {
                let shared = Arc::clone(&self.shared);
                let builder = bar_builder(*lock(&shared.bar_pos));
                ctx.show_viewport_deferred(bar_id(), builder, move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, bar_id());
                    ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                    if let Some(pos) = ctx.input(|i| i.viewport().outer_rect.map(|r| r.min)) {
                        *lock(&shared.bar_pos) = Some(pos);
                    }
                    let mut toolbar = lock(&shared.toolbar);
                    let mut settings = lock(&shared.settings);
                    toolbar.show(ui, &mut settings, &shared);
                });
            }
            AppState::Viewer => {
                let shared = Arc::clone(&self.shared);
                ctx.show_viewport_deferred(viewer_id(), viewer_builder(), move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, viewer_id());
                    ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                    lock(&shared.viewer).show(ui, &shared);
                    // Tabs closed this frame lose their stored rows —
                    // a closed capture stays closed at the next launch.
                    let closed = lock(&shared.viewer).take_closed_ids();
                    if !closed.is_empty() {
                        lock(&shared.store).delete_documents(&closed);
                    }
                });
            }
            AppState::Capturing => {
                // The freeze happens here, before any capture window exists:
                // two anchor frames let the bar tear down, then the screen is
                // captured — registering the overlay only *after* the freeze
                // keeps it out of the photograph.
                enum Step {
                    Freeze,
                    Overlay,
                    Confirm,
                    Cancel,
                    Gone,
                }
                let step = {
                    let mut slot = lock(&self.shared.capture);
                    match slot.as_mut() {
                        None => Step::Gone,
                        Some(s) if s.failed => Step::Cancel,
                        Some(s) if s.frozen.is_none() => {
                            s.warm_freeze();
                            Step::Freeze
                        }
                        Some(s) if s.instant => Step::Confirm,
                        Some(_) => Step::Overlay,
                    }
                };
                match step {
                    Step::Freeze => ctx.request_repaint(),
                    Step::Confirm => self.shared.confirm_capture(&ctx),
                    Step::Cancel => self.shared.cancel_capture(&ctx),
                    Step::Gone => self.shared.enter(AppState::Idle, &ctx),
                    Step::Overlay => {
                        let shared = Arc::clone(&self.shared);
                        let monitor = lock(&shared.capture)
                            .as_ref()
                            .and_then(|s| s.monitor.clone());
                        ctx.show_viewport_deferred(
                            overlay_id(),
                            overlay_builder(monitor.as_ref()),
                            move |ui, _class| {
                                let ctx = ui.ctx().clone();
                                shared.pump(&ctx);
                                shared.maybe_focus(&ctx, overlay_id());
                                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                                if let Some(session) = lock(&shared.capture).as_mut() {
                                    session.show(ui, &shared);
                                }
                            },
                        );
                    }
                }
            }
            // `Idle` shows nothing.
            AppState::Idle => {}
        }

        // Pinned captures: autonomous always-on-top windows living outside
        // the state machine — they survive bar/viewer/idle cycles and die
        // with the app (closing the root closes every child viewport).
        let pin_meta: Vec<(u32, egui::Pos2, egui::Vec2)> = lock(&self.shared.pins)
            .iter()
            .map(|p| (p.id, p.origin, p.logical_size()))
            .collect();
        for (id, origin, size) in pin_meta {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(pin_id(id), pin_builder(origin, size), move |ui, _class| {
                let ctx = ui.ctx().clone();
                shared.pump(&ctx);
                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    shared.push(Command::ClosePin(id));
                }
                if let Some(pin) = lock(&shared.pins).iter_mut().find(|p| p.id == id) {
                    pin.show(ui, &shared);
                }
            });
        }

        // The Draw editor: modal to the viewer, edits a document clone —
        // OK writes it (and the history stacks) back into the tab so
        // re-entry keeps selection + chronological undo; Annuler/close
        // drops the clone untouched.
        if let Some(title) = lock(&self.shared.draw).as_ref().map(|d| d.title.clone()) {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(draw_id(), draw_builder(&title), move |ui, _class| {
                let ctx = ui.ctx().clone();
                shared.pump(&ctx);
                shared.maybe_focus(&ctx, draw_id());
                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    // The session decides: quit silently when clean, or
                    // warn about pending edits first.
                    if let Some(session) = lock(&shared.draw).as_mut() {
                        session.request_cancel();
                    }
                }
                let outcome = {
                    let mut guard = lock(&shared.draw);
                    if let Some(session) = guard.as_mut() {
                        session.show(ui, &shared);
                    }
                    guard.as_ref().and_then(|s| s.outcome())
                };
                if let Some(outcome) = outcome
                    && let Some(session) = lock(&shared.draw).take()
                {
                    if outcome == crate::ui::draw::Outcome::Ok {
                        lock(&shared.viewer).commit_draw(
                            session.tab,
                            session.doc,
                            session.undo,
                            session.redo,
                        );
                    }
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            });
        }

        // The pin save-as window: a decorated modal-style panel; its state
        // owns the image so it outlives the pin it came from.
        if lock(&self.shared.save_dialog).is_some() {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(
                save_dialog_id(),
                save_dialog_builder(),
                move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, save_dialog_id());
                    ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                    // Closing only clears the flag: the ROOT anchor pass is
                    // what deregisters this viewport — it must be woken or
                    // the window lingers as a ghost.
                    let close = |ctx: &egui::Context| {
                        *lock(&shared.save_dialog) = None;
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    };
                    if ctx.input(|i| i.viewport().close_requested()) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        close(&ctx);
                        return;
                    }
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close(&ctx);
                        return;
                    }
                    let mut guard = lock(&shared.save_dialog);
                    if let Some(dialog) = guard.as_mut() {
                        match ui::save_dialog::show(ui, dialog) {
                            ui::save_dialog::Outcome::Confirm => match dialog.save(&shared) {
                                Ok(path) => {
                                    tracing::info!(?path, "saved");
                                    // A viewer document is now on disk:
                                    // the tab flips to clean and records
                                    // where/when it was written.
                                    if let Some(i) = dialog.viewer_tab {
                                        lock(&shared.viewer).mark_saved(i, path);
                                    }
                                    drop(guard);
                                    close(&ctx);
                                }
                                Err(e) => dialog.error = Some(e.to_string()),
                            },
                            ui::save_dialog::Outcome::Cancel => {
                                drop(guard);
                                close(&ctx);
                            }
                            ui::save_dialog::Outcome::Open => {}
                        }
                    }
                },
            );
        }
    }
}

/// The floating bar: borderless, transparent, always on top, sized to its
/// content; position restored across hide/show cycles.
fn bar_builder(position: Option<egui::Pos2>) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title("PixMe")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        // Accessory apps stay inert until a window asks for focus.
        .with_active(true)
        .with_inner_size([340.0, 48.0]);
    match position {
        Some(pos) => builder.with_position(pos),
        None => builder,
    }
}

/// The viewer window: decorated, resizable, the app's real home.
fn viewer_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(t!("app.title"))
        .with_app_id("pixme")
        .with_inner_size([860.0, 560.0])
        .with_min_inner_size([420.0, 280.0])
        .with_active(true)
}

/// The capture overlay: a borderless always-on-top window covering the
/// monitor being captured. `MonitorInfo.rect` is already in logical points
/// (xcap reports `CGDisplay` bounds) — no scale-factor conversion here.
fn overlay_builder(monitor: Option<&crate::capture::MonitorInfo>) -> egui::ViewportBuilder {
    let (size, pos) = monitor
        .map(|m| {
            (
                egui::vec2(m.rect.width as f32, m.rect.height as f32),
                egui::pos2(m.rect.x as f32, m.rect.y as f32),
            )
        })
        .unwrap_or((egui::vec2(800.0, 600.0), egui::pos2(0.0, 0.0)));
    egui::ViewportBuilder::default()
        .with_title("PixMe")
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(true)
        .with_inner_size(size)
        .with_position(pos)
}

/// A pinned capture: borderless, transparent, always on top, not a
/// taskbar entry — the image itself is the chrome; hover reveals the
/// close dot and the control strip. The builder position/size only apply
/// at creation: user drags and zoom resizes use viewport commands.
fn pin_builder(origin: egui::Pos2, size: egui::Vec2) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("PixMe")
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(true)
        .with_taskbar(false)
        .with_active(true)
        .with_inner_size(size)
        .with_position(origin)
}

/// The pin save-as window: a small decorated form floating above the
/// (always-on-top) pins it saves, not a taskbar entry.
fn save_dialog_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(t!("save.title"))
        .with_inner_size([560.0, 205.0])
        .with_resizable(false)
        .with_always_on_top()
        .with_taskbar(false)
        .with_active(true)
}

/// The Draw editor: a regular resizable window (the reference opens it
/// nearly maximised; 1100×700 reads well with the strip + options bar).
fn draw_builder(title: &str) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(title)
        .with_app_id("pixme")
        .with_inner_size([1100.0, 700.0])
        .with_min_inner_size([640.0, 420.0])
        .with_active(true)
}

/// Runs the app. The root viewport is the permanent 1×1 transparent anchor
/// window (never in the taskbar); the app first lives in the menu bar /
/// tray (`Idle`) until the user asks for a capture.
pub fn run() -> eframe::Result<()> {
    let mut options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("PixMe")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_always_on_top()
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_active(false)
            .with_inner_size([1.0, 1.0]),
        // Geometry persistence would restore a stale bar-sized rect onto the
        // invisible anchor; all positions are computed by the app instead.
        persist_window: false,
        ..Default::default()
    };
    platform::apply_native_options(&mut options);
    eframe::run_native(
        "pixme",
        options,
        Box::new(|cc| Ok(Box::new(PixMeApp::new(cc)))),
    )
}

#[cfg(test)]
mod tests {
    //! Lifecycle invariants: one state machine, identical on both OSes.
    use super::*;

    fn harness() -> (Shared, egui::Context) {
        let shared = Shared {
            state: Mutex::new(AppState::Bar),
            settings: Mutex::new(Settings::default()),
            viewer: Mutex::new(Viewer::default()),
            toolbar: Mutex::new(Toolbar::default()),
            bar_pos: Mutex::new(None),
            settings_open: AtomicBool::new(false),
            capture: Mutex::new(None),
            last_fixed_region: Mutex::new(None),
            last_capture_rect: Mutex::new(None),
            pins: Mutex::new(Vec::new()),
            next_pin_id: Mutex::new(0),
            save_dialog: Mutex::new(None),
            draw: Mutex::new(None),
            store: Mutex::new(crate::store::Store::open_in_memory()),
            pending_focus: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            last_language: Mutex::new(Language::Auto),
        };
        (shared, egui::Context::default())
    }

    fn drive(shared: &Shared, ctx: &egui::Context, command: Command) -> AppState {
        shared.push(command);
        shared.pump(ctx);
        *lock(&shared.state)
    }

    #[test]
    fn capture_enters_the_capturing_state() {
        // Region capture arms the selection overlay — the viewer only opens
        // after the overlay confirms. On a headless runner monitor
        // enumeration may fail instead: the machine must then stay in `Bar`.
        let (shared, ctx) = harness();
        let state = drive(&shared, &ctx, Command::Capture(CaptureMode::Region));
        assert!(
            matches!(state, AppState::Capturing | AppState::Bar),
            "unexpected state {state:?}"
        );
    }

    #[test]
    fn closing_bar_with_empty_session_returns_to_idle() {
        let (shared, ctx) = harness();
        assert_eq!(drive(&shared, &ctx, Command::CloseBar), AppState::Idle);
    }

    #[test]
    fn closing_bar_with_captures_opens_viewer() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).push_stub("capture 1");
        assert_eq!(drive(&shared, &ctx, Command::CloseBar), AppState::Viewer);
    }

    #[test]
    fn closing_viewer_returns_to_idle_keeping_session() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).push_stub("capture 1");
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(drive(&shared, &ctx, Command::ViewerClosed), AppState::Idle);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
    }

    /// Closing the viewer window just returns the resident app to
    /// Idle — the session lives in memory, nothing is lost, no dialog.
    #[test]
    fn dirty_viewer_close_idles_without_asking() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        lock(&shared.viewer).dirty = true;
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(drive(&shared, &ctx, Command::ViewerClosed), AppState::Idle);
        assert!(!lock(&shared.viewer).confirm_close);
    }

    #[test]
    fn confirm_cancel_keeps_the_viewer() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        lock(&shared.viewer).dirty = true;
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::Quit);
        assert_eq!(
            drive(&shared, &ctx, Command::ConfirmCancel),
            AppState::Viewer
        );
        assert!(!lock(&shared.viewer).confirm_close);
    }

    #[test]
    fn confirm_discard_idles_and_clears_the_session() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        {
            let mut viewer = lock(&shared.viewer);
            viewer.dirty = true;
            viewer.push_stub("capture 1");
        }
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::Quit);
        // Discard after Quit actually quits (confirm_then_quit) — the
        // observable part is the session being dropped and wiped.
        drive(&shared, &ctx, Command::ConfirmDiscard);
        assert!(lock(&shared.viewer).tabs.is_empty());
        assert!(!lock(&shared.viewer).dirty);
    }

    /// A clean quit writes the session; reopening the viewer brings
    /// every capture back — silent restore, titles and order included.
    #[test]
    fn quit_then_reopen_restores_the_session() {
        let (shared, ctx) = harness();
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_stub("capture 1");
            viewer.push_stub("capture 2");
            for tab in &mut viewer.tabs {
                tab.saved = true; // a clean session quits with no dialog
            }
            viewer.recompute_dirty();
        }
        drive(&shared, &ctx, Command::Quit);
        // New run: fresh viewer state, same store.
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        let viewer = lock(&shared.viewer);
        assert_eq!(viewer.tabs.len(), 2);
        assert_eq!(viewer.tabs[0].title, "capture 1");
        assert_eq!(viewer.tabs[1].title, "capture 2");
        assert!(!viewer.dirty, "all tabs came back saved");
    }

    /// With persistence on, a dirty session quits without asking — the
    /// captures live on in session.db and come back on the next open.
    /// The dirty warning only exists when retention is `Never`.
    #[test]
    fn dirty_quit_saves_without_asking_when_persisted() {
        let (shared, ctx) = harness();
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_stub("capture 1");
            viewer.recompute_dirty();
        }
        assert!(lock(&shared.viewer).dirty);
        drive(&shared, &ctx, Command::Quit);
        assert!(
            !lock(&shared.viewer).confirm_close,
            "no dialog when persisting"
        );
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
    }

    /// A discarded session empties the store — a later viewer open
    /// never resurrects captures the user threw away.
    #[test]
    fn discard_after_quit_keeps_the_store_empty() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_stub("capture 1");
            for tab in &mut viewer.tabs {
                tab.saved = true;
            }
            viewer.recompute_dirty();
        }
        drive(&shared, &ctx, Command::Quit); // persisted once
        *lock(&shared.viewer) = Viewer::default();
        // A new dirty session is quit + discarded: the store is wiped.
        lock(&shared.viewer).push_stub("capture 2");
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::Quit);
        assert_eq!(
            drive(&shared, &ctx, Command::ConfirmDiscard),
            AppState::Idle
        );
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        assert!(lock(&shared.viewer).tabs.is_empty());
    }

    #[test]
    fn bar_and_viewer_are_never_shown_together() {
        // Exclusivity is enforced structurally: the state machine holds one
        // active window at a time and only that viewport is registered.
        let (shared, ctx) = harness();
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(drive(&shared, &ctx, Command::ShowBar), AppState::Bar);
        assert_eq!(drive(&shared, &ctx, Command::OpenViewer), AppState::Viewer);
    }
}
