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
    /// Save the active capture.
    SaveActive,
    /// Save all captures.
    SaveAll,
    /// Rename the active capture.
    RenameActive,
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
fn settings_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.settings")
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
    /// Last folder a capture was saved to (defaults to ~/Pictures).
    pub last_save_dir: Mutex<Option<std::path::PathBuf>>,
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
                let mut viewer = lock(&self.viewer);
                if viewer.dirty {
                    viewer.confirm_close = true;
                } else {
                    drop(viewer);
                    self.enter(AppState::Idle, ctx);
                }
            }
            Command::OpenSettings => {
                self.settings_open.store(true, Ordering::Relaxed);
                self.request_focus(settings_id());
            }
            Command::CloseSettings => {
                self.settings_open.store(false, Ordering::Relaxed);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Command::Quit => {
                let mut viewer = lock(&self.viewer);
                if viewer.dirty {
                    viewer.confirm_close = true;
                    viewer.confirm_then_quit = true;
                    drop(viewer);
                    // The confirmation lives in the viewer: surface it.
                    self.enter(AppState::Viewer, ctx);
                } else {
                    Self::quit_now(ctx);
                }
            }
            Command::ConfirmSave => {
                // Real saving lands with the export pipeline; until then the
                // confirmation is exercised as: log + close as if saved.
                tracing::info!("unsaved-session confirmation: save (stub)");
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
                    let folder = lock(&self.last_save_dir).clone().or_else(|| {
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
            Command::ImportClipboard
            | Command::OpenFile
            | Command::SaveActive
            | Command::SaveAll
            | Command::RenameActive => {
                // Viewer document commands land with the capture pipeline.
                tracing::info!(?command, "command not implemented yet");
            }
        }
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
        if quit {
            Self::quit_now(ctx);
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

    /// Closes the anchor viewport: eframe exits once the root window is gone.
    fn quit_now(ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
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
        Self {
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
                last_save_dir: Mutex::new(None),
                pending_focus: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
            }),
        }
        .tap_debug_capture(&cc.egui_ctx, debug_capture)
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
                                    tracing::info!(?path, "pin saved");
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

        if self.shared.settings_open.load(Ordering::Relaxed) {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(settings_id(), settings_builder(), move |ui, _class| {
                let ctx = ui.ctx().clone();
                shared.pump(&ctx);
                shared.maybe_focus(&ctx, settings_id());
                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                if ctx.input(|i| i.viewport().close_requested()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    shared.settings_open.store(false, Ordering::Relaxed);
                    // Wake the anchor so the viewport gets deregistered —
                    // same ghost-window trap as the save dialog.
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
                ui::settings::show(ui, &shared);
            });
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

/// The settings window: small, fixed size, titled, no taskbar entry.
fn settings_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(t!("settings.title"))
        .with_inner_size([340.0, 150.0])
        .with_resizable(false)
        .with_taskbar(false)
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
            last_save_dir: Mutex::new(None),
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

    #[test]
    fn dirty_viewer_close_asks_for_confirmation_instead() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).dirty = true;
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(
            drive(&shared, &ctx, Command::ViewerClosed),
            AppState::Viewer
        );
        assert!(lock(&shared.viewer).confirm_close);
    }

    #[test]
    fn confirm_cancel_keeps_the_viewer() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).dirty = true;
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::ViewerClosed);
        assert_eq!(
            drive(&shared, &ctx, Command::ConfirmCancel),
            AppState::Viewer
        );
        assert!(!lock(&shared.viewer).confirm_close);
    }

    #[test]
    fn confirm_discard_idles_and_clears_the_session() {
        let (shared, ctx) = harness();
        {
            let mut viewer = lock(&shared.viewer);
            viewer.dirty = true;
            viewer.push_stub("capture 1");
        }
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::ViewerClosed);
        assert_eq!(
            drive(&shared, &ctx, Command::ConfirmDiscard),
            AppState::Idle
        );
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
