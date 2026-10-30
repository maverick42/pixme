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
    /// The quit-time session save is writing on a worker — the UI is
    /// frozen behind the saving veil until the app actually closes.
    Saving,
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
        Command::OpenSettings => modal == ModalState::Settings,
        Command::OpenViewer => modal == ModalState::Settings,
        // Everything else is blocked while a modal owns the UI.
        Command::Capture(_)
        | Command::CaptureNow(_)
        | Command::ShowBar
        | Command::ImportClipboard
        | Command::OpenFile
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
    /// The session database — documents, annotations and undo stacks
    /// written at quit, read back on the first viewer open.
    pub store: Mutex<crate::store::Store>,
    /// The in-flight session restore: a worker thread owns the Store
    /// while loading (image decode off the UI thread) and hands it
    /// back with the session.
    pub restore_rx: Mutex<
        Option<std::sync::mpsc::Receiver<(crate::store::StoredSession, crate::store::Store)>>,
    >,
    /// The in-flight quit-time save: the worker owns the Store + the
    /// tabs while writing; its return closes the app.
    pub quit_rx: Mutex<Option<std::sync::mpsc::Receiver<crate::store::Store>>>,
    /// The in-flight "Tout enregistrer" batch: the worker owns the tabs
    /// while writing; `pump` reattaches them when it lands.
    save_all: Mutex<Option<SaveAllJob>>,
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
    last_modal: Mutex<ModalState>,
    /// An in-viewer dialog (rename, resize, protection, delete-file,
    /// unsaved confirm, dup names) is up — synced every viewer pass so
    /// `modal_state()` never locks the viewer while `viewer.show` holds
    /// it (and would self-deadlock).
    pub viewer_dialog_open: AtomicBool,
}

