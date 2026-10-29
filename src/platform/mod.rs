//! OS-specific services: one implementation per OS, selected at compile time.
//! All `#[cfg(target_os)]` code lives here — nowhere else in the crate.
//!
//! Both OSes present a resident icon whose menu carries the same two actions
//! ("Take a capture", "Open PixMe"): the macOS menu-bar status item and the
//! Windows notification-area tray icon. Menu clicks on either OS are pushed
//! into [`PENDING`] and drained by [`crate::app::Shared::pump`] on the next UI
//! pass. macOS additionally installs the native application menus.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

use std::sync::{Mutex, MutexGuard};

use crate::app::{CaptureMode, Command};
use eframe::egui;

/// What the OS needs PixMe to handle.
pub trait PlatformServices: Send + Sync {
    /// Ensures the OS permission required for screen capture is granted
    /// (macOS Screen Recording; no-op on Windows). `Err` carries a
    /// remediation hint key for i18n.
    fn ensure_capture_permission(&self) -> Result<(), crate::capture::CaptureError>;
    /// Requests the OS to open its settings page for the missing permission,
    /// when the OS supports that (macOS does).
    fn open_permission_settings(&self) {}

    /// Whether the OS provides a native application menu bar (macOS: yes —
    /// menus live at the top of the screen; Windows: no — the viewer renders
    /// its own egui menu strip inside the window instead).
    fn has_native_app_menu(&self) -> bool;
}

/// The implementation for the running OS.
#[cfg(target_os = "macos")]
pub fn current() -> &'static dyn PlatformServices {
    &macos::MacOs
}

/// The implementation for the running OS.
#[cfg(target_os = "windows")]
pub fn current() -> &'static dyn PlatformServices {
    &windows::Windows
}

/// Installs the OS-level UI (status/tray icon, menus, event handler).
/// Owns its handles internally (main thread only, for the app lifetime).
#[cfg(target_os = "macos")]
pub use macos::install_ui;
/// Installs the OS-level UI (see above).
#[cfg(target_os = "windows")]
pub use windows::install_ui;

/// Rewrites every OS menu label in the current language.
#[cfg(target_os = "macos")]
pub use macos::relabel_menus;
/// Rewrites every OS menu label (see above).
#[cfg(target_os = "windows")]
pub use windows::relabel_menus;

/// OS tweaks to `eframe::NativeOptions` before the event loop starts
/// (macOS: accessory activation policy so no Dock icon appears).
#[cfg(target_os = "macos")]
pub use macos::apply_native_options;
/// OS tweaks to `eframe::NativeOptions` (Windows: none).
#[cfg(target_os = "windows")]
pub use windows::apply_native_options;

/// One on-screen window as reported by the OS window list — the fields
/// the pick overlay needs, gathered in a single enumeration pass.
/// `xcap::Window` accessor methods each re-enumerate the whole list on
/// macOS, so asking it for ~10 fields × ~50 windows stalls the UI for
/// seconds; this record comes from one `CGWindowListCopyWindowInfo` call.
pub struct WindowRecord {
    /// OS window id (`kCGWindowNumber` on macOS, `HWND` on Windows).
    pub id: u32,
    /// Owning process id.
    pub pid: u32,
    /// Owning application name.
    pub app_name: String,
    /// Window title.
    pub title: String,
    /// Global logical bounds (same space as `MonitorInfo::rect`).
    pub rect: crate::capture::PhysicalRect,
    /// Window-server layer: 0 = normal, 3 = floating palette, 8 = modal;
    /// Dock/menubar/status items sit at 20+. Always 0 on Windows.
    pub layer: i32,
}

/// All on-screen windows, front-to-back, in one enumeration pass.
#[cfg(target_os = "macos")]
pub use macos::window_list;
/// All on-screen windows (see above).
#[cfg(target_os = "windows")]
pub use windows::window_list;

/// Lock helper: recover a poisoned mutex rather than crash a menu callback.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Commands queued by OS menu clicks (status item / tray / app menus).
static PENDING: Mutex<Vec<Command>> = Mutex::new(Vec::new());

/// Called by OS menu-event handlers on the main thread.
pub(crate) fn queue_menu_command(command: Command) {
    lock(&PENDING).push(command);
}

