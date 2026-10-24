//! Copy-to-clipboard export via `arboard`.

use super::ExportError;
use crate::capture::Capture;
use crate::editor::Document;

/// Puts `capture`'s raw RGBA pixels on the OS clipboard — used by pinned
/// windows and by the quick-copy option.
pub fn copy_capture(capture: &Capture) -> Result<(), ExportError> {
    let img = &capture.image;
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| ExportError::Failed(e.to_string()))?;
    clipboard
        .set_image(arboard::ImageData {
            width: img.width() as usize,
            height: img.height() as usize,
            bytes: std::borrow::Cow::Borrowed(img.as_raw().as_slice()),
        })
        .map_err(|e| ExportError::Failed(e.to_string()))
}

/// Renders `doc` and puts the RGBA pixels on the OS clipboard. Phase 4.
pub fn copy(_doc: &Document) -> Result<(), ExportError> {
    Err(ExportError::Failed("not implemented".into()))
}
