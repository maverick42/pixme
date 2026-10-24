//! Copy-to-clipboard export via `arboard`. Phase 4.

use super::ExportError;
use crate::editor::Document;

/// Renders `doc` and puts the RGBA pixels on the OS clipboard. Phase 4.
pub fn copy(_doc: &Document) -> Result<(), ExportError> {
    Err(ExportError::Failed("not implemented".into()))
}
