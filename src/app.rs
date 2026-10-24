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
use crate::i18n::{self, Language};
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
    ActiveWindow,
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
    /// Start a capture (all modes).
    Capture(CaptureMode),
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
                // Phase 3+: enter `Capturing`, run the pipeline, open the tab.
                // Until then the command demonstrates the Bar -> Viewer
                // handoff it will produce.
                tracing::info!(?mode, "capture requested");
                self.enter(AppState::Viewer, ctx);
            }
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
            Command::CloseSettings => self.settings_open.store(false, Ordering::Relaxed),
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
    /// viewports tracking the state are (de)registered promptly.
    fn enter(&self, next: AppState, ctx: &egui::Context) {
        {
            let mut state = lock(&self.state);
            if *state == next {
                return;
            }
            *state = next;
        }
        tracing::debug!(?next, "lifecycle state");
        // The viewer is a real application window: bring it to the front.
        // The bar is a palette — it rides `always_on_top` and never grabs
        // focus, so it never switches the active app.
        if next == AppState::Viewer {
            self.request_focus(viewer_id());
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

    /// Closes the anchor viewport: eframe exits once the root window is gone.
    fn quit_now(ctx: &egui::Context) {
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
    }
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
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(AppState::Idle),
                last_language: Mutex::new(settings.language),
                settings: Mutex::new(settings),
                viewer: Mutex::new(Viewer::default()),
                toolbar: Mutex::new(Toolbar::default()),
                bar_pos: Mutex::new(None),
                settings_open: AtomicBool::new(false),
                pending_focus: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
            }),
        }
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

        match *lock(&self.shared.state) {
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
            // `Idle` shows nothing; `Capturing` is owned by the capture
            // pipeline (phase 3).
            AppState::Idle | AppState::Capturing => {}
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
    fn capture_hands_bar_over_to_viewer() {
        let (shared, ctx) = harness();
        assert_eq!(
            drive(&shared, &ctx, Command::Capture(CaptureMode::FullScreen)),
            AppState::Viewer
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
        lock(&shared.viewer).tabs.push("capture 1".to_string());
        assert_eq!(drive(&shared, &ctx, Command::CloseBar), AppState::Viewer);
    }

    #[test]
    fn closing_viewer_returns_to_idle_keeping_session() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).tabs.push("capture 1".to_string());
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
            viewer.tabs.push("capture 1".to_string());
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