/// Commands queued since the last call, drained by `Shared::pump`.
pub(crate) fn take_pending_commands() -> Vec<Command> {
    std::mem::take(&mut *lock(&PENDING))
}

/// Maps a menu-item id to an app [`Command`] — shared by the macOS status
/// item + app menus and the Windows tray menu.
pub(crate) fn command_from_menu_id(id: &str) -> Option<Command> {
    Some(match id {
        "take_capture" => Command::ShowBar,
        "open_viewer" => Command::OpenViewer,
        "capture.window" => Command::Capture(CaptureMode::WindowOrObject),
        "capture.region" => Command::Capture(CaptureMode::Region),
        "capture.fullscreen" => Command::Capture(CaptureMode::FullScreen),
        "capture.fixed_region" => Command::Capture(CaptureMode::FixedRegion),
        "capture.pin" => Command::Capture(CaptureMode::Pin),
        "settings" => Command::OpenSettings,
        "import_clipboard" => Command::ImportClipboard,
        "edit.undo" => Command::UndoActive,
        "edit.redo" => Command::RedoActive,
        "edit.copy" => Command::CopyActive,
        "file.open" => Command::OpenFile,
        "file.save" => Command::SaveActive,
        "file.save_all" => Command::SaveAll,
        "file.rename" => Command::RenameActive,
        "file.print" => Command::PrintActive,
        "quit" => Command::Quit,
        _ => return None,
    })
}

/// Greys out menu items whose [`Command`]'s
/// [`crate::app::is_command_enabled`] ruling forbids them under `modal`.
/// Called from [`crate::app::Shared::pump`], i.e. always on the main
/// thread, where the thread_local muda handles live.
#[cfg(target_os = "macos")]
pub use macos::sync_menu_enabled;
/// Greys out menu items (see above).
#[cfg(target_os = "windows")]
pub use windows::sync_menu_enabled;

/// Reveals a file selected in the OS file manager after saving.
#[cfg(target_os = "macos")]
pub use macos::reveal_in_file_manager;
/// Reveals a file selected in the OS file manager (see above).
#[cfg(target_os = "windows")]
pub use windows::reveal_in_file_manager;

/// egui window "ui points" (`ViewportInfo::outer_rect`) → OS-native global
/// units, the space `MonitorInfo::rect` lives in (CGDisplay bounds on
/// macOS, virtual-screen pixels on Windows).
///
/// macOS: winit derives window positions from logical points
/// (`LogicalPosition::to_physical`), so egui's division by
/// `native_pixels_per_point` lands back on native points — identity.
/// Windows: winit reports true physical pixels, so egui points must be
/// scaled back up to match xcap's physical-pixel rects.
pub fn ui_to_native(pos: egui::Pos2, native_pixels_per_point: f32) -> egui::Pos2 {
    #[cfg(target_os = "windows")]
    {
        pos * native_pixels_per_point
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = native_pixels_per_point;
        pos
    }
}

/// OS-native global units (`MonitorInfo::rect` space) → egui window
/// "ui points". The inverse of [`ui_to_native`].
pub fn native_to_ui(pos: egui::Pos2, native_pixels_per_point: f32) -> egui::Pos2 {
    #[cfg(target_os = "windows")]
    {
        pos / native_pixels_per_point
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = native_pixels_per_point;
        pos
    }
}

/// Sends a file to the OS default printer.
#[cfg(target_os = "macos")]
pub use macos::print_file;
/// Sends a file to the OS default printer (see above).
#[cfg(target_os = "windows")]
pub use windows::print_file;

/// Global mouse position in the OS-native display space (the same space
/// `MonitorInfo::rect` lives in).
#[cfg(target_os = "macos")]
pub use macos::cursor_position;
/// Global mouse position in the OS-native display space (see above).
#[cfg(target_os = "windows")]
pub use windows::cursor_position;

/// The short tick sounded at each countdown second (and when a delayed
/// capture is armed) — the OS alert sound.
#[cfg(target_os = "macos")]
pub use macos::tick;
/// The short tick (see above).
#[cfg(target_os = "windows")]
pub use windows::tick;
