//! Save-as-file export with an `rfd` dialog. Phase 4.

use std::path::PathBuf;

use super::{ExportError, ImageFormat};
use crate::editor::Document;

/// Renders `doc` via [`crate::editor::render::render_to_image`], asks for a
/// destination and encodes it. Phase 4.
pub fn save_with_dialog(_doc: &Document, _format: ImageFormat) -> Result<PathBuf, ExportError> {
    Err(ExportError::Failed("not implemented".into()))
}