/// A running background save-all: the moved tabs come back over `rx`,
/// `done` ticks once per processed tab for the progress veil, and
/// `then_confirm` closes the saved session on completion (ConfirmSave).
struct SaveAllJob {
    rx: std::sync::mpsc::Receiver<Vec<crate::ui::viewer::Tab>>,
    done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    total: usize,
    then_confirm: bool,
}

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
        } else if self.settings_open.load(Ordering::Relaxed) {
            ModalState::Settings
        } else if self.viewer_dialog_open.load(Ordering::Relaxed) {
            ModalState::ViewerDialog
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
        // Menu enable state follows every modal transition — opened via
        // commands, closed via window chrome, Escape or programmatic
        // clears all funnel into `modal_state()` read here. `pump` only
        // runs on viewport passes, i.e. the main thread, which is where
        // the thread_local muda handles live.
        let modal = self.modal_state();
        let mut last_modal = lock(&self.last_modal);
        if *last_modal != modal {
            *last_modal = modal;
            platform::sync_menu_enabled(modal);
            ctx.request_repaint_of(egui::ViewportId::ROOT);
            ctx.request_repaint_of(viewer_id());
            ctx.request_repaint_of(bar_id());
        }
        // A background session restore lands here — swap the empty
        // session for the loaded tabs and drop the spinner.
        let done = lock(&self.restore_rx)
            .as_ref()
            .and_then(|r| r.try_recv().ok());
        if let Some((session, store)) = done {
            *lock(&self.restore_rx) = None;
            self.finish_restore(session, store);
            ctx.request_repaint_of(viewer_id());
        }
        // The quit-time save finished on its thread: the store comes
        // back and the app closes for real.
        let store_back = lock(&self.quit_rx).as_ref().and_then(|r| r.try_recv().ok());
        if let Some(store) = store_back {
            *lock(&self.quit_rx) = None;
            *lock(&self.store) = store;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
        // A background save-all finished: the tabs come back into the
        // viewer; a ConfirmSave batch then closes the (now clean)
        // session, or keeps it open when some tab is still unsaved.
        let save_done = lock(&self.save_all)
            .as_ref()
            .and_then(|j| j.rx.try_recv().ok());
        if let Some(tabs) = save_done {
            let job = lock(&self.save_all).take().expect("save-all job");
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
            ctx.request_repaint_of(viewer_id());
        }
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
                // Always open the floating bar top-centred on the monitor
                // where the mouse currently is.
                if let Some(m) = platform::cursor_position()
                    .and_then(|(x, y)| crate::capture::screen::monitor_at(x, y))
                    .or_else(|| monitor_under(ctx))
                    .or_else(|| crate::capture::screen::primary_monitor().ok())
                {
                    let scale = m.scale_factor.max(1.0);
                    let origin =
                        platform::native_to_ui(egui::pos2(m.rect.x as f32, m.rect.y as f32), scale);
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
                self.enter(AppState::Bar, ctx);
            }
            Command::OpenViewer => {
                self.enter(AppState::Viewer, ctx);
                // Re-invoking while the window is already open (behind
                // other apps, or minimized in the Dock) must still bring
                // it forward: `enter` only requests focus on a state
                // CHANGE, so de-minimize and focus explicitly here.
                ctx.send_viewport_cmd_to(viewer_id(), egui::ViewportCommand::Minimized(false));
                // The direct Focus command both wakes the (possibly
                // sleeping) viewport and, on macOS, activates the app
                // (`activateIgnoringOtherApps` + `makeKeyAndOrderFront`)
                // — a mere in-app focus request would do nothing while
                // another application is frontmost.
                ctx.send_viewport_cmd_to(viewer_id(), egui::ViewportCommand::Focus);
                self.request_focus(viewer_id());
                ctx.request_repaint_of(viewer_id());
            }
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
                if self.settings_open.load(Ordering::Relaxed) {
                    // Already open: re-invocation brings the existing
                    // modal to the front rather than re-opening it.
                    ctx.send_viewport_cmd_to(viewer_id(), egui::ViewportCommand::Minimized(false));
                    self.request_focus(viewer_id());
                    ctx.request_repaint_of(viewer_id());
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
                ctx.request_repaint_of(viewer_id());
            }
            Command::CloseSettings => {
                self.settings_open.store(false, Ordering::Relaxed);
                *lock(&self.settings_draft) = None;
                ctx.request_repaint_of(viewer_id());
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
                    let title = self.capture_title();
                    lock(&self.viewer)
                        .push_capture_document(Document::new(synthetic_capture(image)), title);
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
            viewer.dup_names = true;
            ctx.request_repaint_of(viewer_id());
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
                        tracing::info!(?path, "tab saved");
                    }
                    Err(e) => tracing::warn!(?path, "save-all failed: {e}"),
                }
                done.fetch_add(1, Ordering::Relaxed);
                wake.request_repaint();
            }
            let _ = tx.send(tabs);
            wake.request_repaint();
        });
        ctx.request_repaint_of(viewer_id());
        false
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
                session.monitor = Some(monitor);
                session.include_pointer = lock(&self.settings).include_pointer;
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
            self.enter(return_to, ctx);
            return;
        };
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

    /// First entry into the viewer: the persisted session loads on a
    /// worker thread so the window paints instantly — image decode is
    /// what takes time, not the queries. The result lands in
    /// `restore_rx`, drained by `pump()`; the viewer paints a spinner
    /// meanwhile. Silent and best-effort (a corrupt database was
    /// already parked as `.bak` when the store opened).
    fn restore_session(&self, ctx: &egui::Context) {
        let needed = {
            let viewer = lock(&self.viewer);
            !viewer.session_restored() && viewer.tabs.is_empty()
        };
        if !needed {
            return;
        }
        // Claim the restore first — a second viewer entry while the
        // thread runs must not spawn a duplicate loader.
        lock(&self.viewer).start_restore();
        // Retention `Never` writes nothing: skip the thread entirely.
        if !self.session_persists() {
            let sort = lock(&self.settings).tab_sort;
            lock(&self.viewer).restore_session(crate::store::StoredSession::default(), sort);
            return;
        }
        let (tx, rx) = std::sync::mpsc::channel();
        *lock(&self.restore_rx) = Some(rx);
        // The Store itself rides the thread — one connection, no dual
        // access, and tests keep their in-memory database. A disabled
        // placeholder sits in `shared.store` meanwhile.
        let mut store = std::mem::replace(&mut *lock(&self.store), crate::store::Store::disabled());
        let wake = ctx.clone();
        std::thread::spawn(move || {
            let session = store.load_session();
            let _ = tx.send((session, store));
            wake.request_repaint();
        });
    }

    /// Installs the result of the loader thread: the store comes back,
    /// the loaded session swaps into the viewer.
    fn finish_restore(&self, session: crate::store::StoredSession, store: crate::store::Store) {
        *lock(&self.store) = store;
        let sort = lock(&self.settings).tab_sort;
        lock(&self.viewer).restore_session(session, sort);
    }

    /// `true` when the session is persisted to `session.db` — the
    /// dirty-session warning only makes sense when retention is `Never`
    /// (nothing survives the exit otherwise, so we ask first).
    fn session_persists(&self) -> bool {
        lock(&self.settings).db_retention != crate::config::DbRetention::Never
    }

    /// Persists the session (best-effort — a failure never blocks
    /// quitting), then closes the anchor viewport: eframe exits once
    /// the root window is gone.
    ///
    /// The content signature decides the cost: an unchanged session
    /// writes nothing; a meta-only drift (active tab, titles, order)
    /// costs one row of UPDATEs; a real change rides a worker thread
    /// while the viewer shows the saving veil — `pump()` closes the
    /// app when the store comes back.
    fn quit_now(&self, ctx: &egui::Context) {
        if !self.session_persists() {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
        // A restore still decoding owns the real Store — wait for it and
        // install the session first, or quit would save into the disabled
        // placeholder and the whole session would be silently lost.
        if let Some(rx) = lock(&self.restore_rx).take()
            && let Ok((session, store)) = rx.recv()
        {
            self.finish_restore(session, store);
        }
        // An open Draw editor still owns its tab's document clone —
        // finalize any open text panel and commit it back so a quit
        // mid-edit loses nothing.
        if let Some(mut draw) = lock(&self.draw).take() {
            draw.finalize_pending();
            lock(&self.viewer).commit_draw(draw.tab, draw.doc, draw.undo, draw.redo);
        }
        let mut viewer = lock(&self.viewer);
        let sigs = viewer.sigs();
        if viewer.persisted_sigs == Some(sigs) {
            drop(viewer);
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
        let active = viewer.active_index().unwrap_or(0);
        let next_title = viewer.next_title();
        if viewer.persisted_sigs.is_some_and(|(c, _)| c == sigs.0) {
            // Content identical — only meta drifted: cheap UPDATEs,
            // no image work, quit stays instant.
            let rows: Vec<_> = viewer
                .tabs
                .iter()
                .map(|t| (t.doc.meta.id.clone(), t.title.clone(), t.saved))
                .collect();
            drop(viewer);
            lock(&self.store).save_meta(&rows, active, next_title);
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
            return;
        }
        // Real changes — tabs and store both ride the worker; the
        // viewer's saving veil covers the wait and pump() closes the
        // app when the store returns.
        let tabs = std::mem::take(&mut viewer.tabs);
        drop(viewer);
        let mut store = std::mem::replace(&mut *lock(&self.store), crate::store::Store::disabled());
        let (tx, rx) = std::sync::mpsc::channel();
        *lock(&self.quit_rx) = Some(rx);
        let wake = ctx.clone();
        std::thread::spawn(move || {
            store.save_session(&tabs, active, next_title);
            let _ = tx.send(store);
            wake.request_repaint();
        });
        ctx.request_repaint_of(viewer_id());
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
                last_modal: Mutex::new(ModalState::None),
                viewer_dialog_open: AtomicBool::new(false),
                settings: Mutex::new(settings),
                viewer: Mutex::new(Viewer::default()),
                viewer_rect: Mutex::new(None),
                viewer_target_monitor: Mutex::new(None),
                toolbar: Mutex::new(Toolbar::default()),
                bar_pos: Mutex::new(None),
                bar_rect: Mutex::new(None),
                settings_open: AtomicBool::new(false),
                settings_draft: Mutex::new(None),
                capture: Mutex::new(None),
                countdown: Mutex::new(None),
                last_fixed_region: Mutex::new(None),
                capture_start_monitor: Mutex::new(None),
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
                restore_rx: Mutex::new(None),
                quit_rx: Mutex::new(None),
                save_all: Mutex::new(None),
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
                // If a capture just landed on a specific monitor, make sure
                // the viewer opens on that monitor (preserving its relative
                // placement / size when already known, or centring it there).
                let target_mon = lock(&shared.viewer_target_monitor).take();
                if let Some(ref m) = target_mon {
                    let next = place_viewer_on_monitor(*lock(&shared.viewer_rect), m);
                    *lock(&shared.viewer_rect) = Some(next);
                }
                let builder = viewer_builder(lock(&shared.viewer_rect).map(|r| r.min));
                ctx.show_viewport_deferred(viewer_id(), builder, move |ui, _class| {
                    let ctx = ui.ctx().clone();
                    shared.pump(&ctx);
                    shared.maybe_focus(&ctx, viewer_id());
                    if let Some(ref m) = target_mon {
                        let next = place_viewer_on_monitor(*lock(&shared.viewer_rect), m);
                        *lock(&shared.viewer_rect) = Some(next);
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(next.min));
                    } else if let Some(outer) = ctx.input(|i| i.viewport().outer_rect) {
                        *lock(&shared.viewer_rect) = Some(outer);
                    }
                    // Draw is modal to the viewer: deferred viewports are
                    // plain top-level windows with no OS child-window
                    // modality. Clicking the modal's parent (the dim veil
                    // eats it) brings the editor back in front — one
                    // command per click, no per-frame focus fight.
                    if lock(&shared.draw).is_some() && ctx.input(|i| i.pointer.any_pressed()) {
                        ctx.send_viewport_cmd_to(draw_id(), egui::ViewportCommand::Focus);
                    }
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
                // Anchor the card under the floating bar when one is
                // visible; otherwise it falls back to the cursor monitor.
                let bar = (*lock(&shared.state) == AppState::Bar)
                    .then(|| *lock(&shared.bar_rect))
                    .flatten();
                let running = lock(&shared.countdown)
                    .as_mut()
                    .is_some_and(|c| c.show(ui, bar));
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
                    // The modal closed: the viewer gets focus back.
                    ctx.send_viewport_cmd_to(viewer_id(), egui::ViewportCommand::Focus);
                    if outcome == crate::ui::draw::Outcome::Ok {
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

/// Computes the viewer's outer rectangle on `monitor` (in egui ui points):
/// if `current` is already on `monitor` (its centre falls inside `monitor`'s
/// bounds), it is kept as-is; otherwise the window is centred on `monitor`
/// (clamped inside it).
fn place_viewer_on_monitor(current: Option<egui::Rect>, monitor: &MonitorInfo) -> egui::Rect {
    let scale = monitor.scale_factor.max(1.0);
    let mon_min = platform::native_to_ui(
        egui::pos2(monitor.rect.x as f32, monitor.rect.y as f32),
        scale,
    );
    let mon_size = platform::native_to_ui(
        egui::pos2(monitor.rect.width as f32, monitor.rect.height as f32),
        scale,
    )
    .to_vec2();
    let mon_rect = egui::Rect::from_min_size(mon_min, mon_size);
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
        .with_icon(ui::icons::app_icon_data())
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
        .with_icon(ui::icons::app_icon_data())
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

/// The capture-delay countdown: borderless, transparent, always on top,
/// sized to its painted card. The position only applies at creation —
/// the card then tracks the monitor under the cursor.
fn countdown_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("PixMe")
        .with_icon(ui::icons::app_icon_data())
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_always_on_top()
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(true)
        .with_inner_size([140.0, 140.0])
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

/// Writes panics to `data_dir()/panic.log` before the abort — a
/// `panic = "abort"` release binary otherwise dies with no trace of
/// the panic site. Installed once at startup.
fn install_panic_hook() {
    let log =
        directories::ProjectDirs::from("", "", "pixme").map(|d| d.data_dir().join("panic.log"));
    std::panic::set_hook(Box::new(move |info| {
        let line = format!(
            "[{}] panic on '{}': {info}",
            chrono::Local::now().to_rfc3339(),
            std::thread::current().name().unwrap_or("worker"),
        );
        tracing::error!("{line}");
        // stderr write errors must NOT re-panic inside the hook.
        use std::io::Write as _;
        let _ = std::io::stderr().write_all(line.as_bytes());
        let _ = std::io::stderr().write_all(b"\n");
        if let Some(path) = &log {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
            {
                let _ = f.write_all(line.as_bytes());
                let _ = f.write_all(b"\n");
            }
        }
    }));
}

/// Runs the app. The root viewport is the permanent 1×1 transparent anchor
/// window (never in the taskbar); the app first lives in the menu bar /
/// tray (`Idle`) until the user asks for a capture.
pub fn run() -> eframe::Result<()> {
    install_panic_hook();
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
            viewer_rect: Mutex::new(None),
            viewer_target_monitor: Mutex::new(None),
            toolbar: Mutex::new(Toolbar::default()),
            bar_pos: Mutex::new(None),
            bar_rect: Mutex::new(None),
            settings_open: AtomicBool::new(false),
            settings_draft: Mutex::new(None),
            capture: Mutex::new(None),
            countdown: Mutex::new(None),
            last_fixed_region: Mutex::new(None),
            capture_start_monitor: Mutex::new(None),
            pins: Mutex::new(Vec::new()),
            next_pin_id: Mutex::new(0),
            save_dialog: Mutex::new(None),
            draw: Mutex::new(None),
            store: Mutex::new(crate::store::Store::open_in_memory()),
            restore_rx: Mutex::new(None),
            quit_rx: Mutex::new(None),
            save_all: Mutex::new(None),
            pending_focus: Mutex::new(Vec::new()),
            pending: Mutex::new(Vec::new()),
            last_language: Mutex::new(Language::Auto),
            last_modal: Mutex::new(ModalState::None),
            viewer_dialog_open: AtomicBool::new(false),
        };
        (shared, egui::Context::default())
    }

    fn drive(shared: &Shared, ctx: &egui::Context, command: Command) -> AppState {
        shared.push(command);
        shared.pump(ctx);
        *lock(&shared.state)
    }

    /// Blocks on the loader thread and installs its result — the
    /// synchronous twin of the drain `pump()` does each frame.
    fn drain_restore(shared: &Shared) {
        let rx = lock(&shared.restore_rx).take();
        if let Some(rx) = rx
            && let Ok((session, store)) = rx.recv()
        {
            shared.finish_restore(session, store);
        }
    }

    /// Blocks on the quit-time save thread and puts the store back —
    /// the synchronous twin of the drain `pump()` does each frame.
    fn drain_quit(shared: &Shared) {
        let rx = lock(&shared.quit_rx).take();
        if let Some(rx) = rx
            && let Ok(store) = rx.recv()
        {
            *lock(&shared.store) = store;
        }
    }

    /// Blocks on the save-all worker and reattaches the tabs — the
    /// synchronous twin of the `save_all` drain `pump()` does per frame.
    fn drain_save_all(shared: &Shared) {
        let Some(job) = lock(&shared.save_all).take() else {
            return;
        };
        if let Ok(tabs) = job.rx.recv() {
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

    /// A quit that changed nothing writes nothing — the signature check
    /// short-circuits before any store work.
    #[test]
    fn unchanged_quit_writes_nothing() {
        let (shared, ctx) = harness();
        // Restore runs first so the persisted baseline exists.
        drive(&shared, &ctx, Command::OpenViewer);
        drain_restore(&shared);
        drive(&shared, &ctx, Command::Quit);
        assert!(
            lock(&shared.quit_rx).is_none(),
            "unchanged quit must not write"
        );
    }

    /// A quit landing while the restore worker still owns the real
    /// Store must drain it first — otherwise the session is written
    /// into the disabled placeholder and silently lost.
    #[test]
    fn quit_drains_an_in_flight_restore() {
        let (shared, ctx) = harness();
        // Simulate a loader that finished decoding but whose result
        // pump() has not drained yet.
        let mut store = crate::store::Store::open_in_memory();
        let mut v = Viewer::default();
        v.push_stub("from_db");
        store.save_session(&v.tabs, 0, v.next_title());
        let session = store.load_session();
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send((session, store)).unwrap();
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
        assert_eq!(dialog.existing_ok, Some(path));
    }
}
