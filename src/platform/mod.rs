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
        "capture.active_window" => Command::Capture(CaptureMode::ActiveWindow),
        "capture.window" => Command::Capture(CaptureMode::WindowOrObject),
        "capture.region" => Command::Capture(CaptureMode::Region),
        "capture.fullscreen" => Command::Capture(CaptureMode::FullScreen),
        "capture.fixed_region" => Command::Capture(CaptureMode::FixedRegion),
        "capture.pin" => Command::Capture(CaptureMode::Pin),
        "settings" => Command::OpenSettings,
        "import_clipboard" => Command::ImportClipboard,
        "file.open" => Command::OpenFile,
        "file.save" => Command::SaveActive,
        "file.save_all" => Command::SaveAll,
        "file.rename" => Command::RenameActive,
        "quit" => Command::Quit,
        _ => return None,
    })
}
