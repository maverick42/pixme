//! Application root: the lifecycle state machine, the single command-dispatch
//! path and the eframe event loop.
//!
//! One shared machine, identical on both OSes: `Idle` (no visible window —
//! the app is resident in the macOS menu bar / Windows tray), `Capturing`
//! (selection in progress) and `Viewer` (tabbed session window, home of the
//! capture controls). Every user action is a [`Command`] — from the viewer's
//! capture bar, the resident-icon menu or the app menus — queued and
//! dispatched at the top of whichever viewport pass runs.
//!
//! Viewport topology: egui/winit never repaints a window that was never
//! shown, so the root viewport is a permanent 1×1 transparent "anchor"
//! window (`taskbar(false)`, no decorations) that keeps the pump alive.
//! The real windows — viewer, settings — are deferred child viewports
//! registered each frame while their state requires them.
//!
//! Launch opens the viewer directly (it restores the persisted session).
//! Closing the viewer returns to `Idle` (session kept). Quitting only
//! happens through the menu.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use eframe::egui;
use rust_i18n::t;

use crate::capture::MonitorInfo;
use crate::config::Settings;
use crate::editor::Document;
use crate::i18n::{self, Language};
use crate::ui::capture_overlay::OverlaySession;
use crate::ui::viewer::Viewer;
use crate::{platform, ui};

/// The lifecycle states (one shared machine, both OSes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AppState {
    /// No visible window — the app is resident in the menu bar / tray.
    #[default]
    Idle,
    /// A capture/selection is in progress (phase 3 fills this state in).
    Capturing,
    /// The tabbed session viewer is the visible window.
    Viewer,
}

/// Which capture the user asked for. Distinct from [`crate::capture`]'s
/// backend contract: these are the *commands* the viewer's capture bar
/// dispatches; the pipeline maps them to `xcap` calls.
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
    /// Show the viewer (icon menu's "Open PixMe").
    OpenViewer,
    /// A second-instance relaunch asked the resident instance to
    /// surface (Windows wake event): like `OpenViewer`, but it also
    /// runs while a modal owns the UI and re-raises the modal window
    /// (Draw, pin save dialog) above the viewer it is modal to.
    WakeViewer,
    /// Left click on the status item / tray icon: open the viewer from
    /// `Idle`, close it back to the resident icon when it is up.
    ToggleViewer,
    /// The viewer window's close was requested.
    ViewerClosed,
    /// Open the settings window.
    OpenSettings,
    /// Open the settings window on the About page (egui twin of the
    /// macOS native About panel).
    OpenAbout,
    /// The settings window was closed.
    CloseSettings,
    /// Internal: the settings draft was applied — re-sync the armed
    /// global hotkeys with `settings.hotkeys`.
    HotkeysChanged,
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
    /// Move the viewer onto the monitor with this stable id (status strip).
    SelectMonitor(String),
    /// Re-enumerate the connected monitors (status strip ↻ button).
    RefreshMonitors,
    /// Open the monitor-rename prompt (status strip right-click).
    RenameMonitor(String),
    /// Forget every saved per-monitor preference (names + window
    /// geometry) and re-centre the viewer on the primary display —
    /// the resident menu's "Reset monitors" escape hatch.
    ResetMonitors,
    /// Close a pinned capture window.
    ClosePin(u32),
    /// Copy a pinned capture's image to the clipboard.
    CopyPin(u32),
    /// Save a pinned capture via a file dialog.
    SavePin(u32),
    /// Move the viewer's workspace filter to this workspace id —
    /// written to prefs at once; the viewer picks it up on the next
    /// pass (switching is a filter, never a reload).
    SwitchWorkspace(String),
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
        | Command::HotkeysChanged
        | Command::ConfirmSave
        | Command::ConfirmDiscard
        | Command::ConfirmCancel
        | Command::ConfirmCapture
        | Command::CancelCapture
        | Command::ClosePin(_)
        | Command::CopyPin(_)
        | Command::ViewerClosed => true,
        // Draw owns its document: its undo/redo/copy apply there; the
        // viewer's copies must not act behind the modal.
        Command::UndoActive | Command::RedoActive | Command::CopyActive => {
            modal == ModalState::Draw
        }
        // Re-invoking the open modal's entry re-focuses it instead of
        // re-opening. When the SETTINGS modal is up the same menu click
        // brings it forward.
        Command::OpenSettings | Command::OpenAbout => modal == ModalState::Settings,
        Command::OpenViewer | Command::ToggleViewer => modal == ModalState::Settings,
        // A relaunch surfaces the viewer under every modal EXCEPT a
        // live capture — stealing focus from the overlay would eat its
        // Esc/clicks — and the quit flush (early return above).
        Command::WakeViewer => modal != ModalState::Capturing,
        // Everything else is blocked while a modal owns the UI.
        Command::DeleteActive
        | Command::DeleteFileActive
        | Command::Capture(_)
        | Command::CaptureNow(_)
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
        | Command::SelectMonitor(_)
        | Command::RefreshMonitors
        | Command::RenameMonitor(_)
        | Command::ResetMonitors
        | Command::SavePin(_)
        | Command::SwitchWorkspace(_) => false,
    }
}

/// `egui::ViewportId` of the child windows (`from_hash_of` is not a const fn
/// in egui 0.36, hence the functions).
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
/// The fullscreen pick overlays — one transparent viewport per monitor,
/// swallowing the validating click while a colour pick is armed. A
/// single union-spanning window cannot cover a non-rectangular
/// multi-monitor layout: AppKit clips it to one screen's visibleFrame
/// (the Sidecar iPad stayed unveiled).
fn pick_overlay_id(index: usize) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("pixme.pick_overlay", index))
}

/// Wakes every command-pumping viewport from outside a pass (OS menu
/// callbacks, worker threads): `Context::request_repaint()` alone only
/// targets `ViewportId::ROOT`, which macOS throttles when the 1×1 anchor
/// sits behind a visible window.
pub fn wake_viewports(ctx: &egui::Context) {
    ctx.request_repaint_of(egui::ViewportId::ROOT);
    ctx.repaint_if_live(viewer_id());
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
    /// Where the viewer must land: a monitor plus, when one was saved,
    /// its persisted outer rect in NATIVE units (per-monitor geometry).
    pub viewer_target: Mutex<Option<ViewerTarget>>,
    /// Monitor the viewer currently sits on — resolved in NATIVE units at
    /// write time (the viewer's own ppp), so the countdown never has to
    /// mix two windows' point spaces (mixed-DPI safe).
    pub viewer_monitor: Mutex<Option<MonitorInfo>>,
    /// The connected monitors with their OS-stable ids — the status
    /// strip's monitor row. Snapshot refreshed at startup and on
    /// [`Command::RefreshMonitors`]; never re-enumerated per frame.
    pub monitors: Mutex<Vec<platform::MonitorIdentity>>,
    /// Geometry ledger for programmatic viewer moves — commands are
    /// deduped against the last REQUESTED rect, never reported ones.
    pub viewer_sync: Mutex<ui::viewport_sync::ViewportSync>,
    /// Frames to skip before re-sampling the reported `viewer_rect` —
    /// set after a programmatic move so a transient mid-DPI rect is
    /// never persisted.
    pub viewer_geo_settle: Mutex<u8>,
    /// Topology-change pin window: while set, the geometry loop
    /// re-applies the saved rect every frame (macOS re-shuffles
    /// windows asynchronously after a plug/unplug — one restore is
    /// not enough) and nothing is persisted.
    pub viewer_topology_pin: Mutex<Option<std::time::Instant>>,
    /// Last topology-pin pull-back — throttled so a per-frame push
    /// can't create a position feedback storm with egui/winit.
    pub viewer_pin_last: Mutex<Option<std::time::Instant>>,
    /// Last reported viewer outer rect — sampled each frame to spot
    /// user move/resize gestures in flight.
    pub viewer_last_outer: Mutex<Option<egui::Rect>>,
    /// Last observed outer-rect change: a gesture is settled 250 ms
    /// after the last change, and a settle on another monitor becomes
    /// a relocation to that monitor's saved geometry.
    pub viewer_move_at: Mutex<Option<std::time::Instant>>,

    /// Pending landing target for the Draw editor — stashed by
    /// `OpenDraw` (the viewer's monitor + that monitor's saved
    /// `draw_rect_native`, if any), consumed by the draw viewport's
    /// first pass. Reuses the viewer's monitor+rect container.
    draw_target: Mutex<Option<ViewerTarget>>,
    /// The monitor the Draw window currently sits on — the host its
    /// settled geometry is persisted under when the window's centre
    /// lands outside every known display.
    draw_monitor: Mutex<Option<MonitorInfo>>,
    /// Last reported Draw outer rect — sampled each frame to spot a
    /// user move/resize gesture in flight (mirrors `viewer_last_outer`).
    draw_last_outer: Mutex<Option<egui::Rect>>,
    /// Last observed Draw outer-rect change: a gesture is settled
    /// 250 ms after the last change, then `draw_rect_native` is
    /// written under the host monitor (mirrors `viewer_move_at`).
    draw_move_at: Mutex<Option<std::time::Instant>>,
    /// Frames to skip settle tracking after the programmatic
    /// placement — the reported rect lies while the move lands
    /// (same convention as `viewer_geo_settle`).
    draw_geo_settle: Mutex<u8>,

    /// A window's native rect changed on its current monitor —
    /// `prefs.monitors[id].rect_native`/`draw_rect_native` is stale and
    /// `pump` flushes a debounced `settings.save()`.
    pub monitor_geom_dirty_at: Mutex<Option<std::time::Instant>>,
    /// winit `available_monitors()` placements `(x, y, w, h)` in physical
    /// pixels, snapshotted for the overlay's `with_monitor`/`SetMonitor`
    /// targeting (xcap and winit don't enumerate in the same order).
    pub winit_placements: Mutex<Vec<(i32, i32, u32, u32)>>,
    /// Whether the settings window is open.
    pub settings_open: AtomicBool,
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
    /// [`Shared::push_capture`] when the viewer starts a capture, consumed
    /// by `begin_capture`. `None` for window-less launches (dev hooks):
    /// `begin_capture` falls back to the monitor under the cursor, then
    /// the primary monitor.
    capture_start_monitor: Mutex<Option<MonitorInfo>>,
    /// The state a delayed capture was launched from: arming the
    /// countdown hides the viewer right away (a live viewer rides the
    /// whole wait on screen, then its teardown races the freeze — the
    /// "ghost" watermark), so `begin_capture` reads the real origin here
    /// instead of the by-then-`Idle` state — and the countdown's Esc
    /// restores it.
    capture_return_to: Mutex<Option<AppState>>,
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
    /// The in-flight monitor refresh: `RefreshMonitors` enumerates off
    /// the UI thread — `xcap` + `EnumDisplayDevices` + `QueryDisplayConfig`
    /// can block for a long time (RDP, indirect displays) — and `pump`
    /// swaps the list in when the reply lands.
    pub monitors_rx: Mutex<Option<std::sync::mpsc::Receiver<Vec<platform::MonitorIdentity>>>>,
    /// The hot-plug poll: last seen display count + last poll instant —
    /// a change pushes `RefreshMonitors` so a disconnected monitor's
    /// viewer anchoring re-clamps instead of stranding the window.
    pub display_poll: Mutex<(u32, std::time::Instant)>,

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
    /// Armed global hotkeys — `None` when the OS manager failed to
    /// initialize (the app still runs, hotkeys just do nothing).
    pub hotkeys: Mutex<Option<crate::hotkeys::HotkeyManager>>,
    /// Per-action registration errors from the last `HotkeysChanged`
    /// sync — the settings page renders them as badges.
    pub hotkey_errors: Mutex<
        std::collections::BTreeMap<crate::hotkeys::HotkeyAction, crate::hotkeys::HotkeyError>,
    >,
    /// The hotkey id the settings page is testing (macOS only): the
    /// next press of THAT combo must reach the listener — `Some` while
    /// the test window is open, `None` once the press arrived or the
    /// window timed out. macOS `RegisterEventHotKey` accepts combos
    /// another app swallows via an event tap, so the "Test" button
    /// synthesizes a real press and watches for it here. The event id
    /// — not the action — is matched, so an older armed combo bound
    /// to the same action keeps firing normally during the window.
    pub hotkey_verify: Mutex<Option<u32>>,
}

