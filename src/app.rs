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

use crate::capture::MonitorInfo;
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
    /// Open the settings window on the About page (egui twin of the
    /// macOS native About panel).
    OpenAbout,
    /// The settings window was closed.
    CloseSettings,
    /// Application quit (menu only — never a window-close side effect).
    Quit,
    /// Create a capture document from the clipboard.
    ImportClipboard,
    /// Open an image file into the viewer.
    OpenFile,
    /// Reopen one file from the recent-files list (File → Open).
    OpenPath(std::path::PathBuf),
    /// Reopen every file in the recent list (at most five).
    OpenAllRecent,
    /// Empty the recent-files list (File → Open).
    ClearRecent,
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
    /// Delete the active document from the session.
    DeleteActive,
    /// Delete the active document's file on disk (the tab's "Supprimer
    /// le fichier" — confirmation dialog, greyed unless it has one).
    DeleteFileActive,
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

/// Which modal surface owns input right now, if any. Only one modal can
/// exist at a time (opening any command that would create a second one is
/// disabled while a modal is up).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ModalState {
    /// No modal is open.
    #[default]
    None,
    /// The settings panel (drawn inside the viewer viewport).
    Settings,
    /// The save-as dialog window.
    SaveDialog,
    /// The Draw (Dessiner) editor window.
    Draw,
    /// A dialog living inside the viewer window (rename, resize,
    /// protection, delete-file, unsaved confirmation, dup names).
    ViewerDialog,
    /// A capture session is live: the overlay owns the screen, so menus
    /// and commands grey out — a second capture can never start under it.
    Capturing,
    /// The quit-time session save is writing on a worker — the UI is
    /// frozen behind the saving veil until the app actually closes.
    Saving,
}

/// Short, privacy-safe description of a dispatched command — `Debug`
/// alone would dump the full user path of `OpenPath`; everything else
/// prints as `Variant(field)` (modes, ids — never user data).
fn command_desc(command: &Command) -> String {
    match command {
        Command::OpenPath(path) => format!("OpenPath({})", crate::paths::file_name(path)),
        other => format!("{other:?}"),
    }
}

/// The single source of truth for command availability vs modal state:
/// menu items (native + tray + egui strip), the dispatch safety net and
/// every button read this table. The match is exhaustive — adding a
/// `Command` variant without a ruling here fails compilation.
///
/// Rulings: `Quit` is always allowed (it closes the modal first, then
/// runs the normal confirmation flow). While Draw is open its own
/// undo/redo apply to its session, and `edit.copy`/paste live inside
/// egui text fields — `CopyActive` stays reachable. `OpenSettings`
/// re-focuses the existing settings window rather than stacking.
/// Confirmation answers and overlay outcomes stay enabled (they are
/// pushed by the modal/dialog itself, not by user menus).
pub fn is_command_enabled(command: &Command, modal: ModalState) -> bool {
    if modal == ModalState::None {
        return true;
    }
    // A quit already committed is final: nothing may run while the
    // store writes — not even a second Quit.
    if modal == ModalState::Saving {
        return false;
    }
    match command {
        // Always live: quit, modal-owned answers, overlay capture flow
        // and pin self-management.
        Command::Quit
        | Command::CloseSettings
        | Command::ConfirmSave
        | Command::ConfirmDiscard
        | Command::ConfirmCancel
        | Command::ConfirmCapture
        | Command::CancelCapture
        | Command::ClosePin(_)
        | Command::CopyPin(_)
        | Command::ViewerClosed
        | Command::CloseBar => true,
        // Draw owns its document: its undo/redo/copy apply there; the
        // viewer's copies must not act behind the modal.
        Command::UndoActive | Command::RedoActive | Command::CopyActive => {
            modal == ModalState::Draw
        }
        // Re-invoking the open modal's entry re-focuses it instead of
        // re-opening. When the SETTINGS modal is up the same menu click
        // brings it forward.
        Command::OpenSettings | Command::OpenAbout => modal == ModalState::Settings,
        Command::OpenViewer => modal == ModalState::Settings,
        // Everything else is blocked while a modal owns the UI.
        Command::DeleteActive
        | Command::DeleteFileActive
        | Command::Capture(_)
        | Command::CaptureNow(_)
        | Command::ShowBar
        | Command::ImportClipboard
        | Command::OpenFile
        | Command::OpenPath(_)
        | Command::OpenAllRecent
        | Command::ClearRecent
        | Command::OpenLastDir
        | Command::NewDocument
        | Command::SaveActive
        | Command::SaveAll
        | Command::RenameActive
        | Command::OpenDraw
        | Command::PrintActive
        | Command::SavePin(_) => false,
    }
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
/// The capture-delay countdown card.
fn countdown_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.countdown")
}
/// The Draw (Dessiner) editor window.
fn draw_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("pixme.draw")
}

/// Wakes every command-pumping viewport from outside a pass (OS menu
/// callbacks, worker threads): `Context::request_repaint()` alone only
/// targets `ViewportId::ROOT`, which macOS throttles when the 1×1 anchor
/// sits behind a visible window.
pub fn wake_viewports(ctx: &egui::Context) {
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.repaint_if_live(viewer_id());
    ctx.repaint_if_live(bar_id());
    ctx.repaint_if_live(draw_id());
}

/// `Context::request_repaint_of` and `Context::send_viewport_cmd_to` on a
/// viewport id whose window does not exist insert a *default-built*
/// `ViewportState` (decorated, titled "egui window") into `ctx.viewports`.
/// When a later non-root pass emits it in `viewport_output`, eframe
/// materializes it as a real window for a frame — the "egui window" flash
/// seen between the capture click and the crosshair. These helpers no-op
/// unless the window exists.
trait ViewportWake {
    /// `request_repaint_of(id)` only when `id`'s window exists.
    fn repaint_if_live(&self, id: egui::ViewportId);
    /// `send_viewport_cmd_to(id, cmd)` only when `id`'s window exists.
    fn cmd_if_live(&self, id: egui::ViewportId, cmd: egui::ViewportCommand);
}

impl ViewportWake for egui::Context {
    fn repaint_if_live(&self, id: egui::ViewportId) {
        if self.input(|i| i.raw.viewports.contains_key(&id)) {
            self.request_repaint_of(id);
        }
    }

    fn cmd_if_live(&self, id: egui::ViewportId, cmd: egui::ViewportCommand) {
        if self.input(|i| i.raw.viewports.contains_key(&id)) {
            self.send_viewport_cmd_to(id, cmd);
        }
    }
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
    /// Last on-screen outer rectangle of the viewer viewport (restored on
    /// reopen and translated onto the captured monitor after a capture).
    pub viewer_rect: Mutex<Option<egui::Rect>>,
    /// Monitor the viewer must appear on when it next opens (set after a
    /// capture confirms so the viewer lands on the captured screen).
    pub viewer_target_monitor: Mutex<Option<MonitorInfo>>,
    /// The floating toolbar's transient state.
    pub toolbar: Mutex<Toolbar>,
    /// Last on-screen position of the bar viewport (restored on reopen).
    pub bar_pos: Mutex<Option<egui::Pos2>>,
    /// Last on-screen outer rect of the bar viewport — the countdown card
    /// anchors below it and follows its drags.
    pub bar_rect: Mutex<Option<egui::Rect>>,
    /// Monitor the floating bar currently sits on — resolved in NATIVE
    /// units at write time (bar's own ppp), so the countdown never has to
    /// mix two windows' point spaces (mixed-DPI safe).
    pub bar_monitor: Mutex<Option<MonitorInfo>>,
    /// Monitor the viewer currently sits on — same role as `bar_monitor`.
    pub viewer_monitor: Mutex<Option<MonitorInfo>>,
    /// winit `available_monitors()` placements `(x, y, w, h)` in physical
    /// pixels, snapshotted for the overlay's `with_monitor`/`SetMonitor`
    /// targeting (xcap and winit don't enumerate in the same order).
    pub winit_placements: Mutex<Vec<(i32, i32, u32, u32)>>,
    /// Whether the settings window is open.
    pub settings_open: AtomicBool,
    /// The Windows About modal is open (egui twin of the native panel).
    pub about_open: AtomicBool,
    /// The editable copy shown by the settings window — written back to
    /// `settings` only on OK; Annuler/close drops it untouched.
    pub settings_draft: Mutex<Option<Settings>>,
    /// The live capture session while in `Capturing` (`None` otherwise).
    pub capture: Mutex<Option<OverlaySession>>,
    /// The armed capture-delay countdown — a floating card outside the
    /// state machine that ticks the seconds down and fires
    /// [`Command::CaptureNow`] at zero.
    pub countdown: Mutex<Option<ui::countdown::Countdown>>,
    /// The last confirmed fixed-region `(monitor_id, rect)` in
    /// monitor-logical space: the next fixed capture opens at the same
    /// size and position on that monitor.
    pub last_fixed_region: Mutex<Option<(u32, egui::Rect)>>,
    /// The monitor the pending capture was launched from — set by
    /// [`Shared::push_capture`] when a window (the bar, the viewer) starts
    /// a capture, consumed by `begin_capture`. `None` for resident-icon
    /// launches (no window): `begin_capture` falls back to the monitor
    /// under the cursor, then the primary monitor.
    capture_start_monitor: Mutex<Option<MonitorInfo>>,
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
    /// The session store's ordered write queue — a dedicated thread
    /// owns the database for the whole run; every mutation lands there
    /// shortly after the change that produced it.
    pub writer: crate::store::writer::Writer,
    /// The in-flight session restore: the store thread decodes images
    /// off the UI thread and hands the session back over the channel.
    pub restore_rx: Mutex<Option<std::sync::mpsc::Receiver<crate::store::StoredSession>>>,
    /// The in-flight quit-time flush: `Some` while the writer drains
    /// its queue; the reply (or the deadline) closes the app.
    pub quit_rx: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// When the quit flush was requested — bounds the wait.
    quit_started: Mutex<Option<std::time::Instant>>,
    /// The in-flight "Tout enregistrer" batch: the worker owns the tabs
    /// while writing; `pump` reattaches them when it lands.
    save_all: Mutex<Option<SaveAllJob>>,
    /// The in-flight single-capture save (the save-as dialog's
    /// Enregistrer): the PNG encode rides a worker so a 5K write never
    /// stalls a frame; `pump` applies the result.
    save_one: Mutex<Option<SaveOneJob>>,
    /// One-shot, non-modal store-failure notice — set the first time a
    /// session write reports `DocFailed` (translated); the per-tab
    /// warning glyph is the standing signal afterwards.
    pub store_notice: Mutex<Option<String>>,
    /// The notice was already shown once this run — never nags twice.
    store_notice_seen: AtomicBool,
    /// Set by the settings apply when the retention policy may have
    /// pruned (or emptied) the store behind the tabs' backs — the next
    /// `sync_store` forgets the write baselines so everything missing
    /// is re-queued. A flag, not a direct lock: the apply runs while
    /// the viewer lock is already held by its own pass.
    pub store_marks_reset: AtomicBool,
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
    /// Last modal state pushed to the OS menus — diffs drive the
    /// enable/disable sync so menu greying tracks every transition.
    last_menu_sync: Mutex<(ModalState, bool, bool, bool)>,
    /// An in-viewer dialog (rename, resize, protection, delete-file,
    /// unsaved confirm, dup names) is up — synced every viewer pass so
    /// `modal_state()` never locks the viewer while `viewer.show` holds
    /// it (and would self-deadlock).
    pub viewer_dialog_open: AtomicBool,
    /// One-shot notice drawn by the first window to appear (the Windows
    /// data/logs fallback message — set at startup, cleared on OK).
    pub startup_notice: Mutex<Option<String>>,
}

/// `capture_id` source — every capture gets one id so its whole life
/// (start, confirm, store, cancel, failure) greps as a single trail.
static NEXT_CAPTURE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A running background save-all: the moved tabs come back over `rx`,
/// `done` ticks once per processed tab for the progress veil, and
/// `then_confirm` closes the saved session on completion (ConfirmSave).
struct SaveAllJob {
    rx: std::sync::mpsc::Receiver<(Vec<crate::ui::viewer::Tab>, Vec<std::path::PathBuf>)>,
    done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    then_confirm: bool,
}

/// A running background single-file save (the save-as dialog): the
/// encode+write happens off the UI thread; `pump` marks the tab clean,
/// updates the recent files and closes the dialog on success.
struct SaveOneJob {
    rx: std::sync::mpsc::Receiver<Result<(), crate::export::ExportError>>,
    /// The destination the worker is writing.
    path: std::path::PathBuf,
    /// The folder — becomes the next save's default location.
    dir: std::path::PathBuf,
    /// The viewer tab to mark clean (`None` for pin saves).
    viewer_tab: Option<usize>,
    /// Open the file manager on the result.
    reveal: bool,
}

