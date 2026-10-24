//! Save-as-file export with an `rfd` dialog.

use std::path::{Path, PathBuf};

use super::{ExportError, ImageFormat};
use crate::capture::Capture;
use crate::editor::Document;

/// Writes `capture`'s pixels to `path` in `format` — no dialog, the
/// caller picked the destination (the pin save window).
pub fn save_capture_to(
    capture: &Capture,
    path: &Path,
    format: ImageFormat,
) -> Result<(), ExportError> {
    let res = match format {
        // The JPEG encoder takes no alpha channel: flatten to RGB first.
        ImageFormat::Jpeg => image::DynamicImage::ImageRgba8(capture.image.clone())
            .into_rgb8()
            .save_with_format(path, format.into()),
        _ => capture.image.save_with_format(path, format.into()),
    };
    res.map_err(|e| ExportError::Failed(e.to_string()))
}

/// Asks for a destination and writes `capture`'s pixels (PNG or JPEG).
/// Kept for the plain OS-dialog path; the pin save window uses
/// [`save_capture_to`].
pub fn save_capture_with_dialog(
    capture: &Capture,
    format: ImageFormat,
) -> Result<PathBuf, ExportError> {
    let (ext, filter_name) = match format {
        ImageFormat::Png => ("png", "PNG"),
        ImageFormat::Jpeg => ("jpg", "JPEG"),
        ImageFormat::Webp => ("webp", "WebP"),
    };
    let Some(path) = rfd::FileDialog::new()
        .add_filter(filter_name, &[ext])
        .set_file_name(format!("capture.{ext}"))
        .save_file()
    else {
        return Err(ExportError::Cancelled);
    };
    save_capture_to(capture, &path, format)?;
    Ok(path)
}

/// Renders `doc` via [`crate::editor::render::render_to_image`], asks for a
/// destination and encodes it. Phase 4.
pub fn save_with_dialog(_doc: &Document, _format: ImageFormat) -> Result<PathBuf, ExportError> {
    Err(ExportError::Failed("not implemented".into()))
}

impl From<ImageFormat> for image::ImageFormat {
    fn from(f: ImageFormat) -> Self {
        match f {
            ImageFormat::Png => image::ImageFormat::Png,
            ImageFormat::Jpeg => image::ImageFormat::Jpeg,
            ImageFormat::Webp => image::ImageFormat::WebP,
        }
    }
}