/// Where a window must land: `monitor` plus the persisted outer rect
/// in the monitor's NATIVE units when prefs hold one (`None` = centre
/// the window on that monitor with its current size). Used by the
/// viewer (`viewer_target`) and the Draw editor (`draw_target`).
#[derive(Clone, Debug)]
pub struct ViewerTarget {
    /// The destination monitor.
    pub monitor: MonitorInfo,
    /// Saved outer rect `[x, y, w, h]` in native units, if any.
    pub native_rect: Option<[i32; 4]>,
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
    /// from — the viewer window the capture bar lives in — so the overlay
    /// opens on "the screen where the zone starts". The record survives
    /// the capture delay: `begin_capture` may run later, from the anchor's
    /// pass. Window-less launches (dev hooks) leave the slot empty and
    /// `begin_capture` falls back to the monitor under the cursor.
    pub fn push_capture(&self, mode: CaptureMode, ctx: &egui::Context) {
        // The session monitor — the monitor the viewer sits on — is the
        // capture's start monitor (§2.5); the asking viewport's monitor
        // is the fallback (dev hooks, tests with no live viewer).
        *lock(&self.capture_start_monitor) = lock(&self.viewer_monitor)
            .clone()
            .or_else(|| monitor_under(ctx));
        self.push(Command::Capture(mode));
    }

