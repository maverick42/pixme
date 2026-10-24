//! The single rendering path for the document: `paint_document` draws into
//! the UI, `render_to_image` produces the export bitmap. Both walk the same
//! annotation list so display and export never diverge.

use super::Document;

/// Paints `doc` (base image + annotations) inside `rect` in `ui`.
/// Phase 4: texture of the base + one painter pass per annotation.
pub fn paint_document(_ui: &mut egui::Ui, _doc: &Document, _rect: egui::Rect) {}

/// Rasterizes `doc` to RGBA for export (same walk as [`paint_document`]).
/// Phase 4.
pub fn render_to_image(_doc: &Document) -> image::RgbaImage {
    image::RgbaImage::new(0, 0)
}
