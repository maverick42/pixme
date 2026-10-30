//! Quick export of the rendered document: file and clipboard.
//!
//! Clipboard via `arboard`, encoding via `image` (PNG, JPEG, WebP).

pub mod clipboard;
pub mod file;

/// Image formats PixMe can write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageFormat {
    /// Lossless PNG.
    Png,
    /// JPEG at 90% quality.
    Jpeg,
    /// Lossless WebP.
    Webp,
}

impl ImageFormat {
    /// The file extension (without dot).
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    /// The short label shown in save dialogs.
    pub fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPG",
            Self::Webp => "WEBP",
        }
    }
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