/// How long quit waits for the writer queue to empty before giving up
/// — a `warn` is logged and the app exits regardless (the session is
/// a convenience, a stuck disk must not hang the process).
const QUIT_FLUSH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

impl Shared {
    /// Queues a command for dispatch on the next pump.
    pub fn push(&self, command: Command) {
        lock(&self.pending).push(command);
    }

    /// Queues a capture command and records the monitor the request came
    /// from — the visible window (the bar, the viewer) under the cursor —
    /// so the overlay opens on "the screen where the zone starts". The
    /// record survives the capture delay: `begin_capture` may run later,
    /// from the anchor's pass. Resident-icon launches (no window) leave
    /// the slot empty and `begin_capture` falls back to the primary.
    pub fn push_capture(&self, mode: CaptureMode, ctx: &egui::Context) {
        *lock(&self.capture_start_monitor) = monitor_under(ctx);
        self.push(Command::Capture(mode));
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
        self.modal_state() != ModalState::None
    }

    /// The modal surface currently owning input, if any.
    pub fn modal_state(&self) -> ModalState {
        if self.saving() || lock(&self.save_all).is_some() {
            ModalState::Saving
        } else if lock(&self.draw).is_some() {
            ModalState::Draw
        } else if lock(&self.save_dialog).is_some() {
            ModalState::SaveDialog
        } else if self.settings_open.load(Ordering::Relaxed)
            || self.about_open.load(Ordering::Relaxed)
        {
            // The About modal is modal to the viewer too: the dimmed veil
            // + click-eater cover the panel until OK / cross / Esc.
            ModalState::Settings
        } else if self.viewer_dialog_open.load(Ordering::Relaxed) {
            ModalState::ViewerDialog
        } else if *lock(&self.state) == AppState::Capturing {
            ModalState::Capturing
        } else {
            ModalState::None
        }
    }

    /// Whether `command` may run in the current modal state — the UI
    /// reads this to grey out buttons and menu entries.
    pub fn command_enabled(&self, command: &Command) -> bool {
        is_command_enabled(command, self.modal_state())
    }

    /// True while the quit-time save runs on its worker — the viewer
    /// paints the saving veil and every command is gated off until the
    /// store comes back and the app closes.
    pub fn saving(&self) -> bool {
        lock(&self.quit_rx).is_some()
    }

    /// `(done, total)` while a background save-all batch runs — the
    /// viewer paints the progress veil.
    pub fn save_all_progress(&self) -> Option<(usize, usize)> {
        lock(&self.save_all)
            .as_ref()
            .map(|j| (j.done.load(Ordering::Relaxed), j.total))
    }

    /// The concrete theme for the current appearance setting and OS theme.
    pub fn appearance_kind(&self, ctx: &egui::Context) -> ui::theme::ThemeKind {
        lock(&self.settings).appearance.kind(ctx.system_theme())
    }

    /// Drains OS menu events plus queued commands, dispatches them, then
    /// reapplies language side effects if the setting changed. Called at the
    /// top of every viewport pass so any visible window keeps dispatch alive.
    pub fn pump(&self, ctx: &egui::Context) {
        crate::logging::report_health();
        platform::strip_undecorated_frame();
        // The quit-time flush finished on the store thread — or the
        // wait ran out / the writer died: the app closes either way.
        // Checked before `dispatch` so the pass that initiates `Quit`
        // always leaves `quit_rx` armed for one frame (the saving veil
        // and the unit-test `drain_quit` barrier) instead of racing the
        // worker thread inside the same `pump` call.
        if lock(&self.quit_rx).is_some() {
            let flushed = lock(&self.quit_rx)
                .as_ref()
                .map(|r| r.try_recv())
                .is_some_and(|r| !matches!(r, Err(std::sync::mpsc::TryRecvError::Empty)));
            let timed_out =
                lock(&self.quit_started).is_some_and(|t| t.elapsed() > QUIT_FLUSH_TIMEOUT);
            if timed_out {
                tracing::warn!(
                    "session flush exceeded {}s — quitting anyway",
                    QUIT_FLUSH_TIMEOUT.as_secs()
                );
            }
            if flushed || timed_out {
                *lock(&self.quit_rx) = None;
                *lock(&self.quit_started) = None;
                ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
                // The Close only sits in the root's command queue until the
                // root's next pass — nothing else repaints it in Idle, so
                // the app would never actually shut down.
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            } else {
                // No viewport may be repainting while the flush drains
                // (Idle: only the anchor exists — the saving veil's own
                // request_repaint does this when the viewer is open):
                // keep the pump alive so `flushed`/`timed_out` get
                // evaluated, or the app deadlocks with modal=Saving.
                ctx.request_repaint();
            }
        }
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
        // Menu enable state follows every modal transition AND the
        // document/last-dir gates the doc-bound entries depend on (a tab
        // opening or closing changes neither modal): the sync input is
        // the whole tuple, compared each pass. `pump` only runs on
        // viewport passes, i.e. the main thread, where the thread_local
        // muda handles live.
        let modal = self.modal_state();
        let (has_doc, has_file) = {
            let viewer = lock(&self.viewer);
            (viewer.active_index().is_some(), viewer.active_has_file())
        };
        let has_last_dir = {
            let s = lock(&self.settings);
            s.last_open_dir.is_some() || s.last_save_dir.is_some()
        };
        let sync = (modal, has_doc, has_file, has_last_dir);
        let mut last_menu_sync = lock(&self.last_menu_sync);
        if *last_menu_sync != sync {
            *last_menu_sync = sync;
            platform::sync_menu_enabled(modal, has_doc, has_file, has_last_dir);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ctx.repaint_if_live(viewer_id());
            ctx.repaint_if_live(bar_id());
        }
        // Recent files change without a modal transition (open/save
        // succeeds, history cleared): the macOS File menu is native and
        // must be re-synced explicitly — a no-op on Windows, whose menus
        // are egui-drawn every frame.
        platform::sync_recent_menu(&lock(&self.settings).recent_files);
        // A background session restore lands here — swap the empty
        // session for the loaded tabs and drop the spinner.
        let done = lock(&self.restore_rx)
            .as_ref()
            .and_then(|r| r.try_recv().ok());
        if let Some(session) = done {
            *lock(&self.restore_rx) = None;
            self.finish_restore(session);
            ctx.repaint_if_live(viewer_id());
        }
        // A background save-all finished: the tabs come back into the
        // viewer; a ConfirmSave batch then closes the (now clean)
        // session, or keeps it open when some tab is still unsaved.
        let save_done = lock(&self.save_all)
            .as_ref()
            .and_then(|j| j.rx.try_recv().ok());
        if let Some((tabs, saved_paths)) = save_done {
            let job = lock(&self.save_all).take().expect("save-all job");
            {
                let mut s = lock(&self.settings);
                for path in saved_paths {
                    s.push_recent(path);
                }
                s.save();
            }
            {
                let mut viewer = lock(&self.viewer);
                viewer.tabs = tabs;
                viewer.recompute_dirty();
            }
            if job.then_confirm {
                if lock(&self.viewer).dirty {
                    lock(&self.viewer).confirm_then_quit = false;
                } else {
                    self.finish_confirmation(ctx);
                }
            }
            ctx.repaint_if_live(viewer_id());
        }
        // A background single-file save landed: mark the tab clean,
        // refresh the recent files and close the dialog — or hand the
        // error back to it (stays open, inline message).
        let one_done = lock(&self.save_one)
            .as_ref()
            .and_then(|j| j.rx.try_recv().ok());
        if let Some(result) = one_done {
            let job = lock(&self.save_one).take().expect("save-one job");
            match result {
                Ok(()) => {
                    tracing::info!(file = %crate::paths::file_name(&job.path), "saved");
                    if job.reveal {
                        platform::reveal_in_file_manager(&job.path);
                    }
                    if let Some(i) = job.viewer_tab {
                        lock(&self.viewer).mark_saved(i, job.path.clone());
                    }
                    let mut s = lock(&self.settings);
                    s.last_save_dir = Some(job.dir);
                    s.push_recent(job.path);
                    s.save();
                    *lock(&self.save_dialog) = None;
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
                Err(e) => {
                    if let Some(dialog) = lock(&self.save_dialog).as_mut() {
                        dialog.saving = false;
                        dialog.error = Some(e.to_string());
                    }
                    ctx.repaint_if_live(save_dialog_id());
                }
            }
        }
        self.sync_store(ctx);
    }

    /// The one dispatch path for every entry point.
    fn dispatch(&self, command: Command, ctx: &egui::Context) {
        // Safety net under the greyed menus: an out-of-date item, a global
        // hotkey or a queued click can still deliver a stale command —
        // `is_command_enabled` is the single table deciding for all of them.
        let modal = self.modal_state();
        if !is_command_enabled(&command, modal) {
            tracing::debug!(?command, ?modal, "command blocked while modal is open");
            return;
        }
        // Every user action is one Command through this funnel — logging
        // it here covers the bar, the menus, the tray and the viewer.
        tracing::info!(command = %command_desc(&command), "command");
        match command {
            Command::Capture(mode) => {
                let delay = lock(&self.settings).capture_delay_secs;
                if delay > 0 {
                    // The countdown viewport owns the wait: it ticks and
                    // shows the remaining seconds bottom-right of the
                    // monitor under the cursor, then fires CaptureNow.
                    // The delay is session state: it persists between
                    // captures but is never saved to disk.
                    *lock(&self.countdown) = Some(ui::countdown::Countdown::new(mode, delay));
                } else {
                    self.begin_capture(mode, ctx);
                }
            }
            Command::CaptureNow(mode) => self.begin_capture(mode, ctx),
            Command::ConfirmCapture => self.confirm_capture(ctx),
            Command::CancelCapture => self.cancel_capture(ctx),
            Command::ShowBar => {
                // The bar reopens where the user last dragged it — but
                // only while PixMe still lives on that monitor. The
                // anchor is the visible window's monitor (the viewer's,
                // or the bar's own when the bar is re-invoked); with no
                // window up it is the monitor under the mouse. A
                // remembered spot on another monitor is stale — the bar
                // re-centres on the anchor instead of popping up where
                // the user is not looking.
                let anchor = match *lock(&self.state) {
                    AppState::Viewer => lock(&self.viewer_monitor).clone(),
                    AppState::Bar => lock(&self.bar_monitor).clone(),
                    _ => None,
                }
                .or_else(|| {
                    platform::cursor_position()
                        .and_then(|(x, y)| crate::capture::screen::monitor_at(x, y))
                })
                .or_else(|| monitor_under(ctx))
                .or_else(|| crate::capture::screen::primary_monitor().ok());
                let monitors = crate::capture::screen::monitors().unwrap_or_default();
                let spot_on_anchor = (*lock(&self.bar_pos))
                    .zip(anchor.as_ref())
                    .is_some_and(|(pos, a)| ui_pos_on_anchor(pos, &monitors, a));
                // `place_on_monitor` asks `sync_viewport` to re-centre the
                // bar on that monitor next frame — only wanted on the
                // fallback path, never over a remembered spot.
                if !spot_on_anchor {
                    *lock(&self.bar_pos) = None;
                    if let Some(m) = anchor {
                        let scale = m.scale_factor.max(1.0);
                        let origin = platform::native_to_ui(
                            egui::pos2(m.rect.x as f32, m.rect.y as f32),
                            scale,
                        );
                        let size = platform::native_to_ui(
                            egui::pos2(m.rect.width as f32, m.rect.height as f32),
                            scale,
                        )
                        .to_vec2();
                        *lock(&self.bar_pos) = Some(egui::pos2(
                            origin.x + (size.x - 340.0) / 2.0,
                            origin.y + size.y * 0.12,
                        ));
                        lock(&self.toolbar).place_on_monitor(m);
                    }
                }
                self.enter(AppState::Bar, ctx);
            }
            Command::OpenViewer => {
                self.enter(AppState::Viewer, ctx);
                // Re-invoking while the window is already open (behind
                // other apps, or minimized in the Dock) must still bring
                // it forward: `enter` only requests focus on a state
                // CHANGE, so de-minimize and focus explicitly here.
                ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Minimized(false));
                // The direct Focus command both wakes the (possibly
                // sleeping) viewport and, on macOS, activates the app
                // (`activateIgnoringOtherApps` + `makeKeyAndOrderFront`)
                // — a mere in-app focus request would do nothing while
                // another application is frontmost.
                ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Focus);
                self.request_focus(viewer_id());
                ctx.repaint_if_live(viewer_id());
            }
            Command::CloseBar => {
                // Back where the user came from: the viewer when the session
                // has captures, the resident icon otherwise. The guard is
                // bound out of the `if` scrutinee: `enter(Viewer)` re-locks
                // `viewer` and a live scrutinee guard would deadlock.
                let empty = lock(&self.viewer).tabs.is_empty();
                if empty {
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
            Command::OpenAbout => {
                // macOS keeps its native About panel in the PixMe menu;
                // Windows opens the egui twin of that panel.
                #[cfg(not(target_os = "macos"))]
                {
                    self.about_open.store(true, Ordering::Relaxed);
                    ctx.repaint_if_live(viewer_id());
                }
            }
            Command::OpenSettings => {
                if self.settings_open.load(Ordering::Relaxed) {
                    // Already open: re-invocation brings the existing
                    // modal to the front rather than re-opening it.
                    ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Minimized(false));
                    self.request_focus(viewer_id());
                    ctx.repaint_if_live(viewer_id());
                    return;
                }
                self.settings_open.store(true, Ordering::Relaxed);
                // The window edits a draft: changes apply on OK only.
                *lock(&self.settings_draft) = Some(lock(&self.settings).clone());
                // The settings are a modal of the viewer: make sure the
                // viewer exists (and is restored) before it shows.
                self.enter(AppState::Viewer, ctx);
                // The modal lives in the VIEWER's pass — repainting the
                // anchor alone would leave an idle viewer asleep and the
                // modal would wait for the next input event.
                ctx.repaint_if_live(viewer_id());
            }
            Command::CloseSettings => {
                self.settings_open.store(false, Ordering::Relaxed);
                *lock(&self.settings_draft) = None;
                ctx.repaint_if_live(viewer_id());
            }
            Command::Quit => {
                // Quit is always live: close the non-Draw modals first so
                // the confirmation (or the exit) never sits behind them.
                // An open Draw editor is committed as usual by
                // `save_session` on the actual quit.
                self.settings_open.store(false, Ordering::Relaxed);
                *lock(&self.settings_draft) = None;
                if lock(&self.save_dialog).is_some() {
                    *lock(&self.save_dialog) = None;
                }
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
                // "Save" in the unsaved-session dialog = save-all, then
                // close only when every unsaved tab reached disk. When
                // the batch runs in background, its completion decides.
                if self.save_all(ctx, true) {
                    self.finish_confirmation(ctx);
                } else if lock(&self.save_all).is_none() {
                    // Couldn't even start (name clash / no folder).
                    let mut viewer = lock(&self.viewer);
                    viewer.confirm_close = false;
                    viewer.confirm_then_quit = false;
                }
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
                if let Some(capture) = capture {
                    match crate::export::clipboard::copy_capture(&capture) {
                        Ok(()) => tracing::info!("clipboard_copy"),
                        Err(e) => tracing::warn!("pin copy failed: {e}"),
                    }
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
                    tracing::info!("clipboard_imported");
                    let title = self.capture_title();
                    lock(&self.viewer)
                        .push_capture_document(Document::new(synthetic_capture(image)), title);
                    self.enter(AppState::Viewer, ctx);
                }
                Ok(None) => tracing::info!("clipboard holds no image"),
                Err(e) => tracing::warn!("clipboard import failed: {e}"),
            },
            Command::OpenFile => self.open_files(None, ctx),
            Command::OpenPath(path) => {
                if self.open_one(&path) {
                    let mut s = lock(&self.settings);
                    s.push_recent(path.clone());
                    s.save();
                    self.enter(AppState::Viewer, ctx);
                } else {
                    // The recent entry is a dead path — drop it so the
                    // menu stops offering it.
                    let mut s = lock(&self.settings);
                    s.remove_recent(&path);
                    s.save();
                    tracing::warn!(file = %crate::paths::file_name(&path), "recent file could not be opened");
                }
            }
            Command::OpenAllRecent => {
                let paths = lock(&self.settings).recent_files.clone();
                let opened = paths.iter().any(|p| self.open_one(p));
                if opened {
                    self.enter(AppState::Viewer, ctx);
                }
            }
            Command::ClearRecent => {
                let mut s = lock(&self.settings);
                s.recent_files.clear();
                s.save();
            }
            // Same picker, pre-opened in the last used folder.
            Command::OpenLastDir => {
                // Single settings lock — a second `lock()` inside
                // `or_else` would deadlock on the same thread (the first
                // guard lives until the end of the statement).
                let dir = {
                    let s = lock(&self.settings);
                    s.last_open_dir.clone().or_else(|| s.last_save_dir.clone())
                };
                if let Some(dir) = dir {
                    platform::open_folder(&dir);
                }
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
                let fallback = lock(&self.settings).last_save_dir.clone().or_else(|| {
                    directories::UserDirs::new()
                        .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
                });
                if let Some(t) = target {
                    let folder = t
                        .source
                        .as_ref()
                        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
                        .or(fallback);
                    if let Some(folder) = folder {
                        let mut dialog =
                            crate::ui::save_dialog::SaveDialog::new(Arc::new(t.capture), folder);
                        // The title seeds the file name (sanitized later)
                        // — an unchanged title lands on the source file
                        // and overwrites it; a renamed tab saves under
                        // its new name.
                        dialog.filename = t.title;
                        if let Some(path) = t.source {
                            if let Some(fmt) = ext_format(&path) {
                                dialog.format = fmt;
                            }
                            dialog.existing_ok = Some(path);
                        }
                        dialog.viewer_tab = Some(t.index);
                        *lock(&self.save_dialog) = Some(dialog);
                        self.request_focus(save_dialog_id());
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                }
            }
            Command::SaveAll => {
                self.save_all(ctx, false);
            }
            Command::RenameActive => {
                lock(&self.viewer).open_rename();
                // The dialog is painted by the viewer pass — menu-driven
                // opens must wake it or it waits for the next OS event.
                ctx.repaint_if_live(viewer_id());
            }
            Command::CopyActive => {
                if let Some(img) = lock(&self.viewer).copy_image() {
                    match crate::export::clipboard::copy_image(&img) {
                        Ok(()) => tracing::info!("clipboard_copy"),
                        Err(e) => tracing::warn!("copy failed: {e}"),
                    }
                }
            }
            Command::DeleteActive => {
                // Bind first: the scrutinee's MutexGuard would otherwise
                // live for the whole `if let` body and the second lock
                // would deadlock on the same thread.
                let active = lock(&self.viewer).active_index();
                if let Some(i) = active {
                    lock(&self.viewer).close_tabs(&[i]);
                    tracing::info!("tab deleted");
                }
            }
            Command::DeleteFileActive => {
                lock(&self.viewer).open_delete_file_dialog();
                ctx.repaint_if_live(viewer_id());
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
                    ctx.repaint_if_live(draw_id());
                } else {
                    lock(&self.viewer).undo_active();
                    ctx.repaint_if_live(viewer_id());
                }
            }
            Command::RedoActive => {
                if let Some(draw) = lock(&self.draw).as_mut() {
                    draw.redo_once();
                    ctx.repaint_if_live(draw_id());
                } else {
                    lock(&self.viewer).redo_active();
                    ctx.repaint_if_live(viewer_id());
                }
            }
            Command::PrintActive => {
                let (img, title) = {
                    let viewer = lock(&self.viewer);
                    (viewer.active_image(), viewer.active_title())
                };
                if let Some(img) = img {
                    match crate::export::file::print_image(
                        &img,
                        title.as_deref().unwrap_or("PixMe"),
                    ) {
                        Ok(()) => tracing::info!("printed"),
                        Err(e) => tracing::warn!("print failed: {e}"),
                    }
                }
            }
        }
    }