    /// Settings guard for UI code that edits preferences.
    pub fn lock_settings(&self) -> MutexGuard<'_, Settings> {
        lock(&self.settings)
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
        } else if self.settings_open.load(Ordering::Relaxed) {
            // The settings modal is modal to the viewer too: the dimmed veil
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
        // Debounced per-monitor geometry save: the viewer writes
        // `rect_native` on every settled move/resize — a half-second of
        // quiet flushes `prefs.json` once per gesture.
        let flush = lock(&self.monitor_geom_dirty_at)
            .is_some_and(|t| t.elapsed() > std::time::Duration::from_millis(500));
        if flush {
            *lock(&self.monitor_geom_dirty_at) = None;
            lock(&self.settings).save();
        }
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
        // Windows: Alt+Tab (or any OS mechanism) can steal the overlay's
        // focus mid-capture and Esc would then hit another app — keep
        // handing it back while `Capturing`. No-op on macOS (kiosk).
        if *lock(&self.state) == AppState::Capturing {
            platform::refocus_capture_overlay(ctx, overlay_id());
        }
        let commands: Vec<Command> = platform::take_pending_commands()
            .into_iter()
            .chain(std::mem::take(&mut *lock(&self.pending)))
            .collect();
        for command in &commands {
            // Captures pushed mid-drain must land in the CURRENT
            // workspace — a SwitchWorkspace earlier in this batch would
            // otherwise leave the viewer's filter one frame behind.
            {
                let s = lock(&self.settings);
                lock(&self.viewer).sync_workspace(&s);
            }
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
            s.last_open_dir.is_some() || s.active_workspace().output_dir.is_some()
        };
        let sync = (modal, has_doc, has_file, has_last_dir);
        let mut last_menu_sync = lock(&self.last_menu_sync);
        if *last_menu_sync != sync {
            *last_menu_sync = sync;
            platform::sync_menu_enabled(modal, has_doc, has_file, has_last_dir);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ctx.repaint_if_live(viewer_id());
        }
        // Recent files change without a modal transition (open/save
        // succeeds, history cleared): the macOS File menu is native and
        // must be re-synced explicitly — a no-op on Windows, whose menus
        // are egui-drawn every frame.

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
        // A background monitor refresh lands here — swap the cached
        // list. While it runs, keep repainting so the poll stays alive
        // in Idle too (same trap as `quit_rx`). Bound first: a scrutinee
        // temporary would keep the `monitors_rx` guard held through the
        // match arms, which relock it — instant self-deadlock.
        let refresh = lock(&self.monitors_rx).as_ref().map(|r| r.try_recv());
        match refresh {
            Some(Ok(list)) => {
                tracing::info!(count = list.len(), "monitors_refreshed");
                // A failed enumeration (mid reconfiguration) yields an
                // EMPTY list — never swap it in: it would purge the
                // inventory, empty `bounds`, and disable the viewer's
                // clamping. Keep the previous list on zero.
                if !list.is_empty() {
                    *lock(&self.monitors) = list;
                } else {
                    tracing::warn!(
                        "monitor refresh returned no monitors — keeping the previous inventory"
                    );
                }
                *lock(&self.monitors_rx) = None;
                // A plug/unplug shuffles windows at the OS level, and
                // macOS re-applies its own placement asynchronously —
                // a single restore races it and loses. Arm a ~2 s pin
                // window: the geometry loop re-applies the saved rect
                // every frame until the OS settles (and persists
                // nothing meanwhile).
                if *lock(&self.state) == AppState::Viewer {
                    *lock(&self.viewer_topology_pin) =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                }
                ctx.repaint_if_live(viewer_id());
            }
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                // The worker died mid-enumeration — free the slot so a
                // later refresh can retry.
                tracing::warn!("monitor refresh worker died");
                *lock(&self.monitors_rx) = None;
            }
            Some(Err(std::sync::mpsc::TryRecvError::Empty)) => ctx.request_repaint(),
            None => {}
        }
        // Monitor hot-(un)plug: poll the OS display count ~1/s — a
        // disconnect leaves a stale monitor in the cached inventory,
        // and the geometry clamp would keep anchoring the viewer to a
        // screen that no longer exists (window stranded off-screen).
        {
            let mut last = lock(&self.display_poll);
            if last.1.elapsed() >= std::time::Duration::from_secs(1) {
                last.1 = std::time::Instant::now();
                let n = platform::display_count();
                if last.0 != n {
                    last.0 = n;
                    drop(last);
                    // Arm the pin NOW, not when the async refresh lands:
                    // macOS starts shuffling the window immediately, and
                    // the stale inventory (iPad still listed for ~1 s)
                    // would otherwise persist the transient position into
                    // the dying monitor's rect_native.
                    *lock(&self.viewer_topology_pin) =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(2));
                    self.push(Command::RefreshMonitors);
                    ctx.request_repaint();
                }
            }
        }
        // A background save-all finished: the tabs come back into the
        // viewer; a ConfirmSave batch then closes the (now clean)
        // session, or keeps it open when some tab is still unsaved.
        let save_done = lock(&self.save_all)
            .as_ref()
            .and_then(|j| j.rx.try_recv().ok());
        if let Some((tabs, saved_paths)) = save_done {
            let job = lock(&self.save_all).take().expect("save-all job");
            let saved_count = saved_paths.len();
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
            if saved_count == job.total {
                crate::ui::widgets::toast(
                    ctx,
                    crate::ui::widgets::ToastKind::Success,
                    t!("viewer.toast.saved_all", count = saved_count).to_string(),
                );
            } else {
                crate::ui::widgets::toast(
                    ctx,
                    crate::ui::widgets::ToastKind::Error,
                    t!("viewer.toast.save_failed").to_string(),
                );
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
                    // The workspace the saved tab belongs to memorizes
                    // the folder actually used — the per-workspace heir
                    // of the old `last_save_dir`. Pin saves keep their
                    // workspace's folder untouched (a pin owns none).
                    let save_ws = job.viewer_tab.and_then(|i| {
                        let v = lock(&self.viewer);
                        v.tabs.get(i).map(|t| t.doc.meta.workspace_id.clone())
                    });
                    if let Some(i) = job.viewer_tab {
                        lock(&self.viewer).mark_saved(i, job.path.clone());
                    }
                    let mut s = lock(&self.settings);
                    if let Some(ws_id) = save_ws {
                        let id = s.resolve_workspace(&ws_id).to_owned();
                        if let Some(w) = s.workspaces.iter_mut().find(|w| w.id == id) {
                            w.output_dir = Some(job.dir.clone());
                        }
                    }
                    s.push_recent(job.path);
                    s.save();
                    *lock(&self.save_dialog) = None;
                    crate::ui::widgets::toast(
                        ctx,
                        crate::ui::widgets::ToastKind::Success,
                        t!("viewer.toast.saved").to_string(),
                    );
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
        // it here covers the menus, the tray and the viewer.
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
                    // The viewer hides NOW, not at countdown zero: a live
                    // viewer would ride the whole wait on screen, and its
                    // teardown at `CaptureNow` would race the freeze (the
                    // "ghost" watermark). The origin is stashed so a
                    // cancelled capture still lands back here. Skipped
                    // while Draw is modal — the editor can't float alone.
                    if *lock(&self.state) == AppState::Viewer && lock(&self.draw).is_none() {
                        *lock(&self.capture_return_to) = Some(AppState::Viewer);
                        self.enter(AppState::Idle, ctx);
                    }
                } else {
                    self.begin_capture(mode, ctx);
                }
            }
            Command::CaptureNow(mode) => self.begin_capture(mode, ctx),
            Command::ConfirmCapture => self.confirm_capture(ctx),
            Command::CancelCapture => self.cancel_capture(ctx),
            Command::OpenViewer => {
                self.enter(AppState::Viewer, ctx);
                // Re-invoking while the window is already open (behind
                // other apps, or minimized in the Dock) must still bring
                // it forward: `enter` only requests focus on a state
                // CHANGE, so de-minimize and focus explicitly here.
                self.bring_viewer_forward(ctx);
            }
            Command::WakeViewer => {
                // The surface work of OpenViewer, then re-raise every
                // modal child ABOVE the just-raised viewer — deferred
                // viewports are top-level windows, so a relaunch during
                // Draw or a pin save would otherwise strand the modal
                // behind its own dimmed parent.
                self.enter(AppState::Viewer, ctx);
                self.bring_viewer_forward(ctx);
                let modals = [
                    lock(&self.draw).is_some().then(draw_id),
                    lock(&self.save_dialog).is_some().then(save_dialog_id),
                ];
                for id in modals.into_iter().flatten() {
                    ctx.cmd_if_live(id, egui::ViewportCommand::Minimized(false));
                    ctx.cmd_if_live(id, egui::ViewportCommand::Focus);
                    self.request_focus(id);
                    ctx.repaint_if_live(id);
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
            Command::ToggleViewer => {
                // The resident icon's left click acts like a taskbar
                // button PER MONITOR: each display's menu bar (macOS)
                // / taskbar (Windows) carries it, so the click's cursor
                // position picks the monitor the action applies to.
                let clicked = platform::cursor_position().and_then(|(cx, cy)| {
                    lock(&self.monitors)
                        .iter()
                        .find(|id| {
                            let r = &id.info.rect;
                            let (x0, y0) = (r.x as f32, r.y as f32);
                            let (x1, y1) = (
                                (r.x + r.width as i32) as f32,
                                (r.y + r.height as i32) as f32,
                            );
                            cx >= x0 && cx < x1 && cy >= y0 && cy < y1
                        })
                        .map(|id| id.stable_id.clone())
                });
                if let Some(stable_id) = clicked {
                    // The resident icon keeps REAL taskbar semantics:
                    // shown+focused on that monitor → hide; anything
                    // else → show there. (The status-strip button
                    // re-centres instead — see SelectMonitor.)
                    let m = lock(&self.monitors)
                        .iter()
                        .find(|id| id.stable_id == stable_id)
                        .map(|id| id.info.clone());
                    if let Some(m) = m {
                        let (minimized, focused) = ctx.input(|i| {
                            let v = i.raw.viewports.get(&viewer_id());
                            (
                                v.and_then(|v| v.minimized).unwrap_or(false),
                                v.and_then(|v| v.focused).unwrap_or(false),
                            )
                        });
                        let minimized = minimized || platform::viewer_is_minimized();
                        let on_target =
                            lock(&self.viewer_monitor).as_ref().map(|v| v.rect) == Some(m.rect);
                        if *lock(&self.state) == AppState::Viewer
                            && !minimized
                            && focused
                            && on_target
                        {
                            self.dispatch(Command::ViewerClosed, ctx);
                        } else {
                            *lock(&self.viewer_target) = Some(self.viewer_target_for(m));
                            self.dispatch(Command::OpenViewer, ctx);
                        }
                    }
                    return;
                }
                // Cursor off every known display (or position read
                // failed): plain toggle on the current monitor.
                let (minimized, focused) = ctx.input(|i| {
                    let v = i.raw.viewports.get(&viewer_id());
                    (
                        v.and_then(|v| v.minimized).unwrap_or(false),
                        v.and_then(|v| v.focused).unwrap_or(false),
                    )
                });
                let minimized = minimized || platform::viewer_is_minimized();
                if *lock(&self.state) == AppState::Viewer && !minimized && focused {
                    self.dispatch(Command::ViewerClosed, ctx);
                } else {
                    self.dispatch(Command::OpenViewer, ctx);
                }
            }
            Command::OpenAbout => {
                // One About surface on both OSes: the settings modal
                // opened on its About page.
                ui::settings::open_at(ctx, ui::settings::SettingsPage::About);
                self.dispatch(Command::OpenSettings, ctx);
            }
            Command::OpenSettings => {
                if self.settings_open.load(Ordering::Relaxed) {
                    // Already open: re-invocation brings the existing
                    // modal's host viewer to the front rather than
                    // re-opening it.
                    self.bring_viewer_forward(ctx);
                    return;
                }
                self.settings_open.store(true, Ordering::Relaxed);
                // The window edits a draft: changes apply on OK only.
                *lock(&self.settings_draft) = Some(lock(&self.settings).clone());
                // The settings are a modal of the viewer: the viewer must
                // exist AND come forward — a minimized/background window
                // would swallow the modal.
                self.enter(AppState::Viewer, ctx);
                self.bring_viewer_forward(ctx);
            }
            Command::CloseSettings => {
                self.settings_open.store(false, Ordering::Relaxed);
                *lock(&self.settings_draft) = None;
                *lock(&self.hotkey_verify) = None;
                ui::settings::end_recording(ctx);
                // A field may have been listening — re-arm whatever
                // was suspended and release any pending verification.
                if let Some(m) = lock(&self.hotkeys).as_mut() {
                    m.resume();
                    m.release_verify();
                }
                ctx.repaint_if_live(viewer_id());
            }
            Command::HotkeysChanged => {
                let cfg = lock(&self.settings).hotkeys.clone();
                let mut errors = std::collections::BTreeMap::new();
                if let Some(m) = lock(&self.hotkeys).as_mut() {
                    errors = m.sync(&cfg);
                }
                *lock(&self.hotkey_errors) = errors;
            }
            Command::Quit => {
                // Quit is always live: close the non-Draw modals first so
                // the confirmation (or the exit) never sits behind them.
                // An open Draw editor is committed as usual by
                // `save_session` on the actual quit.
                self.settings_open.store(false, Ordering::Relaxed);
                *lock(&self.settings_draft) = None;
                *lock(&self.hotkey_verify) = None;
                ui::settings::end_recording(ctx);
                // A field may have been listening — re-arm whatever
                // was suspended and release any pending verification.
                if let Some(m) = lock(&self.hotkeys).as_mut() {
                    m.resume();
                    m.release_verify();
                }
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
                    // A Dock-initiated quit (the intercepted AppKit
                    // `terminate:`) can catch the viewer minimized or
                    // behind another app — pull it out and focus it, or
                    // the Save/Discard/Cancel modal stays invisible.
                    self.bring_viewer_forward(ctx);
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
                    // Couldn't even start (no writable folder).
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
            Command::SelectMonitor(stable_id) => {
                let target = lock(&self.monitors)
                    .iter()
                    .find(|m| m.stable_id == stable_id)
                    .map(|m| m.info.clone());
                if let Some(m) = target {
                    // Taskbar semantics, shared by the strip buttons and
                    // the resident icon: clicking the monitor the viewer
                    // is SHOWN and FOCUSED on re-centres it at the
                    // default size (the no-prefs landing); anything else
                    // (other monitor, hidden, minimized, behind another
                    // app) brings it up on the clicked monitor's saved
                    // rect.
                    let (minimized, focused) = ctx.input(|i| {
                        let v = i.raw.viewports.get(&viewer_id());
                        (
                            v.and_then(|v| v.minimized).unwrap_or(false),
                            v.and_then(|v| v.focused).unwrap_or(false),
                        )
                    });
                    let minimized = minimized || platform::viewer_is_minimized();
                    let on_target =
                        lock(&self.viewer_monitor).as_ref().map(|v| v.rect) == Some(m.rect);
                    if *lock(&self.state) == AppState::Viewer && !minimized && focused && on_target
                    {
                        // Fresh-install landing on the clicked monitor:
                        // no saved rect and no live rect → centred at
                        // the default size (same path as ResetMonitors).
                        // The target only lands at window BUILD time —
                        // a live viewport ignores it — so the window is
                        // torn down and rebuilt, like every relocation.
                        *lock(&self.viewer_rect) = None;
                        *lock(&self.viewer_target) = Some(ViewerTarget {
                            monitor: m,
                            native_rect: None,
                        });
                        self.dispatch(Command::ViewerClosed, ctx);
                        self.dispatch(Command::OpenViewer, ctx);
                        return;
                    }
                    // The viewer pass applies the saved native rect (or
                    // the centred default); `OpenViewer` also opens from
                    // Idle, un-minimizes, and raises a behind window.
                    *lock(&self.viewer_target) = Some(self.viewer_target_for(m));
                    self.dispatch(Command::OpenViewer, ctx);
                }
            }
            Command::RefreshMonitors => {
                // Enumeration blocks on OS display queries — run it off
                // the UI thread; `pump` swaps the list in on reply.
                if lock(&self.monitors_rx).is_none() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(platform::monitor_identities());
                    });
                    *lock(&self.monitors_rx) = Some(rx);
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    ctx.repaint_if_live(viewer_id());
                }
            }
            Command::RenameMonitor(stable_id) => {
                // The draft opens on the custom name, or the OS name when
                // none was chosen yet — the field never starts empty.
                let name = lock(&self.settings)
                    .monitors
                    .get(&stable_id)
                    .and_then(|p| p.name.clone())
                    .or_else(|| {
                        lock(&self.monitors)
                            .iter()
                            .find(|m| m.stable_id == stable_id)
                            .map(|m| m.info.name.clone())
                    })
                    .unwrap_or_else(|| stable_id.clone());
                lock(&self.viewer).open_monitor_rename(stable_id, name);
            }
            Command::ResetMonitors => {
                // Back to first-launch monitor state: every per-monitor
                // entry (custom names AND saved geometry) plus the
                // last-host marker, flushed to prefs.json right away.
                {
                    let mut s = lock(&self.settings);
                    s.monitors.clear();
                    s.last_monitor = None;
                    s.save();
                }
                // Re-land like a fresh install: centred on the primary
                // display at the default size. Clearing `viewer_rect`
                // stops `place_viewer_on_monitor` from keeping the
                // current rect when its centre is already inside.
                let primary = {
                    let inv = lock(&self.monitors);
                    inv.iter()
                        .find(|id| id.info.is_primary)
                        .or_else(|| inv.first())
                        .map(|id| id.info.clone())
                };
                if let Some(m) = primary {
                    *lock(&self.viewer_rect) = None;
                    *lock(&self.viewer_target) = Some(ViewerTarget {
                        monitor: m,
                        native_rect: None,
                    });
                    self.dispatch(Command::OpenViewer, ctx);
                }
            }
            Command::ClosePin(id) => {
                // Deregistering the viewport next pass destroys the window.
                lock(&self.pins).retain(|p| p.id != id);
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
            Command::SwitchWorkspace(id) => {
                // The prefs entry is the filter's single source of
                // truth: writing it makes the viewer's `sync_workspace`
                // move on the next pass — an unknown id lands on `main`.
                let mut s = lock(&self.settings);
                let resolved = s.resolve_workspace(&id).to_owned();
                if resolved != s.active_workspace_id {
                    s.active_workspace_id = resolved.clone();
                    s.save();
                    tracing::info!(workspace = %resolved, "workspace_switched");
                }
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
                    let folder = {
                        let s = lock(&self.settings);
                        s.active_workspace().output_dir.clone().or_else(|| {
                            directories::UserDirs::new()
                                .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
                        })
                    };
                    if let Some(folder) = folder {
                        let mut dialog = crate::ui::save_dialog::SaveDialog::new(capture, folder);
                        // The workspace's format seeds the choice — the
                        // radio still leaves it free per save.
                        dialog.format = lock(&self.settings).active_workspace().format;
                        *lock(&self.save_dialog) = Some(dialog);
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
                // `|`, not `||` — `any` short-circuits at the first
                // success and only one file would open.
                let opened = paths.iter().fold(false, |acc, p| self.open_one(p) | acc);
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
                    s.last_open_dir
                        .clone()
                        .or_else(|| s.active_workspace().output_dir.clone())
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
                let fallback = {
                    let s = lock(&self.settings);
                    s.active_workspace().output_dir.clone().or_else(|| {
                        directories::UserDirs::new()
                            .and_then(|d| d.picture_dir().map(|p| p.to_path_buf()))
                    })
                };
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
                        // The workspace's format is the default — an
                        // already-saved file keeps its own extension.
                        dialog.format = lock(&self.settings).active_workspace().format;
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
                        Ok(()) => {
                            tracing::info!("clipboard_copy");
                            crate::ui::widgets::toast(
                                ctx,
                                crate::ui::widgets::ToastKind::Success,
                                t!("viewer.toast.copied").to_string(),
                            );
                        }
                        Err(e) => {
                            tracing::warn!("copy failed: {e}");
                            crate::ui::widgets::toast(
                                ctx,
                                crate::ui::widgets::ToastKind::Error,
                                t!("viewer.toast.copy_failed").to_string(),
                            );
                        }
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
                    // The editor opens on the viewer's monitor — its
                    // saved `draw_rect_native` when prefs hold one,
                    // centred there otherwise. Stale gesture tracking
                    // from a previous session must not persist early.
                    let monitor = lock(&self.viewer_monitor)
                        .clone()
                        .or_else(|| monitor_under(ctx))
                        .or_else(|| crate::capture::screen::primary_monitor().ok());
                    if let Some(m) = monitor {
                        let native_rect = self.monitor_stable_id(&m).and_then(|id| {
                            prefs.monitors.get(&id).and_then(|p| p.draw_rect_native)
                        });
                        *lock(&self.draw_target) = Some(ViewerTarget {
                            monitor: m,
                            native_rect,
                        });
                    }
                    *lock(&self.draw_last_outer) = None;
                    *lock(&self.draw_move_at) = None;
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

    /// The next capture tab title, expanded from the ACTIVE WORKSPACE's
    /// naming mask — `Screenshot#####`-style names. The `#` counter is
    /// per-workspace and persists when consumed; masks without it don't
    /// burn sequence numbers.
    fn capture_title(&self) -> String {
        let mut s = lock(&self.settings);
        let (title, bumped) = {
            let w = s.active_workspace_mut();
            let title =
                crate::config::expand_template(&w.name_mask, w.counter, &chrono::Local::now());
            let bumped = w.name_mask.contains('#');
            if bumped {
                w.counter += 1;
            }
            (title, bumped)
        };
        if bumped {
            s.save();
        }
        title
    }

    /// Writes unsaved tabs to their workspace's output folder (or
    /// `~/Pictures`) in the workspace's format with annotations
    /// composited — the same
    /// sequential write loop as before, just riding a worker thread so
    /// the viewer paints a progress veil (`pump` reattaches the tabs
    /// when the job lands). Toolbar/menu Save All (`then_confirm ==
    /// false`) is scoped to the visible workspace; quit-time
    /// `ConfirmSave` (`then_confirm == true`) covers every unsaved tab
    /// across ALL workspaces so a hidden bucket's captures are never
    /// silently dropped or left blocking the exit. Returns `true` only
    /// when the target set is clean right away.
    fn save_all(&self, ctx: &egui::Context, then_confirm: bool) -> bool {
        let default_folder =
            directories::UserDirs::new().and_then(|d| d.picture_dir().map(|p| p.to_path_buf()));
        let (active_ws, ws_folders, ws_formats, main_folder) = {
            let s = lock(&self.settings);
            let map: std::collections::HashMap<String, Option<std::path::PathBuf>> = s
                .workspaces
                .iter()
                .map(|w| (w.id.clone(), w.output_dir.clone()))
                .collect();
            let formats: std::collections::HashMap<String, crate::export::ImageFormat> = s
                .workspaces
                .iter()
                .map(|w| (w.id.clone(), w.format))
                .collect();
            let main = map
                .get(crate::config::workspace::MAIN_ID)
                .cloned()
                .flatten();
            (s.active_workspace().id.clone(), map, formats, main)
        };
        let resolve_ws = |id: &str| -> String {
            if ws_folders.contains_key(id) {
                id.to_owned()
            } else {
                crate::config::workspace::MAIN_ID.to_owned()
            }
        };
        let folder_for = |ws_id: &str| -> Option<std::path::PathBuf> {
            ws_folders
                .get(ws_id)
                .cloned()
                .flatten()
                .or_else(|| main_folder.clone())
                .or_else(|| default_folder.clone())
        };
        // The format is per workspace — unknown ids write PNG like `main`.
        let format_for = |ws_id: &str| -> crate::export::ImageFormat {
            ws_formats
                .get(&resolve_ws(ws_id))
                .copied()
                .unwrap_or_default()
        };
        let ensure_folder = |folder: Option<std::path::PathBuf>| -> Option<std::path::PathBuf> {
            let Some(folder) = folder else {
                tracing::warn!("save-all: no writable folder known");
                return None;
            };
            // A missing or disconnected output folder gets ONE create
            // attempt; on failure the captures stay in memory (the
            // session keeps them) and the user is told — they can save
            // later.
            if !folder.is_dir()
                && let Err(e) = std::fs::create_dir_all(&folder)
            {
                tracing::warn!(folder = %folder.display(), "output folder unreachable: {e}");
                crate::ui::widgets::toast(
                    ctx,
                    crate::ui::widgets::ToastKind::Error,
                    t!("viewer.toast.folder_unreachable").to_string(),
                );
                ctx.repaint_if_live(viewer_id());
                return None;
            }
            Some(folder)
        };

        // Toolbar/menu Save All always validates the active workspace's
        // folder up-front, even when no tab needs writing.
        if !then_confirm && ensure_folder(folder_for(&active_ws)).is_none() {
            return false;
        }

        let mut viewer = lock(&self.viewer);
        // Toolbar/menu Save All only touches what the user SEES (the
        // active workspace); ConfirmSave on quit covers the whole
        // session so hidden workspaces' unsaved captures are saved to
        // their own output folders before the exit wipes memory.
        let visible: std::collections::HashSet<String> = viewer
            .visible_indices()
            .into_iter()
            .map(|i| viewer.tabs[i].doc.meta.id.clone())
            .collect();
        let in_scope = |t: &crate::ui::viewer::Tab| {
            !t.saved && (then_confirm || visible.contains(&t.doc.meta.id))
        };

        // Duplicate titles inside the same workspace are refused until
        // the user renames — but only for the interactive Save All: the
        // quit-time ConfirmSave is a "save at all costs" path (the whole
        // session is about to be wiped, `db_retention = Never`), so
        // same-workspace twins also land via `unique_file`'s `_2`/`_3`
        // suffixing rather than trapping the user in a rename loop.
        if !then_confirm {
            let mut seen = std::collections::HashSet::new();
            let mut clash = false;
            for t in viewer.tabs.iter().filter(|t| in_scope(t)) {
                let ws_id = resolve_ws(&t.doc.meta.workspace_id);
                let stem = crate::config::filename::sanitize_filename(t.title.trim());
                if !seen.insert((ws_id.clone(), stem)) {
                    clash = true;
                    break;
                }
            }
            if clash {
                tracing::warn!("save-all blocked: duplicate tab titles");
                viewer.dup_names = true;
                drop(viewer);
                ctx.repaint_if_live(viewer_id());
                return false;
            }
        }

        // Resolve and validate the target folder for each unsaved tab
        // in scope before spawning the worker.
        let mut targets: std::collections::HashMap<
            String,
            (std::path::PathBuf, crate::export::ImageFormat),
        > = std::collections::HashMap::new();
        for t in viewer.tabs.iter().filter(|t| in_scope(t)) {
            let ws_id = resolve_ws(&t.doc.meta.workspace_id);
            let Some(folder) = ensure_folder(folder_for(&ws_id)) else {
                return false;
            };
            targets.insert(t.doc.meta.id.clone(), (folder, format_for(&ws_id)));
        }
        if targets.is_empty() {
            return true;
        }
        let total = targets.len();
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
            for tab in &mut tabs {
                let Some((folder, format)) = targets.get(&tab.doc.meta.id) else {
                    continue;
                };
                let stem = crate::config::filename::sanitize_filename(tab.title.trim());
                let path = unique_file(folder, &stem, format.extension());
                let mut capture = tab.doc.base.clone();
                capture.image =
                    std::sync::Arc::new(crate::editor::render::render_to_image(&tab.doc));
                match crate::export::file::save_capture_to(&capture, &path, *format) {
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
            // front.
            if next == AppState::Viewer {
                self.request_focus(viewer_id());
                // The viewport's native window is destroyed on
                // deregistration and rebuilt on re-entry: the geometry
                // ledger must be re-applied.
                *lock(&self.viewer_sync) = ui::viewport_sync::ViewportSync::new("PixMe Viewer");
            }
        }
        // Restoring runs on EVERY viewer entry (guarded to once): a
        // same-state re-entry after reset must not skip the load.
        if next == AppState::Viewer {
            self.restore_session(ctx);
        }
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    }

    /// The stable id of the inventory monitor matching `m` by rect —
    /// xcap ids are unstable across enumerations on Windows.
    fn monitor_stable_id(&self, m: &MonitorInfo) -> Option<String> {
        lock(&self.monitors)
            .iter()
            .find(|id| id.info.rect == m.rect)
            .map(|id| id.stable_id.clone())
    }

    /// The landing target for `m`: the persisted native rect when prefs
    /// hold one for its stable id, else the centred/current-size default.
    fn viewer_target_for(&self, m: MonitorInfo) -> ViewerTarget {
        let native_rect = self.monitor_stable_id(&m).and_then(|id| {
            lock(&self.settings)
                .monitors
                .get(&id)
                .and_then(|p| p.rect_native)
        });
        ViewerTarget {
            monitor: m,
            native_rect,
        }
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

    /// Restores and raises the viewer: un-minimizes, activates the app
    /// and requests focus until it lands. Shared by `OpenViewer` and
    /// every command whose surface lives inside the viewer (settings,
    /// monitor selection) — `enter(Viewer)` alone never brings an
    /// already-open window forward.
    fn bring_viewer_forward(&self, ctx: &egui::Context) {
        ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Minimized(false));
        // macOS: `Focus`/`Minimized` viewport commands do not activate the
        // application — without `NSApp activate` the viewer stays behind
        // the frontmost app (dock reopen needed two clicks).
        platform::activate_app();
        // The direct Focus command both wakes the (possibly sleeping)
        // viewport and, on macOS, activates the app
        // (`activateIgnoringOtherApps` + `makeKeyAndOrderFront`) — a
        // mere in-app focus request would do nothing while another
        // application is frontmost.
        ctx.cmd_if_live(viewer_id(), egui::ViewportCommand::Focus);
        self.request_focus(viewer_id());
        // The modal lives in the VIEWER's pass — repainting the anchor
        // alone would leave an idle viewer asleep and the modal would
        // wait for the next input event.
        ctx.repaint_if_live(viewer_id());
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
    /// frozen monitor, full-screen arms an instant capture (the viewer must
    /// be torn down first — one warm-up frame inside the overlay/anchor pass).
    /// `Pin` lands in a later phase.
    fn begin_capture(&self, mode: CaptureMode, ctx: &egui::Context) {
        // Chained capture (menu/hotkey while a session is live): Esc must
        // land where the ORIGINAL session came from — `Capturing` itself
        // is never a return destination. Snapshot the state first: a
        // scrutinee-held guard would live for the whole match.
        let current = *lock(&self.state);
        // A delayed capture hid the viewer when its countdown was armed:
        // the stashed origin — not the countdown's `Idle` — is where Esc
        // lands. No stash: the live state decides (a chained capture
        // keeps the original session's destination).
        let return_to = lock(&self.capture_return_to)
            .take()
            .unwrap_or(match current {
                AppState::Capturing => lock(&self.capture)
                    .as_ref()
                    .map(|s| s.return_to)
                    .unwrap_or(AppState::Idle),
                state => state,
            });
        match mode {
            CaptureMode::Region
            | CaptureMode::FixedRegion
            | CaptureMode::FullScreen
            | CaptureMode::WindowOrObject
            | CaptureMode::Pin => {
                // The overlay initially opens on the SESSION monitor —
                // the viewer's own monitor, recorded by `push_capture`
                // — then the cursor's, the asking viewport's, or the
                // primary's. It still follows the cursor across
                // monitors until a selection starts.
                let monitor = lock(&self.capture_start_monitor)
                    .take()
                    .or_else(|| {
                        platform::cursor_position()
                            .and_then(|(x, y)| crate::capture::screen::monitor_at(x, y))
                    })
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
                // WDA_EXCLUDEFROMCAPTURE while the windows this capture
                // displaces (viewer, countdown card) still exist — a dead
                // hwnd can't be flagged and DWM honours the affinity at
                // its own cadence, so arming at BitBlt time loses the
                // race on fast machines. `warm_freeze` lifts it once the
                // frames are frozen; no-op on macOS.
                session.early_excl = Some(crate::platform::capture_exclusion());
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
        *lock(&self.viewer_target) = Some(self.viewer_target_for(frame.monitor.clone()));
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
        // A stray countdown stash must not leak into the next capture.
        *lock(&self.capture_return_to) = None;
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
                WriteEvent::MoveFailed { ids, revert_to } => {
                    tracing::warn!(
                        ids = ?ids,
                        workspace = %revert_to,
                        "workspace move rolled back"
                    );
                    lock(&self.viewer).revert_move(&ids, &revert_to);
                    crate::ui::widgets::toast(
                        ctx,
                        crate::ui::widgets::ToastKind::Error,
                        t!("workspace.move_failed").to_string(),
                    );
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
        // Flush a pending monitor-geometry save first — the debounce
        // timer may not have fired yet.
        if lock(&self.monitor_geom_dirty_at).take().is_some() {
            lock(&self.settings).save();
        }
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
        // The measured VACUUM rides the queue behind Finalize: it runs
        // inside the bounded quit wait, not in a Drop the exit may skip.
        self.writer.vacuum();
        *lock(&self.quit_rx) = Some(self.writer.flush());
        *lock(&self.quit_started) = Some(std::time::Instant::now());
        ctx.repaint_if_live(viewer_id());
    }
}

/// Returns the first `folder/stem.ext` that does not exist, suffixing
/// `_2`, `_3`… on collisions (same-name file already on disk — e.g. a
/// same-named capture saved from another workspace pointing here).
fn unique_file(folder: &std::path::Path, stem: &str, ext: &str) -> std::path::PathBuf {
    let candidate = folder.join(format!("{stem}.{ext}"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2u32.. {
        let path = folder.join(format!("{stem}_{n}.{ext}"));
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

/// Root application state, created once in [`eframe::App::ui`]'s setup.
pub struct PixMeApp {
    /// Everything the deferred-viewport callbacks reach.
    shared: Arc<Shared>,
    /// The second-instance wake listener (Windows named event) — held
    /// for the app lifetime so a relaunched `pixme.exe` surfaces this
    /// instance's viewer. `None` on macOS and on event-creation failure.
    _wakeup: Option<platform::WakeupServer>,
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
        // Windows: the named event a relaunched `pixme.exe` sets — the
        // listener queues `WakeViewer` exactly like a tray click queues
        // a menu command. macOS gets `None`: LaunchServices reopens the
        // running app itself, no second process exists.
        let wakeup = {
            let wake_ctx = cc.egui_ctx.clone();
            platform::serve_instance_wakeup(&crate::paths::instance_key(), move || {
                platform::queue_menu_command(Command::WakeViewer);
                wake_viewports(&wake_ctx);
            })
        };
        log_banner(&settings);
        // Dev/test hook: `PIXME_CAPTURE=region|fullscreen|fixed` fires that
        // capture at startup without going through the resident menu.
        let debug_capture = std::env::var("PIXME_CAPTURE")
            .ok()
            .and_then(|v| debug_mode(&v));
        // The launch landing: the viewer reopens on the monitor it last
        // sat on (stable id), at its saved rect when prefs hold one —
        // applied by the normal target path on the first viewer pass.
        let inventory = platform::monitor_identities();
        let initial_target = settings.last_monitor.as_deref().and_then(|id| {
            inventory
                .iter()
                .find(|m| m.stable_id == id)
                .map(|m| ViewerTarget {
                    monitor: m.info.clone(),
                    native_rect: settings.monitors.get(id).and_then(|p| p.rect_native),
                })
        });
        let app = Self {
            _wakeup: wakeup,
            shared: Arc::new(Shared {
                state: Mutex::new(AppState::Idle),
                last_language: Mutex::new(settings.language),
                last_menu_sync: Mutex::new((ModalState::None, false, false, false)),
                viewer_dialog_open: AtomicBool::new(false),
                settings: Mutex::new(settings),
                viewer: Mutex::new(Viewer::default()),
                viewer_rect: Mutex::new(None),
                viewer_target: Mutex::new(initial_target),
                viewer_monitor: Mutex::new(None),
                monitors: Mutex::new(inventory),
                viewer_sync: Mutex::new(ui::viewport_sync::ViewportSync::new("PixMe Viewer")),
                viewer_geo_settle: Mutex::new(0),
                viewer_topology_pin: Mutex::new(None),
                viewer_pin_last: Mutex::new(None),
                viewer_last_outer: Mutex::new(None),
                viewer_move_at: Mutex::new(None),
                draw_target: Mutex::new(None),
                draw_monitor: Mutex::new(None),
                draw_last_outer: Mutex::new(None),
                draw_move_at: Mutex::new(None),
                draw_geo_settle: Mutex::new(0),

                monitor_geom_dirty_at: Mutex::new(None),
                winit_placements: Mutex::new(Vec::new()),
                settings_open: AtomicBool::new(false),
                settings_draft: Mutex::new(None),
                capture: Mutex::new(None),
                countdown: Mutex::new(None),
                last_fixed_region: Mutex::new(None),
                capture_start_monitor: Mutex::new(None),
                capture_return_to: Mutex::new(None),
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
                monitors_rx: Mutex::new(None),
                display_poll: Mutex::new((platform::display_count(), std::time::Instant::now())),

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
                hotkeys: Mutex::new(None),
                hotkey_errors: Mutex::new(std::collections::BTreeMap::new()),
                hotkey_verify: Mutex::new(None),
            }),
        };
        // Global hotkeys: arm the configured combos, then park the
        // listener thread that pushes their Commands — same dispatch
        // path as the resident menu, woken the same way as the writer.
        if let Ok(mut manager) = crate::hotkeys::HotkeyManager::new() {
            let cfg = lock(&app.shared.settings).hotkeys.clone();
            *lock(&app.shared.hotkey_errors) = manager.sync(&cfg);
            let ids = manager.ids();
            *lock(&app.shared.hotkeys) = Some(manager);
            crate::hotkeys::listen(ids, app.shared.clone(), cc.egui_ctx.clone());
        } else {
            tracing::warn!("global-hotkey manager unavailable");
        }
        // Launch lands on the viewer, not resident-idle: it restores
        // the persisted session and gives a visible landing point.
        app.shared.enter(AppState::Viewer, &cc.egui_ctx);
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
    /// Transparent clear: the anchor, countdown and pinned windows are
    /// transparent, and child viewports paint their whole surface anyway.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        egui::Color32::TRANSPARENT.to_normalized_gamma_f32()
    }

    /// Last code that runs on every quit path — winit calls
    /// `process::exit` right after, before any destructor, so quit-time
    /// work cannot ride `Drop` or `Command::Quit` alone (menu Quit and
    /// the intercepted AppKit `terminate:` — Dock/Cmd+Q — dispatch the
    /// command, but the window cross and a hard exit never do). Replays
    /// `begin_quit`'s tail: pending-Draw commit, Finalize (meta refresh
    /// and stale-row prune), then the measured session-db VACUUM on the
    /// store thread, all inside one bounded flush wait.
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(mut draw) = lock(&self.shared.draw).take() {
            draw.finalize_pending();
            lock(&self.shared.viewer).commit_draw(draw.tab, draw.doc, draw.undo, draw.redo);
        }
        let (rows, live, active, next_title) = {
            let viewer = lock(&self.shared.viewer);
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
        self.shared.writer.finalize(rows, live, active, next_title);
        self.shared.writer.vacuum();
        match self.shared.writer.flush().recv_timeout(QUIT_FLUSH_TIMEOUT) {
            Ok(()) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => tracing::warn!(
                "session flush exceeded {}s — quitting anyway",
                QUIT_FLUSH_TIMEOUT.as_secs()
            ),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                tracing::warn!("store writer died before the quit flush")
            }
        }
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
            AppState::Viewer => {
                let shared = Arc::clone(&self.shared);
                // If a capture just landed on a specific monitor, make sure
                // the viewer opens on that monitor (preserving its relative
                // placement / size when already known, or centring it there).
                if let Some(t) = lock(&shared.viewer_target).clone() {
                    // The window is being (re)built ON the target monitor:
                    // its ppp is the monitor's own scale.
                    let next = target_ui_rect(
                        &t,
                        *lock(&shared.viewer_rect),
                        t.monitor.scale_factor.max(1.0),
                    );
                    *lock(&shared.viewer_rect) = Some(next);
                }
                let builder = viewer_builder(lock(&shared.viewer_rect).map(|r| r.min));
                ctx.show_viewport_deferred(viewer_id(), builder, move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, viewer_id());
                    let target = lock(&shared.viewer_target).take();
                    if let Some(t) = target {
                        // Commands are converted back by THIS window's
                        // current ppp — convert with it so the physical
                        // target is exact even mid-DPI-change.
                        let ppp = platform::window_ppp(&ctx, t.monitor.scale_factor.max(1.0));
                        let next = target_ui_rect(&t, *lock(&shared.viewer_rect), ppp);
                        *lock(&shared.viewer_rect) = Some(next);
                        *lock(&shared.viewer_monitor) = Some(t.monitor);
                        // `next` is the OUTER rect: `InnerSize` needs the
                        // decoration delta (outer − inner reported last
                        // frame; zero until measured).
                        let deco = ctx.input(|i| {
                            match (i.viewport().outer_rect, i.viewport().inner_rect) {
                                (Some(o), Some(n)) => o.size() - n.size(),
                                _ => egui::Vec2::ZERO,
                            }
                        });
                        let inner = (next.size() - deco).max(egui::vec2(100.0, 100.0));
                        {
                            let mut sync = lock(&shared.viewer_sync);
                            sync.request_outer_position(&ctx, next.min);
                            sync.request_inner_size(&ctx, inner);
                        }
                        // The reported rect lies for a frame or two while
                        // the move (and any DPI hand-off) settles — never
                        // persist it.
                        *lock(&shared.viewer_geo_settle) = 2;
                    } else if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                        let mut settle = lock(&shared.viewer_geo_settle);
                        if *settle > 0 {
                            *settle -= 1;
                        } else {
                            let ppp = platform::window_ppp(&ctx, 1.0);
                            // The viewer is freely draggable — across
                            // monitors too. A move that settles on a
                            // DIFFERENT monitor becomes a relocation:
                            // that monitor's memorized rect (or the
                            // centred fallback) is applied through the
                            // same target path as `SelectMonitor`. A
                            // stale current monitor (disconnected)
                            // falls back to the monitor under the
                            // centre.
                            let centre = platform::ui_to_native(outer.center(), ppp);
                            // Cached-inventory hit-test, never a fresh xcap
                            // enumeration: `monitor_at` calls Monitor::all()
                            // EVERY FRAME on the main thread — during a
                            // display reconfiguration (iPad plug/unplug)
                            // that blocks and freezes the UI. The async
                            // refresh keeps the inventory current instead.
                            let hit_rect = {
                                let inv = lock(&shared.monitors);
                                inv.iter()
                                    .find(|id| {
                                        let r = &id.info.rect;
                                        let (x0, y0) = (r.x as f32, r.y as f32);
                                        let (x1, y1) = (
                                            (r.x + r.width as i32) as f32,
                                            (r.y + r.height as i32) as f32,
                                        );
                                        centre.x >= x0
                                            && centre.x < x1
                                            && centre.y >= y0
                                            && centre.y < y1
                                    })
                                    .map(|id| id.info.clone())
                            };
                            let hit = hit_rect.clone();
                            let cur = lock(&shared.viewer_monitor).clone().filter(|c| {
                                lock(&shared.monitors)
                                    .iter()
                                    .any(|id| id.info.rect == c.rect)
                            });
                            // Topology pin window: macOS re-shuffles the
                            // window asynchronously after a plug/unplug.
                            // The saved rect is pushed DIRECTLY here (not
                            // via viewer_target — that path's settle
                            // counter pauses this loop for two frames and
                            // would let a late shuffle stick).
                            let pinned = lock(&shared.viewer_topology_pin)
                                .is_some_and(|until| std::time::Instant::now() < until);
                            // The hosting monitor: the one under the
                            // window centre in normal time — the viewer
                            // drags freely and follows wherever it is
                            // dropped. While the pin owns corrections
                            // (topology turbulence), the PREVIOUS host is
                            // preferred so a mid-shuffle centre dumped
                            // onto another display can't steal the
                            // window. `or(cur)` plus the primary/live
                            // fallback cover a centre landing in a
                            // monitor gap or the host being gone.
                            let bounds = if pinned {
                                cur.clone().or_else(|| hit.clone())
                            } else {
                                hit.clone().or_else(|| cur.clone())
                            }
                            .or_else(|| {
                                let inv = lock(&shared.monitors);
                                inv.iter()
                                    .find(|id| id.info.is_primary)
                                    .or_else(|| inv.first())
                                    .map(|id| id.info.clone())
                            });
                            // The hosting monitor VANISHED (unplug): the
                            // OS dumps the window wherever — run the same
                            // target path as `SelectMonitor` so the new
                            // monitor's saved rect is restored. Gated on
                            // `cur` being gone: a normal cross-monitor
                            // drop changes `bounds` too but must NOT
                            // relocate — free drag wins, and its landing
                            // rect is persisted on settle below.
                            let monitor_changed = cur.is_none()
                                && bounds.as_ref().is_some_and(|m| {
                                    lock(&shared.viewer_monitor).as_ref().map(|v| v.rect)
                                        != Some(m.rect)
                                });
                            if monitor_changed && let Some(m) = &bounds {
                                *lock(&shared.viewer_target) =
                                    Some(shared.viewer_target_for(m.clone()));
                            }
                            // Gesture tracking: every reported-rect
                            // change restarts a 250 ms quiet timer —
                            // a move/resize is "done" once it settles.
                            // While the pin owns corrections (topology
                            // shuffle), no gesture is tracked.
                            let moved =
                                lock(&shared.viewer_last_outer).replace(outer) != Some(outer);
                            if pinned {
                                *lock(&shared.viewer_move_at) = None;
                            } else if moved {
                                *lock(&shared.viewer_move_at) = Some(std::time::Instant::now());
                            }
                            // Copy the Option OUT first: keeping the
                            // scrutinee guard alive through the `if`
                            // body self-deadlocks the clear below.
                            let move_at = *lock(&shared.viewer_move_at);
                            if !pinned && let Some(t) = move_at {
                                if t.elapsed() >= std::time::Duration::from_millis(250) {
                                    *lock(&shared.viewer_move_at) = None;
                                    // Gesture over — the ONE geometry
                                    // write (nothing persists mid-drag):
                                    // the dropped rect goes under the
                                    // monitor the window ACTUALLY sits
                                    // on, so the monitor buttons always
                                    // switch to the last real position
                                    // of each display. Skipped while an
                                    // unplug restore is armed — the
                                    // dumped rect is transient.
                                    if !monitor_changed
                                        && let Some(m) = hit.clone().or_else(|| cur.clone())
                                        && let Some(id) = shared.monitor_stable_id(&m)
                                    {
                                        let rect = clamped_native_rect(outer, ppp, &m.rect);
                                        tracing::debug!(
                                            monitor = m.name.as_str(),
                                            ppp,
                                            outer = ?outer,
                                            rect = ?rect,
                                            "monitor_geometry_persist"
                                        );
                                        let mut s = lock(&shared.settings);
                                        let prefs = s.monitors.entry(id.clone()).or_default();
                                        let mut dirty = prefs.rect_native != Some(rect);
                                        if dirty {
                                            prefs.rect_native = Some(rect);
                                        }
                                        if s.last_monitor.as_deref() != Some(id.as_str()) {
                                            s.last_monitor = Some(id);
                                            dirty = true;
                                        }
                                        if dirty {
                                            *lock(&shared.monitor_geom_dirty_at) =
                                                Some(std::time::Instant::now());
                                        }
                                    }
                                } else {
                                    // Poll so the settle check still runs
                                    // once the OS stops sending move events.
                                    ctx.request_repaint_after(std::time::Duration::from_millis(50));
                                }
                            }
                            let mut eff = outer;
                            if let Some(m) = &bounds {
                                // Topology pin: macOS re-shuffles the
                                // window asynchronously after a plug/
                                // unplug — often WITHIN the same monitor
                                // (it dumps the window at bottom-left).
                                // Pull it back whenever the reported
                                // rect deviates from the saved one,
                                // throttled to 300 ms: a per-frame push
                                // created a position feedback storm
                                // (egui/winit fight → full freeze).
                                if pinned {
                                    let saved = target_ui_rect(
                                        &shared.viewer_target_for(m.clone()),
                                        *lock(&shared.viewer_rect),
                                        ppp,
                                    );
                                    // Sliding pin: macOS re-shuffles
                                    // asynchronously for several seconds
                                    // after a replug. Detection runs
                                    // EVERY frame (unthrottled) so the
                                    // window can't expire while it is
                                    // still displaced; only the push is
                                    // throttled to 200 ms.
                                    if saved.min.distance(outer.min) > 0.5 {
                                        *lock(&shared.viewer_topology_pin) = Some(
                                            std::time::Instant::now()
                                                + std::time::Duration::from_secs(2),
                                        );
                                        if lock(&shared.viewer_pin_last).as_ref().is_none_or(|t| {
                                            t.elapsed() >= std::time::Duration::from_millis(200)
                                        }) {
                                            *lock(&shared.viewer_pin_last) =
                                                Some(std::time::Instant::now());
                                            ctx.send_viewport_cmd(
                                                egui::ViewportCommand::OuterPosition(saved.min),
                                            );
                                            eff = saved;
                                        }
                                    }
                                }
                            }
                            *lock(&shared.viewer_monitor) = bounds;
                            // `last_monitor` is written in the settle
                            // persist above — NOT per frame: tracking
                            // the physical host mid-drag would flag it
                            // as the "last" monitor before the gesture
                            // even ends.
                            // `viewer_rect` tracks the reported rect —
                            // per-monitor `rect_native` persistence does
                            // NOT happen here: it is written exactly
                            // once, when the move/resize gesture settles
                            // (the `viewer_move_at` quiet check above).
                            if *lock(&shared.viewer_rect) != Some(eff) {
                                *lock(&shared.viewer_rect) = Some(eff);
                            }
                        }
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
                });
            }
            AppState::Capturing => {
                // The freeze happens here, before any capture window exists:
                // anchor frames let the viewer tear down, then the screen is
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
                            // The freeze waits for the REAL teardown of
                            // the windows the capture displaced — a fast
                            // machine counts two warm frames inside one
                            // DWM composition and the BitBlt still reads
                            // the dying viewer's pixels (the "ghost"
                            // watermark). The viewport id leaves
                            // `raw.viewports` once winit confirms the
                            // native window is gone.
                            let gone = ctx.input(|i| {
                                !i.raw.viewports.contains_key(&viewer_id())
                                    && !i.raw.viewports.contains_key(&countdown_id())
                            });
                            s.warm_freeze(gone);
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
        // state machine — it can run on top of the viewer or an empty
        // desktop. It repaints itself to zero, then fires CaptureNow.
        if lock(&self.shared.countdown).is_some() {
            let shared = Arc::clone(&self.shared);
            ctx.show_viewport_deferred(countdown_id(), countdown_builder(), move |ui, _class| {
                let ctx = ui.ctx().clone();
                shared.pump(&ctx);
                ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                // Esc cancels the pending capture.
                if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                    *lock(&shared.countdown) = None;
                    // The countdown hid the viewer on arm — cancelling
                    // puts it back.
                    if let Some(state) = lock(&shared.capture_return_to).take() {
                        shared.enter(state, &ctx);
                    }
                    ctx.request_repaint_of(egui::ViewportId::ROOT);
                    return;
                }
                // The countdown parks on the viewer's monitor — resolved
                // in native units by the owner window, so no two windows'
                // point spaces are ever mixed here.
                let anchor = lock(&shared.viewer_monitor).clone();
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
                // Geometry, same convention as the viewer: the stashed
                // target is applied once at open (saved `draw_rect_native`
                // or centred on the host monitor), then a 250 ms quiet
                // after the last reported-rect change settles a gesture
                // into the host monitor's persisted rect.
                let target = lock(&shared.draw_target).take();
                if let Some(t) = target {
                    let ppp = platform::window_ppp(&ctx, t.monitor.scale_factor.max(1.0));
                    let rect = match t.native_rect {
                        Some(_) => target_ui_rect(&t, None, ppp),
                        // Nothing persisted for this monitor: centre the
                        // window on it at its current (or builder's) size.
                        None => {
                            let mon = platform::monitor_ui_rect(&t.monitor, ppp);
                            let size = ctx
                                .input(|i| i.viewport().outer_rect)
                                .map(|r| r.size())
                                .unwrap_or(egui::vec2(1100.0, 700.0))
                                .min(mon.size());
                            egui::Rect::from_center_size(mon.center(), size)
                        }
                    };
                    // `rect` is the OUTER rect: `InnerSize` needs the
                    // decoration delta (outer − inner reported last
                    // frame; zero until measured).
                    let deco =
                        ctx.input(
                            |i| match (i.viewport().outer_rect, i.viewport().inner_rect) {
                                (Some(o), Some(n)) => o.size() - n.size(),
                                _ => egui::Vec2::ZERO,
                            },
                        );
                    ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(rect.min));
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(
                        (rect.size() - deco).max(egui::vec2(100.0, 100.0)),
                    ));
                    *lock(&shared.draw_monitor) = Some(t.monitor);
                    // The reported rect lies for a frame or two while the
                    // move settles — never persist it.
                    *lock(&shared.draw_geo_settle) = 2;
                } else if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                    let mut settle = lock(&shared.draw_geo_settle);
                    if *settle > 0 {
                        *settle -= 1;
                    } else {
                        drop(settle);
                        let moved = lock(&shared.draw_last_outer).replace(outer) != Some(outer);
                        if moved {
                            *lock(&shared.draw_move_at) = Some(std::time::Instant::now());
                        }
                        let move_at = *lock(&shared.draw_move_at);
                        if let Some(t) = move_at {
                            if t.elapsed() >= std::time::Duration::from_millis(250) {
                                *lock(&shared.draw_move_at) = None;
                                // Gesture over — the ONE geometry write:
                                // persisted under the monitor the window's
                                // centre actually sits on (free cross-monitor
                                // drag, same rule as the viewer).
                                let ppp = platform::window_ppp(&ctx, 1.0);
                                let centre = platform::ui_to_native(outer.center(), ppp);
                                let host = {
                                    let inv = lock(&shared.monitors);
                                    inv.iter()
                                        .find(|id| {
                                            let r = &id.info.rect;
                                            centre.x >= r.x as f32
                                                && centre.x < (r.x + r.width as i32) as f32
                                                && centre.y >= r.y as f32
                                                && centre.y < (r.y + r.height as i32) as f32
                                        })
                                        .map(|id| id.info.clone())
                                        .or_else(|| lock(&shared.draw_monitor).clone())
                                };
                                if let Some(m) = host
                                    && let Some(id) = shared.monitor_stable_id(&m)
                                {
                                    *lock(&shared.draw_monitor) = Some(m.clone());
                                    let rect = clamped_native_rect(outer, ppp, &m.rect);
                                    let mut s = lock(&shared.settings);
                                    let prefs = s.monitors.entry(id).or_default();
                                    if prefs.draw_rect_native != Some(rect) {
                                        prefs.draw_rect_native = Some(rect);
                                        *lock(&shared.monitor_geom_dirty_at) =
                                            Some(std::time::Instant::now());
                                    }
                                }
                            } else {
                                // Poll so the settle check still runs
                                // once the OS stops sending move events.
                                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                            }
                        }
                    }
                }
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
                    // The user's last tool styling survives to the next
                    // Draw session — same convention as the text_*
                    // fields, whichever way the window closed.
                    session.persist_opts(&shared);
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

        // The fullscreen pick overlays: raised while a colour pick is
        // armed and the cursor roams outside the Draw window — one
        // viewport per monitor, so every click lands on our chrome and
        // the app underneath never sees the validating press. A single
        // union-spanning window would not work: AppKit clips it to one
        // screen's visibleFrame, leaving a non-rectangular layout
        // (e.g. Sidecar iPad below-left) partly uncovered. Each window
        // is re-pinned to its monitor's rect every frame (a plug/unplug
        // or DPI shift mid-pick cannot leave a strip uncovered).
        if lock(&self.shared.draw)
            .as_ref()
            .is_some_and(|d| d.pick_overlay_active())
        {
            for (i, rect) in cached_pick_monitors(&ctx).into_iter().enumerate() {
                let shared = Arc::clone(&self.shared);
                ctx.show_viewport_deferred(
                    pick_overlay_id(i),
                    pick_overlay_builder(rect),
                    move |ui, _class| {
                        let ctx = ui.ctx().clone();
                        shared.pump(&ctx);
                        ui::theme::apply(&ctx, shared.appearance_kind(&ctx));
                        let (l, t, r, b, _scale) = rect;
                        let ppp = platform::window_ppp(&ctx, 1.0);
                        let want = egui::Rect::from_min_max(
                            platform::native_to_ui(egui::pos2(l as f32, t as f32), ppp),
                            platform::native_to_ui(egui::pos2(r as f32, b as f32), ppp),
                        );
                        // Tolerance: float round-trips never compare equal,
                        // so commands would be re-sent every single frame.
                        let off = ctx
                            .input(|i| i.viewport().outer_rect)
                            .map(|cur| {
                                cur.min.distance(want.min) > 0.5
                                    || (cur.size() - want.size()).length() > 0.5
                            })
                            .unwrap_or(true);
                        if off {
                            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(want.size()));
                            // AppKit can move the top edge while resizing a
                            // window that spans monitors. Position it last.
                            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(want.min));
                        }
                        // Above the Dock and menu bar — macOS floats
                        // `with_always_on_top` at level 3, under system
                        // chrome, and a click there would reach it.
                        platform::elevate_pick_overlay();
                        ui::draw::show_pick_overlay(ui);
                    },
                );
            }
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

/// The viewer window: decorated, resizable, the app's real home.
fn viewer_builder(position: Option<egui::Pos2>) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(t!("app.title"))
        .with_icon(ui::icons::app_icon_data())
        .with_app_id("pixme")
        .with_inner_size([860.0, 560.0])
        .with_min_inner_size([580.0, 420.0])
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

/// The viewer's landing rect in the target window's UI units: the
/// persisted native outer rect converted with `ppp` when one was saved
/// for the monitor, else the keep-if-inside / centred placement.
fn target_ui_rect(t: &ViewerTarget, current: Option<egui::Rect>, ppp: f32) -> egui::Rect {
    match t.native_rect {
        Some([x, y, w, h]) => {
            let min = platform::native_to_ui(egui::pos2(x as f32, y as f32), ppp);
            let size = platform::native_to_ui(egui::pos2(w as f32, h as f32), ppp).to_vec2();
            // Defensive clamp: a stale/corrupt persisted rect (written
            // during a mixed-DPI transition) must never land the window
            // larger than the monitor or off-screen. The real rect is
            // persisted on the next settle, so the entry self-heals.
            let mon = platform::monitor_ui_rect(&t.monitor, ppp);
            let size = size.min(mon.size()).max(egui::vec2(100.0, 100.0));
            let min = min.clamp(mon.min, mon.max - size);
            egui::Rect::from_min_size(min, size)
        }
        None => place_viewer_on_monitor(current, &t.monitor, ppp),
    }
}

/// The reported `outer` rect converted to the monitor's native units and
/// clamped inside `mon`'s bounds — shared by the per-monitor geometry
/// persistence of every window role (viewer `rect_native`, Draw editor
/// `draw_rect_native`). The clamp is the DPI-transition guard: during a
/// mixed-DPI transition `outer` and `ppp` can be mismatched (old points
/// × new scale), persisting a rect inflated by the ratio — re-applied on
/// the next relocation it compounds (the window grows at every
/// cross-DPI move). The real rect is always inside the host's native
/// bounds, so a clamped write is never lossy here.
fn clamped_native_rect(
    outer: egui::Rect,
    ppp: f32,
    mon: &crate::capture::PhysicalRect,
) -> [i32; 4] {
    let n_min = platform::ui_to_native(outer.min, ppp);
    let n_max = platform::ui_to_native(outer.max, ppp);
    let w = (n_max.x - n_min.x).clamp(100.0, mon.width as f32);
    let h = (n_max.y - n_min.y).clamp(100.0, mon.height as f32);
    let x = n_min
        .x
        .clamp(mon.x as f32, mon.x as f32 + mon.width as f32 - w);
    let y = n_min
        .y
        .clamp(mon.y as f32, mon.y as f32 + mon.height as f32 - h);
    [x as i32, y as i32, w as i32, h as i32]
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

/// The fullscreen pick overlay: borderless, transparent, always on top —
/// the whole virtual screen is its canvas so no click escapes it, and an
/// alpha-1 veil inside keeps transparent pixels hit-testable. Raised
/// only while a colour pick is armed and the cursor roams outside the
/// Draw window.
/// Each monitor's rect + scale factor, cached 1 s in ctx data —
/// monitor enumeration is a WindowServer round-trip, too heavy to run
/// every ROOT pass. The same key feeds every overlay's per-frame rect
/// correction, so all viewports share one enumeration.
fn cached_pick_monitors(ctx: &egui::Context) -> Vec<(i32, i32, i32, i32, f32)> {
    ctx.data_mut(|d| {
        let id = egui::Id::new("pick_overlay_monitors");
        if let Some((_, v)) = d
            .get_temp::<(std::time::Instant, Vec<(i32, i32, i32, i32, f32)>)>(id)
            .filter(|(at, _)| at.elapsed().as_millis() < 1000)
        {
            return v;
        }
        let v: Vec<_> = crate::capture::screen::monitors()
            .unwrap_or_default()
            .into_iter()
            .map(|m| {
                (
                    m.rect.x,
                    m.rect.y,
                    m.rect.x + m.rect.width as i32,
                    m.rect.y + m.rect.height as i32,
                    m.scale_factor.max(1.0),
                )
            })
            .collect();
        d.insert_temp(id, (std::time::Instant::now(), v.clone()));
        v
    })
}

fn pick_overlay_builder(rect: (i32, i32, i32, i32, f32)) -> egui::ViewportBuilder {
    // Created AT its monitor's rect: a mid-creation 800×600 would leave
    // screen strips uncovered (clicks escape, loupe clipped) until the
    // per-frame correction converged — worse, every close/reopen cycle
    // replayed that phase. Residual error is still corrected per frame.
    let (l, t, r, b, ppp) = rect;
    let size = platform::native_to_ui(egui::pos2((r - l) as f32, (b - t) as f32), ppp).to_vec2();
    let pos = platform::native_to_ui(egui::pos2(l as f32, t as f32), ppp);
    let builder = egui::ViewportBuilder::default()
        .with_title("PixMe Pick")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(true)
        .with_inner_size(size)
        .with_position(pos);
    #[cfg(target_os = "windows")]
    // Hidden until `strip_undecorated_frame` shows it — winit forces
    // WS_CAPTION at creation; `egui`'s `Visible(true)` would re-apply it.
    let builder = builder.with_visible(false);
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
    // Hidden until `strip_undecorated_frame` shows it — winit forces
    // WS_CAPTION at creation; `egui`'s `Visible(true)` would re-apply it.
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
    // Hidden until `strip_undecorated_frame` shows it — winit forces
    // WS_CAPTION at creation; `egui`'s `Visible(true)` would re-apply it.
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
            // Hidden at creation on Windows only: the strip guard applies
            // the empty region BEFORE the first ShowWindow — killing the
            // startup-race ring. macOS keeps it visible: an invisible
            // ROOT drops repaint requests and starves the command pump
            // (queued resident-icon commands needed two clicks).
            .with_visible(cfg!(target_os = "macos"))
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
            state: Mutex::new(AppState::Idle),
            settings: Mutex::new(Settings::default()),
            viewer: Mutex::new(Viewer::default()),
            viewer_rect: Mutex::new(None),
            viewer_target: Mutex::new(None),
            viewer_monitor: Mutex::new(None),
            monitors: Mutex::new(Vec::new()),
            viewer_sync: Mutex::new(ui::viewport_sync::ViewportSync::new("PixMe Viewer")),
            viewer_geo_settle: Mutex::new(0),
            viewer_topology_pin: Mutex::new(None),
            viewer_pin_last: Mutex::new(None),
            viewer_last_outer: Mutex::new(None),
            viewer_move_at: Mutex::new(None),
            draw_target: Mutex::new(None),
            draw_monitor: Mutex::new(None),
            draw_last_outer: Mutex::new(None),
            draw_move_at: Mutex::new(None),
            draw_geo_settle: Mutex::new(0),

            monitor_geom_dirty_at: Mutex::new(None),
            winit_placements: Mutex::new(Vec::new()),
            settings_open: AtomicBool::new(false),
            settings_draft: Mutex::new(None),
            capture: Mutex::new(None),
            countdown: Mutex::new(None),
            last_fixed_region: Mutex::new(None),
            capture_start_monitor: Mutex::new(None),
            capture_return_to: Mutex::new(None),
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
            monitors_rx: Mutex::new(None),
            display_poll: Mutex::new((platform::display_count(), std::time::Instant::now())),

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
            hotkeys: Mutex::new(None),
            hotkey_errors: Mutex::new(std::collections::BTreeMap::new()),
            hotkey_verify: Mutex::new(None),
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
        // enumeration may fail instead: the machine must then stay in `Idle`.
        let (shared, ctx) = harness();
        let state = drive(&shared, &ctx, Command::Capture(CaptureMode::Region));
        assert!(
            matches!(state, AppState::Capturing | AppState::Idle),
            "unexpected state {state:?}"
        );
    }

    #[test]
    fn closing_viewer_returns_to_idle_keeping_session() {
        let (shared, ctx) = harness();
        lock(&shared.viewer).push_stub("capture 1");
        drive(&shared, &ctx, Command::OpenViewer);
        assert_eq!(drive(&shared, &ctx, Command::ViewerClosed), AppState::Idle);
        assert_eq!(lock(&shared.viewer).tabs.len(), 1);
    }

    /// The resident icon's left click toggles: opens the viewer from
    /// Idle, closes it back to the resident icon when it is up and
    /// focused.
    #[test]
    fn toggle_viewer_opens_and_closes() {
        let (shared, ctx) = harness();
        assert_eq!(
            drive(&shared, &ctx, Command::ToggleViewer),
            AppState::Viewer
        );
        // The viewer reports itself focused — the click closes it.
        ctx.input_mut(|i| {
            i.raw.viewports.entry(viewer_id()).or_default().focused = Some(true);
        });
        assert_eq!(drive(&shared, &ctx, Command::ToggleViewer), AppState::Idle);
    }

    /// Up but merely behind another app — the click raises it (open
    /// semantics), never closes it into `Idle`.
    #[test]
    fn toggle_viewer_behind_raises_instead_of_closing() {
        let (shared, ctx) = harness();
        drive(&shared, &ctx, Command::OpenViewer);
        ctx.input_mut(|i| {
            i.raw.viewports.entry(viewer_id()).or_default().focused = Some(false);
        });
        assert_eq!(
            drive(&shared, &ctx, Command::ToggleViewer),
            AppState::Viewer
        );
    }

    /// A second-instance relaunch surfaces the viewer from Idle like
    /// `OpenViewer` — and stays legal while a modal owns the UI (the
    /// re-raised modal rides above the viewer).
    #[test]
    fn wake_viewer_surfaces_from_idle() {
        let (shared, ctx) = harness();
        assert_eq!(drive(&shared, &ctx, Command::WakeViewer), AppState::Viewer);
    }

    /// The wake ruling: every modal but a live capture — stealing the
    /// overlay's focus would eat its Esc/clicks — and never the quit
    /// flush (`Saving` rejects everything anyway).
    #[test]
    fn wake_viewer_ruling() {
        for modal in [
            ModalState::None,
            ModalState::Settings,
            ModalState::Draw,
            ModalState::SaveDialog,
            ModalState::ViewerDialog,
        ] {
            assert!(
                is_command_enabled(&Command::WakeViewer, modal),
                "WakeViewer should be enabled under {modal:?}"
            );
        }
        for modal in [ModalState::Capturing, ModalState::Saving] {
            assert!(
                !is_command_enabled(&Command::WakeViewer, modal),
                "WakeViewer must stay blocked under {modal:?}"
            );
        }
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

    /// With retention set to `Never`, a fresh capture marks the session
    /// dirty and `Command::Quit` arms the Save/Discard/Cancel modal.
    #[test]
    fn quit_with_never_retention_and_fresh_capture_shows_confirmation() {
        let (shared, ctx) = harness();
        lock(&shared.settings).db_retention = crate::config::DbRetention::Never;
        {
            let mut viewer = lock(&shared.viewer);
            viewer.push_capture_document(
                Document::new(synthetic_capture(image::RgbaImage::new(4, 4))),
                "shot".to_owned(),
            );
            assert!(viewer.dirty, "fresh capture must be dirty");
        }
        let state = drive(&shared, &ctx, Command::Quit);
        assert_eq!(state, AppState::Viewer);
        assert!(
            lock(&shared.viewer).confirm_close,
            "confirmation must be armed for a dirty Never-retention session"
        );
    }

    /// `ConfirmSave` on quit must save unsaved captures across ALL
    /// workspaces (each to its own `output_dir`), even when the active
    /// workspace is empty — unlike toolbar Save All, which only writes
    /// the visible workspace.
    #[test]
    fn confirm_save_on_quit_saves_unsaved_captures_across_all_workspaces() {
        let (shared, ctx) = harness();
        let base = std::env::temp_dir().join(format!(
            "pixme-confirm-save-ws-{}",
            crate::editor::meta::new_id()
        ));
        let dir_main = base.join("main");
        let dir_bee = base.join("bee");
        let dir_empty = base.join("empty");
        std::fs::create_dir_all(&dir_main).unwrap();
        std::fs::create_dir_all(&dir_bee).unwrap();
        std::fs::create_dir_all(&dir_empty).unwrap();

        let (bee_id, empty_id) = {
            let mut s = lock(&shared.settings);
            s.db_retention = crate::config::DbRetention::Never;
            s.active_workspace_mut().output_dir = Some(dir_main.clone());

            let mut bee = crate::config::workspace::Workspace::new("Bee".to_owned());
            bee.output_dir = Some(dir_bee.clone());
            let bee_id = bee.id.clone();
            s.workspaces.push(bee);

            let mut empty = crate::config::workspace::Workspace::new("Empty".to_owned());
            empty.output_dir = Some(dir_empty.clone());
            let empty_id = empty.id.clone();
            s.workspaces.push(empty);
            (bee_id, empty_id)
        };

        // Push an unsaved capture in `main`.
        {
            let mut viewer = lock(&shared.viewer);
            viewer.sync_workspace(&lock(&shared.settings));
            viewer.push_capture_document(
                Document::new(synthetic_capture(image::RgbaImage::new(4, 4))),
                "main_shot".to_owned(),
            );
        }
        // Switch to `Bee` and push another unsaved capture.
        drive(&shared, &ctx, Command::SwitchWorkspace(bee_id));
        {
            let mut viewer = lock(&shared.viewer);
            viewer.sync_workspace(&lock(&shared.settings));
            viewer.push_capture_document(
                Document::new(synthetic_capture(image::RgbaImage::new(4, 4))),
                "bee_shot".to_owned(),
            );
        }
        // Switch to `Empty` (0 visible tabs) and quit → ConfirmSave.
        drive(&shared, &ctx, Command::SwitchWorkspace(empty_id));
        drive(&shared, &ctx, Command::Quit);
        assert!(lock(&shared.viewer).confirm_close);
        drive(&shared, &ctx, Command::ConfirmSave);
        drain_save_all(&shared);

        assert!(
            dir_main.join("main_shot.png").exists(),
            "main workspace capture must be saved to main's output_dir"
        );
        assert!(
            dir_bee.join("bee_shot.png").exists(),
            "bee workspace capture must be saved to bee's output_dir"
        );
        assert!(
            !lock(&shared.viewer).dirty,
            "session must be clean after ConfirmSave completes"
        );
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Same-named captures in different workspaces sharing one folder
    /// are NOT a clash — each writes under its workspace's `format`,
    /// and a real filename collision suffixes `_2` on disk.
    #[test]
    fn save_all_writes_workspace_format_and_suffixes_shared_folder() {
        let (shared, ctx) = harness();
        let dir = std::env::temp_dir().join(format!(
            "pixme-save-twins-{}",
            crate::editor::meta::new_id()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let bee_id = {
            let mut s = lock(&shared.settings);
            s.active_workspace_mut().output_dir = Some(dir.clone());
            let mut bee = crate::config::workspace::Workspace::new("Bee".to_owned());
            bee.output_dir = Some(dir.clone());
            bee.format = crate::export::ImageFormat::Jpeg;
            let bee_id = bee.id.clone();
            s.workspaces.push(bee);
            bee_id
        };
        let push_a = |shared: &Shared| {
            lock(&shared.viewer).push_capture_document(
                Document::new(synthetic_capture(image::RgbaImage::new(4, 4))),
                "a".to_owned(),
            );
        };
        {
            let mut viewer = lock(&shared.viewer);
            viewer.sync_workspace(&lock(&shared.settings));
        }
        push_a(&shared);
        drive(&shared, &ctx, Command::SwitchWorkspace(bee_id.clone()));
        {
            let mut viewer = lock(&shared.viewer);
            viewer.sync_workspace(&lock(&shared.settings));
        }
        push_a(&shared);

        // Bee's Save All writes `a.jpg` (its own format); main's writes
        // `a.png` — different extensions, no collision, no dup modal.
        drive(&shared, &ctx, Command::SaveAll);
        drain_save_all(&shared);
        drive(
            &shared,
            &ctx,
            Command::SwitchWorkspace(crate::config::workspace::MAIN_ID.to_owned()),
        );
        drive(&shared, &ctx, Command::SaveAll);
        drain_save_all(&shared);
        assert!(dir.join("a.jpg").exists(), "bee writes JPEG per its format");
        assert!(dir.join("a.png").exists(), "main writes PNG");
        assert!(
            !lock(&shared.viewer).dup_names,
            "cross-workspace twins never raise the rename modal"
        );

        // Same format on both sides → the twin suffixes on disk.
        {
            let mut s = lock(&shared.settings);
            s.workspaces
                .iter_mut()
                .find(|w| w.id == bee_id)
                .unwrap()
                .format = crate::export::ImageFormat::Png;
        }
        drive(&shared, &ctx, Command::SwitchWorkspace(bee_id));
        {
            // The command only writes prefs — resync before pushing so
            // the capture lands in `bee`, like the app's own pre-command
            // sync does.
            let mut viewer = lock(&shared.viewer);
            viewer.sync_workspace(&lock(&shared.settings));
        }
        push_a(&shared);
        drive(&shared, &ctx, Command::SaveAll);
        drain_save_all(&shared);
        assert!(
            dir.join("a_2.png").exists(),
            "same folder + same format suffixes the twin"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
            Command::OpenViewer,
            Command::ToggleViewer,
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
            Command::SelectMonitor(String::new()),
            Command::RefreshMonitors,
            Command::RenameMonitor(String::new()),
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
            (Command::OpenViewer, true, false, false, false),
            (Command::ToggleViewer, true, false, false, false),
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
            (
                Command::SelectMonitor(String::new()),
                false,
                false,
                false,
                false,
            ),
            (Command::RefreshMonitors, false, false, false, false),
            (
                Command::RenameMonitor(String::new()),
                false,
                false,
                false,
                false,
            ),
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
    /// duplicate-name clash during ConfirmSave does NOT block the quit:
    /// the save-at-all-costs path suffixes the twin (`dup_2.png`).
    #[test]
    fn save_all_renders_annotations_and_confirm_save_suffixes_clash() {
        let (shared, ctx) = harness();
        let dir =
            std::env::temp_dir().join(format!("pixme-save-all-{}", crate::editor::meta::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        lock(&shared.settings).active_workspace_mut().output_dir = Some(dir.clone());
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
                tool: None,
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
        drain_save_all(&shared);

        // ConfirmSave never blocks on same-workspace twins: `dup.png`
        // (with the red annotation baked in) and `dup_2.png` both land,
        // and the session comes back clean so the pump can finish the
        // quit instead of re-arming the confirmation.
        let saved_img = crate::export::file::load_image(&dir.join("dup.png")).unwrap();
        assert_eq!(saved_img.get_pixel(4, 4).0, [255, 0, 0, 255]);
        assert!(
            dir.join("dup_2.png").is_file(),
            "twin suffixed, not blocked"
        );
        assert!(
            !lock(&shared.viewer).dirty,
            "clean session lets the quit proceed"
        );
        assert!(
            !lock(&shared.viewer).dup_names,
            "no rename modal on the quit path"
        );
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

    /// The `#` counter lives in each workspace: captures title from
    /// the ACTIVE workspace's mask, and switching back resumes the
    /// sequence where that workspace left it.
    #[test]
    fn workspace_counters_advance_independently() {
        let (shared, ctx) = harness();
        let bee = {
            let mut s = lock(&shared.settings);
            s.active_workspace_mut().name_mask = "A##".to_owned();
            let mut w = crate::config::workspace::Workspace::new("Bee".to_owned());
            w.name_mask = "B##".to_owned();
            let id = w.id.clone();
            s.workspaces.push(w);
            id
        };
        assert_eq!(shared.capture_title(), "A01");
        assert_eq!(shared.capture_title(), "A02");
        drive(&shared, &ctx, Command::SwitchWorkspace(bee));
        assert_eq!(shared.capture_title(), "B01");
        drive(
            &shared,
            &ctx,
            Command::SwitchWorkspace(crate::config::workspace::MAIN_ID.to_owned()),
        );
        assert_eq!(shared.capture_title(), "A03");
    }

    /// Switching to a workspace id that no longer exists lands on
    /// `main` — captures are never orphaned by a stale id.
    #[test]
    fn switch_to_unknown_workspace_lands_on_main() {
        let (shared, ctx) = harness();
        drive(&shared, &ctx, Command::SwitchWorkspace("gone".to_owned()));
        assert_eq!(
            lock(&shared.settings).active_workspace_id,
            crate::config::workspace::MAIN_ID
        );
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
}
