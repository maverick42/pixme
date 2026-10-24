//! OS-specific services: one implementation per OS, selected at compile time.
//! All `#[cfg(target_os)]` code lives here — nowhere else in the crate.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

/// What the OS needs PixMe to handle.
pub trait PlatformServices: Send + Sync {
    /// Ensures the OS permission required for screen capture is granted
    /// (macOS Screen Recording; no-op on Windows). `Err` carries a
    /// remediation hint key for i18n.
    fn ensure_capture_permission(&self) -> Result<(), crate::capture::CaptureError>;
    /// Requests the OS to open its settings page for the missing permission,
    /// when the OS supports that (macOS does).
    fn open_permission_settings(&self) {}
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