    /// Writes every unsaved tab to the last-used folder (or ~/Pictures) as
    /// timestamped PNGs — the "Save All" / unsaved-confirmation path has no
    /// per-tab dialog, each file is unique via the index suffix.
    /// Loads one image path as a clean tab titled after the file —
    /// shared by File → Open's picker loop and the recent-files entries.
    /// A file already open as a tab is focused, never duplicated.
    /// Returns `false` when the file cannot be decoded.
    fn open_one(&self, path: &std::path::Path) -> bool {
        {
            let mut viewer = lock(&self.viewer);
            let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
            if let Some(i) = viewer.tabs.iter().position(|t| {
                t.doc.meta.source_path.as_ref().is_some_and(|s| {
                    std::fs::canonicalize(s).unwrap_or_else(|_| s.clone()) == canon
                })
            }) {
                viewer.set_active(i);
                return true;
            }
        }
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
                doc.meta.source_path = Some(path.to_path_buf());
                lock(&self.viewer).push_named_document(
                    doc, title, // From disk = clean until edited.
                    true,
                );
                tracing::info!(file = %crate::paths::file_name(path), "document_opened");
                true
            }
            Err(e) => {
                tracing::warn!(file = %crate::paths::file_name(path), "cannot open image: {e}");
                false
            }
        }
    }

    /// The File → Open picker (seeded with `dir` when given) — one tab per
    /// image, and the picked folder is persisted for next time.
    fn open_files(&self, dir: Option<std::path::PathBuf>, ctx: &egui::Context) {
        let mut dialog =
            rfd::FileDialog::new().add_filter("Images", &["png", "jpg", "jpeg", "webp"]);
        if let Some(dir) = dir {
            dialog = dialog.set_directory(dir);
        }
        let Some(paths) = dialog.pick_files() else {
            ctx.request_repaint();
            return;
        };
        let mut opened = Vec::new();
        for path in &paths {
            if self.open_one(path) {
                opened.push(path.clone());
            }
        }
        if !opened.is_empty() {
            let mut s = lock(&self.settings);
            if let Some(dir) = paths
                .first()
                .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            {
                s.last_open_dir = Some(dir);
            }
            for path in opened {
                s.push_recent(path);
            }
            s.save();
            self.enter(AppState::Viewer, ctx);
        }
    }

    /// The next capture tab title, expanded from the filename template —
    /// `Screenshot#####`-style names. The `#` counter is persisted when
    /// consumed; templates without it don't burn sequence numbers.
    fn capture_title(&self) -> String {
        let mut s = lock(&self.settings);
        let title = crate::config::expand_template(
            &s.filename_template,
            s.filename_counter,
            &chrono::Local::now(),
        );
        if s.filename_template.contains('#') {
            s.filename_counter += 1;
            s.save();
        }
        title
    }

    /// Writes every unsaved tab to the last-used folder (or ~/Pictures) as
    /// PNGs with annotations composited — the same sequential write loop as
    /// before, just riding a worker thread so the viewer paints a progress
    /// veil (`pump` reattaches the tabs when the job lands). Returns `true`
    /// only when the session is clean right away; `then_confirm` marks a
    /// ConfirmSave batch so its completion runs the saved-session close.
    fn save_all(&self, ctx: &egui::Context, then_confirm: bool) -> bool {
        let Some(folder) = lock(&self.settings).last_save_dir.clone().or_else(|| {
            directories::UserDirs::new().and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
        }) else {
            tracing::warn!("save-all: no writable folder known");
            return false;
        };
        let mut viewer = lock(&self.viewer);
        // Duplicate titles are legal everywhere but HERE: two files can
        // never share a name, so the batch is refused until the user
        // renames — the viewer surfaces the blocking notice.
        let mut seen = std::collections::HashSet::new();
        let clash = viewer
            .tabs
            .iter()
            .filter(|t| !t.saved)
            .any(|t| !seen.insert(crate::config::filename::sanitize_filename(t.title.trim())));
        if clash {
            tracing::warn!("save-all blocked: duplicate tab titles");
            viewer.dup_names = true;
            ctx.repaint_if_live(viewer_id());
            return false;
        }
        if viewer.tabs.iter().all(|t| t.saved) {
            return true;
        }
        let total = viewer.tabs.iter().filter(|t| !t.saved).count();
        // The confirm modal must not linger above the progress veil;
        // `confirm_then_quit` survives for the completion step.
        viewer.confirm_close = false;
        let tabs = std::mem::take(&mut viewer.tabs);
        drop(viewer);
        let (tx, rx) = std::sync::mpsc::channel();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        *lock(&self.save_all) = Some(SaveAllJob {
            rx,
            done: std::sync::Arc::clone(&done),
            total,
            then_confirm,
        });
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let mut tabs = tabs;
            let mut saved_paths = Vec::new();
            for tab in tabs.iter_mut().filter(|t| !t.saved) {
                let stem = crate::config::filename::sanitize_filename(tab.title.trim());
                let path = unique_png(&folder, &stem);
                let mut capture = tab.doc.base.clone();
                capture.image =
                    std::sync::Arc::new(crate::editor::render::render_to_image(&tab.doc));
                match crate::export::file::save_capture_to(
                    &capture,
                    &path,
                    crate::export::ImageFormat::Png,
                ) {
                    Ok(()) => {
                        tab.saved = true;
                        tab.doc.meta.source_path = Some(path.clone());
                        tab.doc.meta.saved_at = Some(chrono::Local::now());
                        saved_paths.push(path.clone());
                        tracing::info!(file = %crate::paths::file_name(&path), "tab saved");
                    }
                    Err(e) => {
                        tracing::warn!(file = %crate::paths::file_name(&path), "save-all failed: {e}")
                    }
                }
                done.fetch_add(1, Ordering::Relaxed);
                wake.request_repaint();
            }
            let _ = tx.send((tabs, saved_paths));
            wake.request_repaint();
        });
        ctx.repaint_if_live(viewer_id());
        false
    }

    /// Moves the machine to `next` and wakes the anchor so the child
    /// viewports tracking the state are (de)registered promptly. The
    /// repaint is sent even when `next == current`: a same-state entry
    /// still replaces sub-state (e.g. a new capture session over a live
    /// one) that the anchor pass must re-evaluate.
    fn enter(&self, next: AppState, ctx: &egui::Context) {
        let (prev, changed) = {
            let mut state = lock(&self.state);
            let prev = *state;
            *state = next;
            (prev, prev != next)
        };
        if changed {
            tracing::info!(state = ?next, "lifecycle");
            if prev == AppState::Capturing {
                // Leaving the capture kiosk: the real menu bar and Dock
                // slide back in (the overlay hid them — no-op on Windows).
                platform::capture_chrome_exit();
            }
            // The viewer is a real application window: bring it to the
            // front. The bar is a palette — `always_on_top`, never focus.
            if next == AppState::Viewer {
                self.request_focus(viewer_id());
            }
        }
        if next == AppState::Bar {
            // The bar's HWND is destroyed when the viewport deregisters and
            // rebuilt on re-entry — clear the geometry ledger so size and
            // position commands are resent (a stale ledger dedupes them and
            // the new window keeps the builder's clipped default size).
            lock(&self.toolbar).reset_sync();
        }
        // Restoring runs on EVERY viewer entry (guarded to once): a
        // same-state re-entry after reset must not skip the load.
        if next == AppState::Viewer {
            self.restore_session(ctx);
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
        // Queued writes stay ordered — the clear runs behind them.
        tracing::info!("session_discarded");
        self.writer.clear();
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
    /// user deliberately backgrounds is not re-focused forever). Each retry
    /// re-requests a repaint: a single Focus can race a not-yet-created or
    /// still-minimized window, and a sleeping viewport would never retry.
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
            ctx.request_repaint();
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
            | CaptureMode::Pin => {
                // The overlay initially opens on the screen under the cursor
                // (or where the capture was asked for), then follows the
                // cursor across monitors until the user starts a selection.
                let monitor = platform::cursor_position()
                    .and_then(|(x, y)| crate::capture::screen::monitor_at(x, y))
                    .or_else(|| lock(&self.capture_start_monitor).take())
                    .or_else(|| monitor_under(ctx))
                    .or_else(|| crate::capture::screen::primary_monitor().ok());
                let Some(monitor) = monitor else {
                    tracing::error!("cannot enumerate monitors");
                    return;
                };
                // Fixed region recalls ITS memory — the next fixed
                // capture reopens at the same size and position. Every
                // other mode (including Pin) starts blank: the crosshair
                // follows the cursor and the region is drawn by hand.
                let remembered = match mode {
                    CaptureMode::FixedRegion => *lock(&self.last_fixed_region),
                    _ => None,
                };
                let mut session = OverlaySession::new(mode, return_to, remembered);
                session.capture_id = NEXT_CAPTURE_ID.fetch_add(1, Ordering::Relaxed);
                session.monitor = Some(monitor);
                session.include_pointer = lock(&self.settings).include_pointer;
                let _span = session.span().entered();
                tracing::info!(mode = ?mode, "capture_start");
                let instant = session.instant;
                *lock(&self.capture) = Some(session);
                self.enter(AppState::Capturing, ctx);
                if !instant {
                    self.request_focus(overlay_id());
                }
            }
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
        let _span = session.span().entered();
        let capture = if session.instant {
            session.frozen.take().map(|mut c| {
                // Full screen confirms with no selection: stamp the cursor
                // at its freeze-time position (`snap.position`).
                if let Some(snap) = session.cursor.as_ref() {
                    crate::capture::screen::stamp_cursor(
                        std::sync::Arc::make_mut(&mut c.image),
                        &c.monitor.rect,
                        c.scale_factor,
                        snap,
                    );
                }
                // Dev hook: PIXME_DUMP_FINAL=/path.png dumps the stamped image.
                if let Some(path) = std::env::var_os("PIXME_DUMP_FINAL") {
                    let _ = c.image.save(&path);
                }
                c
            })
        } else {
            session.take_capture()
        };
        let return_to = session.return_to;
        let Some(frame) = capture else {
            // Nothing to keep — behave like a cancel.
            tracing::info!(mode = ?session.mode, "capture_empty");
            self.enter(return_to, ctx);
            return;
        };
        tracing::info!(
            mode = ?session.mode,
            w = frame.image.width(),
            h = frame.image.height(),
            ms = session.started_at.elapsed().as_millis() as u64,
            "capture_finished"
        );
        // The gear-menu "copy to clipboard" option applies to every
        // confirmed capture — including Pin captures (which never reach
        // the viewer).
        if lock(&self.settings).copy_to_clipboard
            && let Err(e) = crate::export::clipboard::copy_capture(&frame)
        {
            tracing::warn!("auto clipboard copy failed: {e}");
        }
        // Fixed-region memory: the next fixed capture reopens exactly here
        // (same monitor, same size, same position — monitor-logical space).
        if matches!(session.mode, CaptureMode::FixedRegion) {
            *lock(&self.last_fixed_region) = session.monitor_and_selection();
        }
        // A Pin capture becomes an autonomous floating window — nothing
        // is added to the viewer.
        if matches!(session.mode, CaptureMode::Pin) {
            self.push_pin(frame, &session, ctx);
            self.enter(return_to, ctx);
            return;
        }
        // Open the viewer on the monitor that was just captured.
        *lock(&self.viewer_target_monitor) = Some(frame.monitor.clone());
        let title = {
            let mut viewer = lock(&self.viewer);
            let title = self.capture_title();
            viewer.push_capture_document(Document::new(frame), title.clone());
            title
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
        // The pin's viewport position is in egui points of its own
        // display; `monitor_space_selection` already is in that space,
        // so only the monitor origin needs the native→ui conversion
        // (identity on macOS, ÷scale on Windows).
        let origin_ui = crate::platform::native_to_ui(
            egui::pos2(monitor.x as f32, monitor.y as f32),
            frame.scale_factor,
        );
        let origin = session
            .monitor_space_selection()
            .map(|r| origin_ui + r.min.to_vec2())
            .unwrap_or(origin_ui + egui::vec2(80.0, 80.0));
        let id = {
            let mut n = lock(&self.next_pin_id);
            *n += 1;
            *n
        };
        lock(&self.pins).push(crate::ui::pin::PinWindow::new(id, frame, origin));
        tracing::info!(id, "pin_created");
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// Cancel leaves the session untouched and returns where capture began.
    fn cancel_capture(&self, ctx: &egui::Context) {
        let return_to = {
            let mut slot = lock(&self.capture);
            if let Some(s) = slot.as_ref() {
                let _span = s.span().entered();
                tracing::info!(mode = ?s.mode, "capture_cancelled");
            }
            let return_to = slot.as_ref().map(|s| s.return_to).unwrap_or(AppState::Idle);
            *slot = None;
            return_to
        };
        self.enter(return_to, ctx);
    }

    /// First entry into the viewer: the persisted session loads on a
    /// worker thread so the window paints instantly — image decode is
    /// what takes time, not the queries. The result lands in
    /// `restore_rx`, drained by `pump()`; the viewer paints a spinner
    /// meanwhile. Silent and best-effort (a corrupt database was
    /// already parked as `.bak` when the store opened).
    fn restore_session(&self, _ctx: &egui::Context) {
        let needed = {
            let viewer = lock(&self.viewer);
            !viewer.session_restored() && viewer.tabs.is_empty()
        };
        if !needed {
            return;
        }
        // Claim the restore first — a second viewer entry while the
        // job runs must not queue a duplicate load.
        lock(&self.viewer).start_restore();
        // Retention `Never` reads nothing: skip the job entirely.
        if !self.session_persists() {
            let sort = lock(&self.settings).tab_sort;
            lock(&self.viewer).restore_session(crate::store::StoredSession::default(), sort);
            return;
        }
        // The store thread loads — one connection, PNG decodes off the
        // UI thread, and it replies over the channel the pump drains.
        *lock(&self.restore_rx) = Some(self.writer.load());
    }

    /// Installs the result of the loader thread: the loaded session
    /// swaps into the viewer.
    fn finish_restore(&self, session: crate::store::StoredSession) {
        let sort = lock(&self.settings).tab_sort;
        lock(&self.viewer).restore_session(session, sort);
        // A capture marked saved whose file vanished on disk is not
        // saved anymore: flag it (red dot) and drop the stale path so
        // size/delete never act on a file that isn't there.
        lock(&self.viewer).mark_missing_files_unsaved();
    }

    /// The one-way session drain, run at the tail of every pump:
    ///
    /// 1. Worker events land — a failed write flags its tab (warning
    ///    glyph) and surfaces the one-shot notice; a successful write
    ///    clears the flag.
    /// 2. Closed tabs queue their row deletion.
    /// 3. Any tab whose [`Tab::fingerprint`] drifted from what the
    ///    writer last saw is snapshotted and queued — a single
    ///    detection point covering captures, edits, commits, renames,
    ///    protection, save-state flips and everything added later.
    ///
    /// "Handed to the writer" marks (`stored_fp`, `persisted_rev`,
    /// `base_dirty`) update at enqueue, not at commit: a failed job is
    /// retried from the snapshot it already carries, so the marks must
    /// not lie about needing work.
    fn sync_store(&self, ctx: &egui::Context) {
        use crate::store::writer::WriteEvent;
        for ev in self.writer.drain_events() {
            match ev {
                WriteEvent::DocWritten { id, .. } => {
                    if let Some(tab) = lock(&self.viewer)
                        .tabs
                        .iter_mut()
                        .find(|t| t.doc.meta.id == id)
                    {
                        tab.store_failed = false;
                    }
                }
                WriteEvent::DocFailed { id } => {
                    if let Some(tab) = lock(&self.viewer)
                        .tabs
                        .iter_mut()
                        .find(|t| t.doc.meta.id == id)
                    {
                        tab.store_failed = true;
                    }
                    if !self.store_notice_seen.swap(true, Ordering::Relaxed) {
                        *lock(&self.store_notice) = Some(t!("app.store_write_failed").to_string());
                    }
                    ctx.repaint_if_live(viewer_id());
                }
            }
        }
        // Closed tabs lose their stored rows — a closed capture stays
        // closed at the next launch.
        let closed = lock(&self.viewer).take_closed_ids();
        if !closed.is_empty() {
            tracing::info!(count = closed.len(), "tabs_closed");
            self.writer.delete(closed);
        }
        if self.store_marks_reset.swap(false, Ordering::Relaxed) {
            // Settings applied a pruning retention (or left `Never`) —
            // the store may lack rows the tabs believe written.
            lock(&self.viewer).reset_store_marks();
        }
        if !self.session_persists() || lock(&self.restore_rx).is_some() {
            // `Never` retention (or a session load still decoding): no
            // writes — and NO marks touched, so a later return to
            // `Always` (or the finished load) queues what is needed.
            return;
        }
        let mut viewer = lock(&self.viewer);
        let sigs = viewer.sigs();
        if viewer.persisted_sigs == Some(sigs) {
            return;
        }
        let content_same = viewer.persisted_sigs.is_some_and(|(c, _)| c == sigs.0);
        let meta_same = viewer.persisted_sigs.is_some_and(|(_, m)| m == sigs.1);
        for (ord, tab) in viewer.tabs.iter_mut().enumerate() {
            let fp = tab.fingerprint();
            if tab.stored_fp == Some(fp) {
                continue;
            }
            if tab.persisted_rev == Some(tab.doc.revision) && !tab.base_dirty {
                // Meta-only drift (rename, save flag, protection…):
                // the bulk meta write below already covers it.
                tab.stored_fp = Some(fp);
            } else {
                let write = tab.to_doc_write(ord);
                tab.stored_fp = Some(fp);
                tab.persisted_rev = Some(tab.doc.revision);
                tab.base_dirty = false;
                self.writer.write_doc(write);
            }
        }
        if !content_same {
            // A reorder or a meta-only change touched rows the per-tab
            // pass skipped — refresh every meta column in one job.
            let rows = viewer
                .tabs
                .iter()
                .enumerate()
                .map(|(ord, tab)| tab.to_meta_row(ord))
                .collect();
            self.writer.write_meta(rows);
        }
        if !meta_same {
            self.writer.set_session(viewer.active, viewer.next_title());
        }
        viewer.persisted_sigs = Some(sigs);
    }

    /// `true` when the session is persisted to `session.db` — the
    /// dirty-session warning only makes sense when retention is `Never`
    /// (nothing survives the exit otherwise, so we ask first).
    fn session_persists(&self) -> bool {
        lock(&self.settings).db_retention != crate::config::DbRetention::Never
    }

    /// Queues the quit-time write barrier, then waits — every eager
    /// write is already done or in flight, so the flush only has to
    /// drain the tail of the queue, retry parked failures and refresh
    /// the meta columns. `pump()` closes the app when the barrier
    /// lifts or the timeout expires; the saving veil covers the wait.
    fn quit_now(&self, ctx: &egui::Context) {
        if !self.session_persists() {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
        // A restore still decoding is ahead of every write job — wait
        // for it and install the session first, or its tabs would be
        // finalized as absent and their rows pruned.
        if let Some(rx) = lock(&self.restore_rx).take()
            && let Ok(session) = rx.recv()
        {
            self.finish_restore(session);
        }
        // An open Draw editor still owns its tab's document clone —
        // finalize any open text panel and commit it back so a quit
        // mid-edit loses nothing.
        if let Some(mut draw) = lock(&self.draw).take() {
            draw.finalize_pending();
            lock(&self.viewer).commit_draw(draw.tab, draw.doc, draw.undo, draw.redo);
        }
        // Push whatever changed since the last drain (the draw commit
        // above included), then the barrier: meta refresh + stale-row
        // prune, and the flush that lifts when the queue is empty.
        self.sync_store(ctx);
        let (rows, live, active, next_title) = {
            let viewer = lock(&self.viewer);
            (
                viewer
                    .tabs
                    .iter()
                    .enumerate()
                    .map(|(ord, t)| t.to_meta_row(ord))
                    .collect::<Vec<_>>(),
                viewer
                    .tabs
                    .iter()
                    .map(|t| t.doc.meta.id.clone())
                    .collect::<Vec<_>>(),
                viewer.active_index().unwrap_or(0),
                viewer.next_title(),
            )
        };
        self.writer.finalize(rows, live, active, next_title);
        *lock(&self.quit_rx) = Some(self.writer.flush());
        *lock(&self.quit_started) = Some(std::time::Instant::now());
        ctx.repaint_if_live(viewer_id());
    }
}

/// Returns the first `folder/stem.png` that does not exist, suffixing
/// `_2`, `_3`… on collisions (same-name file already on disk).
fn unique_png(folder: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let candidate = folder.join(format!("{stem}.png"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2u32.. {
        let path = folder.join(format!("{stem}_{n}.png"));
        if !path.exists() {
            return path;
        }
    }
    unreachable!()
}

/// File extension → save dialog format (`None` keeps the default PNG).
fn ext_format(path: &std::path::Path) -> Option<crate::export::ImageFormat> {
    match path.extension()?.to_str()?.to_lowercase().as_str() {
        "png" => Some(crate::export::ImageFormat::Png),
        "jpg" | "jpeg" => Some(crate::export::ImageFormat::Jpeg),
        "webp" => Some(crate::export::ImageFormat::Webp),
        _ => None,
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
        image: std::sync::Arc::new(image),
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

/// The monitor under the window dispatching a capture — its centre hit
/// against `MonitorInfo::rect`. The invisible 1×1 anchor (resident-icon
/// launches, delayed `CaptureNow`) returns `None`: there is no window to
/// anchor the overlay to.
///
/// Coordinate space: `outer_rect` is in "ui points of the window's
/// display" while `MonitorInfo::rect` is in the OS-native global space;
/// [`platform::ui_to_native`] bridges the two (identity on macOS, ×scale
/// on Windows).
fn monitor_under(ctx: &egui::Context) -> Option<MonitorInfo> {
    if ctx.viewport_id() == egui::ViewportId::ROOT {
        return None;
    }
    ctx.input(|i| {
        let vp = i.viewport();
        match (vp.outer_rect, vp.native_pixels_per_point) {
            (Some(r), Some(s)) if s > 0.0 => Some(platform::ui_to_native(r.center(), s)),
            _ => None,
        }
    })
    .and_then(|c| crate::capture::screen::monitor_at(c.x, c.y))
}

/// Whether `pos` (egui ui points) lands on `anchor`'s monitor, hit-tested
/// against `monitors` — each candidate converts `pos` with its own scale
/// (mixed-DPI safe). Monitors compare by rect, not id: xcap ids are not
/// stable across enumerations on Windows.
fn ui_pos_on_anchor(pos: egui::Pos2, monitors: &[MonitorInfo], anchor: &MonitorInfo) -> bool {
    monitors
        .iter()
        .find(|m| {
            let scale = m.scale_factor.max(1.0);
            let native = platform::ui_to_native(pos, scale);
            let r = &m.rect;
            native.x >= r.x as f32
                && native.x < r.x as f32 + r.width as f32
                && native.y >= r.y as f32
                && native.y < r.y as f32 + r.height as f32
        })
        .is_some_and(|m| m.rect == anchor.rect)
}

/// Root application state, created once in [`eframe::App::ui`]'s setup.
pub struct PixMeApp {
    /// Everything the deferred-viewport callbacks reach.
    shared: Arc<Shared>,
}

/// The `startup` event — the context every diagnosis needs first:
/// version, build, OS, screens, language, theme, log level, the dirs in
/// use and the macOS Screen Recording permission state.
fn log_banner(settings: &Settings) {
    let monitors = crate::capture::screen::monitors().unwrap_or_default();
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        commit = env!("PIXME_GIT_HASH"),
        os = %platform::os_version(),
        arch = std::env::consts::ARCH,
        screens = monitors.len(),
        language = ?settings.language,
        appearance = ?settings.appearance,
        log_level = %crate::logging::base_directive(),
        data_dir = %crate::paths::data_dir().display(),
        logs_dir = %crate::paths::logs_dir().display(),
        fallback = crate::paths::fallback_from().is_some(),
        screen_recording = ?platform::current().capture_permission_status(),
        "startup"
    );
    for m in &monitors {
        tracing::info!(
            id = m.id,
            w = m.rect.width,
            h = m.rect.height,
            scale = m.scale_factor,
            primary = m.is_primary,
            "monitor"
        );
    }
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
        log_banner(&settings);
        // Dev/test hook: `PIXME_CAPTURE=region|fullscreen|fixed` fires that
        // capture at startup without going through the resident menu.
        let debug_capture = std::env::var("PIXME_CAPTURE")
            .ok()
            .and_then(|v| debug_mode(&v));
        let app = Self {
            shared: Arc::new(Shared {
                state: Mutex::new(AppState::Idle),
                last_language: Mutex::new(settings.language),
                last_menu_sync: Mutex::new((ModalState::None, false, false, false)),
                viewer_dialog_open: AtomicBool::new(false),
                settings: Mutex::new(settings),
                viewer: Mutex::new(Viewer::default()),
                viewer_rect: Mutex::new(None),
                viewer_target_monitor: Mutex::new(None),
                toolbar: Mutex::new(Toolbar::default()),
                bar_pos: Mutex::new(None),
                bar_rect: Mutex::new(None),
                bar_monitor: Mutex::new(None),
                viewer_monitor: Mutex::new(None),
                winit_placements: Mutex::new(Vec::new()),
                settings_open: AtomicBool::new(false),
                about_open: AtomicBool::new(false),
                settings_draft: Mutex::new(None),
                capture: Mutex::new(None),
                countdown: Mutex::new(None),
                last_fixed_region: Mutex::new(None),
                capture_start_monitor: Mutex::new(None),
                pins: Mutex::new(Vec::new()),
                next_pin_id: Mutex::new(0),
                save_dialog: Mutex::new(None),
                draw: Mutex::new(None),
                writer: {
                    // The store thread owns the database for the whole
                    // run — retention rides the queue first so pruning
                    // always lands before the session load.
                    let ctx = cc.egui_ctx.clone();
                    let writer = crate::store::writer::Writer::spawn(
                        crate::store::Store::open(),
                        Some(std::sync::Arc::new(move || wake_viewports(&ctx))),
                    );
                    writer.retention(db_retention);
                    writer
                },
                restore_rx: Mutex::new(None),
                quit_rx: Mutex::new(None),
                quit_started: Mutex::new(None),
                save_all: Mutex::new(None),
                save_one: Mutex::new(None),
                store_notice: Mutex::new(None),
                store_notice_seen: AtomicBool::new(false),
                store_marks_reset: AtomicBool::new(false),
                pending_focus: Mutex::new(Vec::new()),
                pending: Mutex::new(Vec::new()),
                // Windows portable fallback: the one translated notice,
                // shown once the first window exists (i18n already ran).
                startup_notice: Mutex::new(crate::paths::fallback_from().map(|attempted| {
                    t!(
                        "app.fallback_notice",
                        attempted = attempted.display().to_string(),
                        used = crate::paths::data_dir().display().to_string()
                    )
                    .to_string()
                })),
            }),
        };
        // Launch lands on the viewer, not resident-idle: it restores
        // the persisted session and gives a visible landing point.
        // Any viewport left open by eframe's persisted state (e.g. the
        // capture bar showing at a previous quit) is closed explicitly.
        app.shared.enter(AppState::Viewer, &cc.egui_ctx);
        cc.egui_ctx
            .cmd_if_live(bar_id(), egui::ViewportCommand::Close);
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
    fn ui(&mut self, _ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
                    if ctx.input(|i| i.key_pressed(egui::Key::Escape))
                        && lock(&shared.countdown).is_some()
                    {
                        *lock(&shared.countdown) = None;
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                    let mut toolbar = lock(&shared.toolbar);
                    let mut settings = lock(&shared.settings);
                    toolbar.show(ui, &mut settings, &shared);
                });
            }
            AppState::Viewer => {
                let shared = Arc::clone(&self.shared);
                // If a capture just landed on a specific monitor, make sure
                // the viewer opens on that monitor (preserving its relative
                // placement / size when already known, or centring it there).
                if let Some(m) = lock(&shared.viewer_target_monitor).clone() {
                    // The window is being (re)built ON m: its ppp is m's
                    // own scale.
                    let next = place_viewer_on_monitor(
                        *lock(&shared.viewer_rect),
                        &m,
                        m.scale_factor.max(1.0),
                    );
                    *lock(&shared.viewer_rect) = Some(next);
                }
                let builder = viewer_builder(lock(&shared.viewer_rect).map(|r| r.min));
                ctx.show_viewport_deferred(viewer_id(), builder, move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, viewer_id());
                    let target = lock(&shared.viewer_target_monitor).take();
                    if let Some(m) = target {
                        // `OuterPosition` is converted back by THIS
                        // window's current ppp — convert with it so the
                        // physical target is exact even mid-DPI-change.
                        let ppp = platform::window_ppp(&ctx, m.scale_factor.max(1.0));
                        let next =
                            place_viewer_on_monitor(*lock(&shared.viewer_rect), &m, ppp);
                        *lock(&shared.viewer_rect) = Some(next);
                        *lock(&shared.viewer_monitor) = Some(m);
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(next.min));
                    } else if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                        *lock(&shared.viewer_rect) = Some(outer);
                        let ppp = platform::window_ppp(&ctx, 1.0);
                        let centre = platform::ui_to_native(outer.center(), ppp);
                        *lock(&shared.viewer_monitor) =
                            crate::capture::screen::monitor_at(centre.x, centre.y);
                    }
                    // Draw is modal to the viewer: deferred viewports are
                    // plain top-level windows with no OS child-window
                    // modality. Clicking the modal's parent (the dim veil
                    // eats it) brings the editor back in front — one
                    // command per click, no per-frame focus fight.
                    if lock(&shared.draw).is_some() && ctx.input(|i| i.pointer.any_pressed()) {
                        ctx.cmd_if_live(draw_id(), egui::ViewportCommand::Focus);
                    }
                    ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                    lock(&shared.viewer).show(ui, &shared);
                    // One-shot store-failure notice (translated): a write
                    // failed on the worker — the per-tab warning glyph is
                    // the standing signal; this explains it once.
                    notice_window(&ctx, "pixme.store_notice", &shared.store_notice);
                    // Deferred startup notice (Windows data/logs fallback):
                    // the first real window delivers it — non-modal, the
                    // app stays usable.
                    notice_window(&ctx, "pixme.startup_notice", &shared.startup_notice);
                    // Windows About modal — the egui twin of the macOS
                    // native About panel, framed like the settings modal:
                    // title band with a painted close cross, compact body.
                    if shared.about_open.load(Ordering::Relaxed) {
                        let mut open = true;
                        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                            open = false;
                        }
                        egui::Area::new(egui::Id::new("pixme.about_modal"))
                            .order(egui::Order::Foreground)
                            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                            .show(&ctx, |ui| {
                                let t = crate::ui::theme::tokens(&ctx);
                                egui::Frame::default()
                                    .fill(t.dock)
                                    .stroke(egui::Stroke::new(1.0, t.card_border))
                                    .corner_radius(egui::CornerRadius::same(t.radius_lg))
                                    .show(ui, |ui| {
                                        ui.set_width(300.0);
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        // Title band: "À propos" + close cross.
                                        egui::Frame::default()
                                            .inner_margin(egui::Margin::symmetric(10, 6))
                                            .fill(t.card)
                                            .corner_radius(egui::CornerRadius {
                                                nw: t.radius_lg,
                                                ne: t.radius_lg,
                                                sw: 0,
                                                se: 0,
                                            })
                                            .show(ui, |ui| {
                                                ui.horizontal(|ui| {
                                                    ui.label(
                                                        egui::RichText::new(
                                                            t!("menu.help.about").to_string(),
                                                        )
                                                        .color(t.text)
                                                        .strong(),
                                                    );
                                                    ui.with_layout(
                                                        egui::Layout::right_to_left(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            let (rect, resp) = ui
                                                                .allocate_exact_size(
                                                                    egui::Vec2::splat(20.0),
                                                                    egui::Sense::click(),
                                                                );
                                                            if ui.is_rect_visible(rect) {
                                                                let hovered = resp.hovered();
                                                                if hovered {
                                                                    let fill = egui::Color32::from_rgba_unmultiplied(
                                                                        t.danger.r(),
                                                                        t.danger.g(),
                                                                        t.danger.b(),
                                                                        34,
                                                                    );
                                                                    ui.painter().rect_filled(
                                                                        rect,
                                                                        egui::CornerRadius::same(t.radius_sm),
                                                                        fill,
                                                                    );
                                                                }
                                                                crate::ui::icons::paint(
                                                                    ui,
                                                                    rect,
                                                                    crate::ui::icons::Icon::X,
                                                                    if hovered { 14.0 } else { 12.0 },
                                                                    if hovered { t.danger } else { t.icon },
                                                                );
                                                            }
                                                            if resp.clicked() {
                                                                open = false;
                                                            }
                                                        },
                                                    );
                                                });
                                            });
                                        egui::Frame::default()
                                            .inner_margin(egui::Margin::same(16))
                                            .show(ui, |ui| {
                                                ui.vertical_centered(|ui| {
                                                    // Full-bleed 256 artwork
                                                    // (the window icon) — the
                                                    // 1024 dock master carries
                                                    // margins + shadow and reads
                                                    // as "no icon" at 64 px.
                                                    let icon_rgba =
                                                        crate::ui::icons::app_icon_rgba(128);
                                                    let icon_img =
                                                        egui::ColorImage::from_rgba_unmultiplied(
                                                            [128, 128],
                                                            &icon_rgba,
                                                        );
                                                    let icon_tex = ctx.load_texture(
                                                        "pixme://about-icon",
                                                        icon_img,
                                                        egui::TextureOptions::LINEAR,
                                                    );
                                                    ui.image(egui::load::SizedTexture::new(
                                                        icon_tex.id(),
                                                        egui::vec2(64.0, 64.0),
                                                    ));
                                                    ui.add_space(6.0);
                                                    ui.label(
                                                        egui::RichText::new("PixMe").font(
                                                            crate::ui::theme::semibold(17.0),
                                                        ),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(format!(
                                                            "v{}",
                                                            env!("CARGO_PKG_VERSION")
                                                        ))
                                                        .size(11.0),
                                                    );
                                                    ui.add_space(8.0);
                                                    ui.label(
                                                        egui::RichText::new(
                                                            t!("settings.about.copyright")
                                                                .to_string(),
                                                        )
                                                        .size(10.5),
                                                    );
                                                    ui.label(
                                                        egui::RichText::new(
                                                            t!("settings.about.license").to_string(),
                                                        )
                                                        .size(10.5),
                                                    );
                                                });
                                                ui.add_space(10.0);
                                                ui.with_layout(
                                                    egui::Layout::right_to_left(
                                                        egui::Align::Center,
                                                    ),
                                                    |ui| {
                                                        if crate::ui::widgets::primary_button(
                                                            ui,
                                                            "OK",
                                                        )
                                                        .clicked()
                                                        {
                                                            open = false;
                                                        }
                                                    },
                                                );
                                            });
                                    });
                            });
                        if !open {
                            shared.about_open.store(false, Ordering::Relaxed);
                        }
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
                        // winit monitor placements (physical px, both
                        // OSes): lets `with_monitor`/`SetMonitor` target
                        // the captured display — xcap and winit do not
                        // enumerate in the same order.
                        let placements: Vec<(i32, i32, u32, u32)> = frame
                            .winit_window()
                            .map(|w| {
                                w.available_monitors()
                                    .map(|m| {
                                        let p = m.position();
                                        let s = m.size();
                                        (p.x, p.y, s.width, s.height)
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        *lock(&shared.winit_placements) = placements.clone();
                        let monitor = lock(&shared.capture)
                            .as_ref()
                            .and_then(|s| s.monitor.clone());
                        let winit_idx = monitor
                            .as_ref()
                            .and_then(|m| platform::winit_monitor_index(m, &placements, 1));
                        ctx.show_viewport_deferred(
                            overlay_id(),
                            overlay_builder(monitor.as_ref(), winit_idx),
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

        // The capture-delay countdown: like the pins it lives outside the
        // state machine — it can run on top of the bar, the viewer or an
        // empty desktop. It repaints itself to zero, then fires CaptureNow.
        if lock(&self.shared.countdown).is_some() {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(countdown_id(), countdown_builder(), move |ui, _class| {
                let ctx = ui.ctx().clone();
                shared.pump(&ctx);
                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                // Esc cancels the pending capture.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    *lock(&shared.countdown) = None;
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }
                // The countdown parks on the monitor PixMe lives on: the
                // floating bar's when it is up, the viewer's otherwise —
                // resolved in native units by the owner window, so no two
                // windows' point spaces are ever mixed here.
                let anchor = if *lock(&shared.state) == AppState::Bar {
                    lock(&shared.bar_monitor).clone()
                } else {
                    lock(&shared.viewer_monitor).clone()
                };
                let running = lock(&shared.countdown)
                    .as_mut()
                    .is_some_and(|c| c.show(ui, anchor));
                if !running && let Some(cd) = lock(&shared.countdown).take() {
                    shared.push(Command::CaptureNow(cd.mode));
                    // Wake the anchor pass: it unregisters this viewport
                    // and dispatches the pending capture.
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                }
            });
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
                    && let Some(mut session) = lock(&shared.draw).take()
                {
                    tracing::info!("draw_closed");
                    // The modal closed: the viewer gets focus back.
                    ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Focus);
                    if outcome == crate::ui::draw::Outcome::Ok {
                        tracing::info!("draw_committed");
                        session.finalize_pending();
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
                    let saving = lock(&shared.save_dialog).as_ref().is_some_and(|d| d.saving);
                    if ctx.input(|i| i.viewport().close_requested()) {
                        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                        if !saving {
                            close(&ctx);
                        }
                        return;
                    }
                    if !saving && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close(&ctx);
                        return;
                    }
                    let mut guard = lock(&shared.save_dialog);
                    if let Some(dialog) = guard.as_mut() {
                        match ui::save_dialog::show(ui, dialog) {
                            ui::save_dialog::Outcome::Confirm => {
                                let dir = std::path::PathBuf::from(dialog.folder.trim());
                                if !dir.is_dir() {
                                    dialog.error = Some(t!("save.error_dir").into_owned());
                                } else {
                                    // The encode+write rides a worker —
                                    // a 5K PNG must not stall a frame.
                                    // `pump` lands the result (mark_saved,
                                    // recent files, close) or hands the
                                    // error back to the dialog.
                                    let path = dialog.candidate_path();
                                    let capture = std::sync::Arc::clone(&dialog.capture);
                                    let format = dialog.format;
                                    let (tx, rx) = std::sync::mpsc::channel();
                                    let wake = ctx.clone();
                                    std::thread::spawn(move || {
                                        let res = crate::export::file::save_capture_to(
                                            &capture, &path, format,
                                        );
                                        let _ = tx.send(res);
                                        wake_viewports(&wake);
                                    });
                                    dialog.saving = true;
                                    *lock(&shared.save_one) = Some(SaveOneJob {
                                        rx,
                                        path: dialog.candidate_path(),
                                        dir,
                                        viewer_tab: dialog.viewer_tab,
                                        reveal: dialog.reveal,
                                    });
                                }
                            }
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

/// One-shot non-modal notice window (store failure or Windows
/// portable fallback): distinct egui ID, translated OK button.
fn notice_window(ctx: &egui::Context, id: &'static str, slot: &Mutex<Option<String>>) {
    if lock(slot).is_none() {
        return;
    }
    let mut open = true;
    egui::Window::new(t!("app.title").to_string())
        .id(egui::Id::new(id))
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .open(&mut open)
        .show(ctx, |ui| {
            if let Some(msg) = &*lock(slot) {
                ui.label(msg);
            }
            ui.add_space(6.0);
            if ui::widgets::primary_button(ui, &t!("draw.ok")).clicked() {
                *lock(slot) = None;
            }
        });
    if !open {
        *lock(slot) = None;
    }
}

/// The floating bar: borderless, transparent, always on top, sized to its
/// content; position restored across hide/show cycles.
fn bar_builder(position: Option<egui::Pos2>) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title("PixMe Bar")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        // Accessory apps stay inert until a window asks for focus.
        .with_active(true)
        .with_inner_size([340.0, 48.0]);
    #[cfg(target_os = "windows")]
    // winit forces WS_CAPTION at creation — created hidden; the pump's
    // `strip_undecorated_frame` ShowWindow's it once chrome-free (egui's
    // `Visible(true)` would re-apply the caption via winit's apply_diff).
    let builder = builder.with_visible(false);
    match position {
        Some(pos) => builder.with_position(pos),
        None => builder,
    }
}

/// The viewer window: decorated, resizable, the app's real home.
fn viewer_builder(position: Option<egui::Pos2>) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(t!("app.title"))
        .with_icon(ui::icons::app_icon_data())
        .with_app_id("pixme")
        .with_inner_size([860.0, 560.0])
        .with_min_inner_size([420.0, 280.0])
        .with_active(true);
    match position {
        Some(pos) => builder.with_position(pos),
        None => builder,
    }
}

/// Computes the viewer's outer rectangle on `monitor` (in egui ui points
/// of a window whose `native_pixels_per_point` is `ppp` — the receiving
/// window's OWN scale, never the monitor's: egui-winit multiplies
/// `OuterPosition` back by that scale, so this lands on exact physical
/// coordinates). If `current` is already on `monitor` (its centre falls
/// inside `monitor`'s bounds), it is kept as-is; otherwise the window is
/// centred on `monitor` (clamped inside it).
fn place_viewer_on_monitor(
    current: Option<egui::Rect>,
    monitor: &MonitorInfo,
    ppp: f32,
) -> egui::Rect {
    let mon_rect = platform::monitor_ui_rect(monitor, ppp);
    let mon_min = mon_rect.min;
    let mon_size = mon_rect.size();
    if let Some(rect) = current
        && mon_rect.contains(rect.center())
    {
        return rect;
    }
    let size = current
        .map(|r| r.size())
        .unwrap_or(egui::vec2(860.0, 588.0))
        .min(mon_size);
    let pos = egui::pos2(
        mon_min.x + ((mon_size.x - size.x) * 0.5).max(0.0),
        mon_min.y + ((mon_size.y - size.y) * 0.5).max(0.0),
    );
    egui::Rect::from_min_size(pos, size)
}

/// The capture overlay: a borderless always-on-top window covering the
/// monitor being captured. `MonitorInfo.rect` is in the OS-native display
/// space (physical px on Windows, points on macOS) — it is converted to
/// that monitor's logical units via `native_to_ui` for the position/size
/// *fallback*. On Windows the real placement is `with_monitor(idx)`
/// (borderless fullscreen resolved by winit, immune to every scale
/// assumption); macOS keeps the manual cover (its Fullscreen API would
/// animate a new Space — the points-based rect is already exact there).
fn overlay_builder(
    monitor: Option<&crate::capture::MonitorInfo>,
    winit_idx: Option<usize>,
) -> egui::ViewportBuilder {
    let (size, pos) = monitor
        .map(|m| {
            let s = m.scale_factor.max(1.0);
            let min = platform::native_to_ui(egui::pos2(m.rect.x as f32, m.rect.y as f32), s);
            let sz =
                platform::native_to_ui(egui::pos2(m.rect.width as f32, m.rect.height as f32), s)
                    .to_vec2();
            (sz, min)
        })
        .unwrap_or((egui::vec2(800.0, 600.0), egui::pos2(0.0, 0.0)));
    let builder = egui::ViewportBuilder::default()
        // Unique title: Windows' strip_undecorated_frame keys its chrome
        // strip on it ("PixMe" is already the invisible anchor's title —
        // and clearing a taskbar-bearing title would blank its button).
        .with_title("PixMe Overlay")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(true)
        .with_inner_size(size)
        .with_position(pos);
    #[cfg(target_os = "windows")]
    let builder = {
        let builder = builder.with_visible(false);
        match winit_idx {
            // winit places the window: borderless fullscreen on the
            // captured monitor, physical-rect exact, no manual geometry.
            Some(idx) => builder.with_monitor(idx),
            None => builder,
        }
    };
    let _ = winit_idx;
    builder
}

/// A pinned capture: borderless, transparent, always on top, not a
/// taskbar entry — the image itself is the chrome; hover reveals the
/// close dot and the control strip. The builder position/size only apply
/// at creation: user drags and zoom resizes use viewport commands.
fn pin_builder(origin: egui::Pos2, size: egui::Vec2) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title("PixMe Pin")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(true)
        .with_taskbar(false)
        .with_active(true)
        // TOP_MARGIN transparent rows above the image: the Windows
        // region clip removes the DWM accent line without cropping it.
        .with_inner_size(size + egui::vec2(0.0, crate::ui::pin::TOP_MARGIN))
        .with_position(origin - egui::vec2(0.0, crate::ui::pin::TOP_MARGIN));
    #[cfg(target_os = "windows")]
    // Hidden until `strip_undecorated_frame` shows it (see `bar_builder`).
    let builder = builder.with_visible(false);
    builder
}

/// The capture-delay countdown: borderless, transparent, always on top,
/// sized to its painted card. The position only applies at creation —
/// the card then tracks the monitor under the cursor.
fn countdown_builder() -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title("PixMe Countdown")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(false)
        .with_inner_size([140.0, 140.0]);
    #[cfg(target_os = "windows")]
    // Hidden until `strip_undecorated_frame` shows it (see `bar_builder`).
    let builder = builder.with_visible(false);
    builder
}

/// The pin save-as window: a small decorated form floating above the
/// (always-on-top) pins it saves, not a taskbar entry.
fn save_dialog_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title(t!("save.title"))
        .with_icon(ui::icons::app_icon_data())
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
        .with_icon(ui::icons::app_icon_data())
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

    /// Windows: every chrome-less viewport starts hidden — winit forces
    /// `WS_CAPTION` at creation and a window shown before
    /// `strip_undecorated_frame` ran flashes a title bar. Decorated
    /// windows keep the default (created visible).
    #[cfg(target_os = "windows")]
    #[test]
    fn chromeless_viewports_start_hidden() {
        assert_eq!(bar_builder(None).visible, Some(false));
        assert_eq!(
            pin_builder(egui::pos2(0.0, 0.0), egui::vec2(10.0, 10.0)).visible,
            Some(false)
        );
        assert_eq!(countdown_builder().visible, Some(false));
        assert_eq!(overlay_builder(None, None).visible, Some(false));
        assert_eq!(viewer_builder(None).visible, None);
        assert_eq!(save_dialog_builder().visible, None);
        assert_eq!(draw_builder("t").visible, None);
    }

    fn harness() -> (Shared, egui::Context) {
        let shared = Shared {
            state: Mutex::new(AppState::Bar),
            settings: Mutex::new(Settings::default()),
            viewer: Mutex::new(Viewer::default()),
            viewer_rect: Mutex::new(None),
            viewer_target_monitor: Mutex::new(None),
            toolbar: Mutex::new(Toolbar::default()),
            bar_pos: Mutex::new(None),
            bar_rect: Mutex::new(None),
            bar_monitor: Mutex::new(None),
            viewer_monitor: Mutex::new(None),
            winit_placements: Mutex::new(Vec::new()),
            settings_open: AtomicBool::new(false),
            about_open: AtomicBool::new(false),
            settings_draft: Mutex::new(None),
            capture: Mutex::new(None),
            countdown: Mutex::new(None),
            last_fixed_region: Mutex::new(None),
            capture_start_monitor: Mutex::new(None),
            pins: Mutex::new(Vec::new()),
            next_pin_id: Mutex::new(0),
            save_dialog: Mutex::new(None),
            draw: Mutex::new(None),
            writer: crate::store::writer::Writer::spawn(
                crate::store::Store::open_in_memory(),
                None,
            ),
            restore_rx: Mutex::new(None),
            quit_rx: Mutex::new(None),
            quit_started: Mutex::new(None),
            save_all: Mutex::new(None),
            save_one: Mutex::new(None),
            store_notice: Mutex::new(None),
            store_notice_seen: AtomicBool::new(false),
            store_marks_reset: AtomicBool::new(false),
            pending_focus: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            last_language: Mutex::new(Language::Auto),
            last_menu_sync: Mutex::new((ModalState::None, false, false, false)),
            viewer_dialog_open: AtomicBool::new(false),
            startup_notice: Mutex::new(None),
        };
        (shared, egui::Context::default())
    }

    fn drive(shared: &Shared, ctx: &egui::Context, command: Command) -> AppState {
        shared.push(command);
        shared.pump(ctx);
        *lock(&shared.state)
    }

    /// Blocks on the store thread's load and installs its result — the
    /// synchronous twin of the drain `pump()` does each frame.
    fn drain_restore(shared: &Shared) {
        let rx = lock(&shared.restore_rx).take();
        if let Some(rx) = rx
            && let Ok(session) = rx.recv()
        {
            shared.finish_restore(session);
        }
    }

    /// Blocks on the quit-time flush — the synchronous twin of the
    /// drain `pump()` does each frame.
    fn drain_quit(shared: &Shared) {
        let rx = lock(&shared.quit_rx).take();
        if let Some(rx) = rx {
            let _ = rx.recv();
        }
    }

    /// Blocks on the save-all worker and reattaches the tabs — the
    /// synchronous twin of the `save_all` drain `pump()` does per frame.
    fn drain_save_all(shared: &Shared) {
        let Some(job) = lock(&shared.save_all).take() else {
            return;
        };
        if let Ok((tabs, _)) = job.rx.recv() {
            let mut viewer = lock(&shared.viewer);
            viewer.tabs = tabs;
            viewer.recompute_dirty();
        }
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
        drain_quit(&shared);
        // New run: fresh viewer state, same store.
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
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
        drain_quit(&shared);
        assert!(
            !lock(&shared.viewer).confirm_close,
            "no dialog when persisting"
        );
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
    }

    /// A quit that changed nothing queues no document write — the
    /// fingerprint check short-circuits before any row work, and the
    /// flush barrier still lands.
    #[test]
    fn unchanged_quit_writes_nothing() {
        let (shared, ctx) = harness();
        // Restore runs first so the persisted baseline exists.
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        drive(&shared, &ctx, Command::Quit);
        assert!(
            lock(&shared.quit_rx).is_some(),
            "quit always flushes the writer queue"
        );
        drain_quit(&shared);
        assert!(
            shared.writer.drain_events().is_empty(),
            "unchanged quit must not rewrite any document"
        );
    }

    /// A quit landing while the store thread is still loading must
    /// wait for the session first — otherwise the load's tabs would be
    /// finalized as absent and their rows pruned.
    #[test]
    fn quit_drains_an_in_flight_restore() {
        let (shared, ctx) = harness();
        // Seed the shared store, then simulate a finished load whose
        // result pump() has not drained yet.
        let mut v = Viewer::default();
        v.push_stub("from_db");
        shared.writer.write_doc(v.tabs[0].to_doc_write(0));
        shared.writer.flush().recv().expect("seed write");
        let session = shared.writer.load().recv().expect("load reply");
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(session).unwrap();
        *lock(&shared.restore_rx) = Some(rx);

        drive(&shared, &ctx, Command::Quit);
        drain_quit(&shared);
        assert!(
            lock(&shared.restore_rx).is_none(),
            "pending restore drained at quit"
        );

        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
        assert_eq!(lock(&shared.viewer).tabs[0].title, "from_db");
    }

    /// A real change still takes the worker path — the store goes away
    /// and comes back through the quit channel.
    #[test]
    fn changed_quit_saves_on_a_thread() {
        let (shared, ctx) = harness();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        lock(&shared.viewer).push_stub("capture 1");
        drive(&shared, &ctx, Command::Quit);
        assert!(
            lock(&shared.quit_rx).is_some(),
            "changed quit must spawn the save thread"
        );
        drain_quit(&shared);
        *lock(&shared.viewer) = Viewer::default();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
    }

    /// When retention empties the store (`Never`) and later comes back
    /// (`Always`), `store_marks_reset` forces `sync_store` to re-queue
    /// the full document (base pixels included), not a meta-only UPDATE.
    #[test]
    fn reset_store_marks_rewrites_full_documents_after_never() {
        let (shared, ctx) = harness();
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        lock(&shared.viewer).push_stub("capture 1");
        shared.pump(&ctx);
        shared.writer.flush().recv().expect("initial write");
        assert_eq!(shared.writer.document_count(), 1);

        // Simulate switching retention to Never (clears DB), then back
        // to Always with `store_marks_reset` set.
        shared.writer.retention(crate::config::DbRetention::Never);
        shared.writer.flush().recv().expect("clear");
        assert_eq!(shared.writer.document_count(), 0);

        shared.store_marks_reset.store(true, Ordering::Relaxed);
        shared.pump(&ctx);
        shared.writer.flush().recv().expect("rewrite");
        assert_eq!(shared.writer.document_count(), 1);
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

    /// Every `Command` variant, one instance each. `is_command_enabled`
    /// matches exhaustively (no wildcard arm), so a new variant added here
    /// without a ruling in the table fails to compile — and forgetting to
    /// add the variant to this list fails this test's intent: update BOTH.
    fn all_commands() -> Vec<Command> {
        vec![
            Command::Capture(CaptureMode::Region),
            Command::CaptureNow(CaptureMode::Region),
            Command::ConfirmCapture,
            Command::CancelCapture,
            Command::ShowBar,
            Command::OpenViewer,
            Command::CloseBar,
            Command::ViewerClosed,
            Command::OpenSettings,
            Command::CloseSettings,
            Command::Quit,
            Command::ImportClipboard,
            Command::OpenFile,
            Command::OpenPath(std::path::PathBuf::new()),
            Command::OpenAllRecent,
            Command::ClearRecent,
            Command::OpenLastDir,
            Command::NewDocument,
            Command::SaveActive,
            Command::SaveAll,
            Command::RenameActive,
            Command::CopyActive,
            Command::OpenDraw,
            Command::UndoActive,
            Command::RedoActive,
            Command::PrintActive,
            Command::ConfirmSave,
            Command::ConfirmDiscard,
            Command::ConfirmCancel,
            Command::ClosePin(0),
            Command::CopyPin(0),
            Command::SavePin(0),
        ]
    }

    /// Nothing is blocked when no modal is open.
    #[test]
    fn all_commands_enabled_without_modal() {
        for command in all_commands() {
            assert!(
                is_command_enabled(&command, ModalState::None),
                "{command:?} must run with no modal"
            );
        }
    }

    /// The modal × command table. `(command, Settings, SaveDialog, Draw)`
    /// — one row per variant of `all_commands()`.
    #[test]
    fn modal_command_table() {
        // (Command, Settings?, SaveDialog?, ViewerDialog?, Draw?)
        let table: &[(Command, bool, bool, bool, bool)] = &[
            (
                Command::Capture(CaptureMode::Region),
                false,
                false,
                false,
                false,
            ),
            (
                Command::CaptureNow(CaptureMode::Pin),
                false,
                false,
                false,
                false,
            ),
            (Command::ConfirmCapture, true, true, true, true),
            (Command::CancelCapture, true, true, true, true),
            (Command::ShowBar, false, false, false, false),
            (Command::OpenViewer, true, false, false, false),
            (Command::CloseBar, true, true, true, true),
            (Command::ViewerClosed, true, true, true, true),
            (Command::OpenSettings, true, false, false, false),
            (Command::CloseSettings, true, true, true, true),
            (Command::Quit, true, true, true, true),
            (Command::ImportClipboard, false, false, false, false),
            (Command::OpenFile, false, false, false, false),
            (
                Command::OpenPath(std::path::PathBuf::new()),
                false,
                false,
                false,
                false,
            ),
            (Command::OpenAllRecent, false, false, false, false),
            (Command::ClearRecent, false, false, false, false),
            (Command::OpenLastDir, false, false, false, false),
            (Command::NewDocument, false, false, false, false),
            (Command::SaveActive, false, false, false, false),
            (Command::SaveAll, false, false, false, false),
            (Command::RenameActive, false, false, false, false),
            (Command::CopyActive, false, false, false, true),
            (Command::OpenDraw, false, false, false, false),
            (Command::UndoActive, false, false, false, true),
            (Command::RedoActive, false, false, false, true),
            (Command::PrintActive, false, false, false, false),
            (Command::ConfirmSave, true, true, true, true),
            (Command::ConfirmDiscard, true, true, true, true),
            (Command::ConfirmCancel, true, true, true, true),
            (Command::ClosePin(1), true, true, true, true),
            (Command::CopyPin(1), true, true, true, true),
            (Command::SavePin(1), false, false, false, false),
        ];
        for (command, settings, save_dialog, viewer_dialog, draw) in table {
            assert_eq!(
                is_command_enabled(command, ModalState::Settings),
                *settings,
                "Settings: {command:?}"
            );
            assert_eq!(
                is_command_enabled(command, ModalState::SaveDialog),
                *save_dialog,
                "SaveDialog: {command:?}"
            );
            assert_eq!(
                is_command_enabled(command, ModalState::ViewerDialog),
                *viewer_dialog,
                "ViewerDialog: {command:?}"
            );
            assert_eq!(
                is_command_enabled(command, ModalState::Draw),
                *draw,
                "Draw: {command:?}"
            );
        }
        // A committed quit freezes everything — even Quit itself.
        for command in all_commands() {
            assert!(
                !is_command_enabled(&command, ModalState::Saving),
                "Saving: {command:?}"
            );
        }
        // While a capture owns the screen, only the always-live commands
        // pass — in particular a second Capture can never start.
        for command in all_commands() {
            let expected = matches!(
                command,
                Command::Quit
                    | Command::CloseSettings
                    | Command::ConfirmSave
                    | Command::ConfirmDiscard
                    | Command::ConfirmCancel
                    | Command::ConfirmCapture
                    | Command::CancelCapture
                    | Command::ClosePin(_)
                    | Command::CopyPin(_)
                    | Command::ViewerClosed
                    | Command::CloseBar
            );
            assert_eq!(
                is_command_enabled(&command, ModalState::Capturing),
                expected,
                "Capturing: {command:?}"
            );
        }
    }

    /// `modal_state()` reports the capture session as modal so menus grey
    /// out while the overlay owns the screen.
    #[test]
    fn capturing_reports_a_modal_state() {
        let (shared, _ctx) = harness();
        *lock(&shared.state) = AppState::Capturing;
        assert_eq!(shared.modal_state(), ModalState::Capturing);
    }

    /// A blocked command is dropped by dispatch, not executed: SaveAll
    /// during the Settings modal must not write anything.
    #[test]
    fn modal_blocks_save_all_dispatch() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).push_stub("capture 1");
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::OpenSettings);
        assert_eq!(shared.modal_state(), ModalState::Settings);
        drive(&shared, &ctx, Command::SaveAll);
        // The tab is still unsaved: SaveAll never ran.
        assert!(lock(&shared.viewer).tabs.iter().all(|t| !t.saved));
    }

    /// Quit stays live under a modal: it closes the settings flag before
    /// running the normal dirty-session confirmation.
    #[test]
    fn quit_under_settings_closes_modal_first() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_stub("capture 1");
            viewer.dirty = true;
        }
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::OpenSettings);
        drive(&shared, &ctx, Command::Quit);
        assert!(!shared.settings_open.load(Ordering::Relaxed));
        assert!(lock(&shared.viewer).confirm_close);
    }

    /// Save-all composites annotations onto the exported PNG, and a
    /// duplicate-name clash during ConfirmSave aborts the quit instead of
    /// wiping the unsaved session.
    #[test]
    fn save_all_renders_annotations_and_confirm_save_respects_clash() {
        let (shared, ctx) = harness();
        let dir =
            std::env::temp_dir().join(format!("pixme-save-all-{}", crate::editor::meta::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        lock(&shared.settings).last_save_dir = Some(dir.clone());
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;

        // Two unsaved tabs sharing a title block ConfirmSave without
        // dropping the session.
        {
            let mut viewer = lock(&shared.viewer);
            let img = image::RgbaImage::from_pixel(8, 8, image::Rgba([0, 0, 0, 255]));
            let mut doc = Document::new(synthetic_capture(img));
            let id = doc.alloc_id();
            doc.annotations.push(crate::editor::document::Annotation {
                id,
                kind: crate::editor::document::AnnotationKind::Rect(
                    crate::editor::document::RectObj {
                        rect: crate::capture::PhysicalRect {
                            x: 0,
                            y: 0,
                            width: 8,
                            height: 8,
                        },
                        stroke: egui::Color32::RED,
                        stroke_w: 0.0,
                        dash: crate::editor::document::Dash::Solid,
                        fill: Some(egui::Color32::from_rgb(255, 0, 0)),
                        radius: 0.0,
                        outline: 0.0,
                        outline_color: egui::Color32::WHITE,
                        opacity: 1.0,
                        shadow: 0.0,
                    },
                ),
            });
            viewer.push_capture_document(doc, "dup".to_owned());
            viewer.push_capture_document(
                Document::new(synthetic_capture(image::RgbaImage::new(4, 4))),
                "dup".to_owned(),
            );
        }
        drive(&shared, &ctx, Command::OpenViewer);
        drive(&shared, &ctx, Command::Quit);
        assert!(lock(&shared.viewer).confirm_close);
        drive(&shared, &ctx, Command::ConfirmSave);
        {
            let viewer = lock(&shared.viewer);
            assert_eq!(viewer.tabs.len(), 2, "session kept on clash");
            assert!(viewer.dup_names, "clash modal raised");
            assert!(!viewer.confirm_close, "confirm modal dismissed");
        }

        // Rename the second tab and run SaveAll: the first tab's red
        // annotation must be baked into the written PNG.
        {
            let mut viewer = lock(&shared.viewer);
            viewer.dup_names = false;
            viewer.tabs[1].title = "unique".to_owned();
        }
        drive(&shared, &ctx, Command::SaveAll);
        drain_save_all(&shared);
        let saved_img = crate::export::file::load_image(&dir.join("dup.png")).unwrap();
        assert_eq!(saved_img.get_pixel(4, 4).0, [255, 0, 0, 255]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SaveActive on a saved + renamed tab proposes the NEW title (not
    /// the old file stem) while still exempting the source path from
    /// the exists-block.
    #[test]
    fn save_active_proposes_the_renamed_title() {
        let (shared, ctx) = harness();
        let path = std::env::temp_dir().join("old_name.png");
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_stub("old_name");
            viewer.mark_saved(0, path.clone());
            viewer.set_active_title("new_name".to_owned());
        }
        drive(&shared, &ctx, Command::SaveActive);
        let guard = lock(&shared.save_dialog);
        let dialog = guard.as_ref().expect("save dialog open");
        assert_eq!(dialog.filename, "new_name");
        assert_eq!(dialog.existing_ok, None);
    }

    /// Clicking the pin close dot (even with slight pointer drift or near
    /// its bottom-right edge) closes the pin on the first click without
    /// emitting `StartDrag`, `OuterPosition` or `InnerSize`.
    #[test]
    fn pin_close_dot_closes_without_moving_or_resizing() {
        for press_pos in [egui::pos2(16.0, 16.0), egui::pos2(22.0, 22.0)] {
            let (shared, ctx) = harness();
            crate::ui::theme::install_fonts(&ctx);
            let img = image::RgbaImage::from_pixel(320, 200, image::Rgba([40, 40, 40, 255]));
            let mut pin =
                crate::ui::pin::PinWindow::new(7, synthetic_capture(img), egui::pos2(100.0, 100.0));
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(320.0, 200.0));

            let mut run_frame = |events: Vec<egui::Event>| -> Vec<egui::ViewportCommand> {
                let raw = egui::RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..Default::default()
                };
                let mut out = ctx.run_ui(raw, |ui| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::NONE)
                        .show(ui, |ui| pin.show(ui, &shared));
                });
                out.textures_delta.clear();
                out.viewport_output
                    .get(&egui::ViewportId::ROOT)
                    .map(|v| v.commands.clone())
                    .unwrap_or_default()
            };

            let moved_or_resized = |cmds: &[egui::ViewportCommand]| {
                cmds.iter().any(|c| {
                    matches!(
                        c,
                        egui::ViewportCommand::StartDrag
                            | egui::ViewportCommand::OuterPosition(_)
                            | egui::ViewportCommand::InnerSize(_)
                    )
                })
            };

            // Frame 1: hover over the pin so chrome widgets are registered.
            let cmds = run_frame(vec![egui::Event::PointerMoved(press_pos)]);
            assert!(!moved_or_resized(&cmds), "hover emitted {cmds:?}");

            // Frame 2: press on the close dot.
            let cmds = run_frame(vec![egui::Event::PointerButton {
                pos: press_pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }]);
            assert!(
                !moved_or_resized(&cmds),
                "press at {press_pos:?} leaked viewport command: {cmds:?}"
            );

            // Frame 3: slight trackpad drift while held, then release.
            let release_pos = press_pos + egui::vec2(2.0, 2.0);
            let cmds = run_frame(vec![
                egui::Event::PointerMoved(release_pos),
                egui::Event::PointerButton {
                    pos: release_pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ]);
            assert!(
                !moved_or_resized(&cmds),
                "drift+release at {press_pos:?} leaked viewport command: {cmds:?}"
            );
            assert!(
                lock(&shared.pending)
                    .iter()
                    .any(|c| matches!(c, Command::ClosePin(7))),
                "ClosePin(7) was not queued after clicking at {press_pos:?}"
            );
        }
    }

    /// `ShowBar` restores the remembered bar spot only while it lands on
    /// the anchor monitor — a spot from a since-moved viewer is stale.
    #[test]
    fn remembered_bar_spot_must_share_the_anchor_monitor() {
        fn monitor(x: i32, y: i32) -> MonitorInfo {
            MonitorInfo {
                id: x as u32,
                name: "m".into(),
                rect: crate::capture::PhysicalRect {
                    x,
                    y,
                    width: 1000,
                    height: 800,
                },
                // scale 1.0 keeps `ui_to_native` an identity on both
                // OSes so the hit test is deterministic.
                scale_factor: 1.0,
                is_primary: false,
            }
        }
        let sidecar = monitor(-1133, 819);
        let main5k = monitor(0, 0);
        let monitors = vec![sidecar.clone(), main5k];
        // On the Sidecar display, a spot on it restores.
        assert!(ui_pos_on_anchor(
            egui::pos2(-700.0, 900.0),
            &monitors,
            &sidecar
        ));
        // …but a spot remembered on the BenQ does not.
        assert!(!ui_pos_on_anchor(
            egui::pos2(500.0, 300.0),
            &monitors,
            &sidecar
        ));
        // A spot outside every monitor is stale too.
        assert!(!ui_pos_on_anchor(
            egui::pos2(9999.0, 9999.0),
            &monitors,
            &sidecar
        ));
        // No monitors enumerated (xcap failure): nothing restores.
        assert!(!ui_pos_on_anchor(egui::pos2(-700.0, 900.0), &[], &sidecar));
    }
}
