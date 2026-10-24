//! Quick export of the rendered document: file and clipboard.
//!
//! File dialogs via `rfd`, clipboard via `arboard`, encoding via `image`
//! (PNG and JPEG only).

pub mod clipboard;
pub mod file;

/// Image formats PixMe can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Lossless PNG.
    Png,
    /// JPEG at 90% quality.
    Jpeg,
}

/// Export failure (surfaced to the user via i18n at the UI layer).
#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    /// Encoding or writing failed.
    #[error("export failed: {0}")]
    Failed(String),
    /// The user cancelled the save dialog.
    #[error("cancelled")]
    Cancelled,
}
