//! Non-destructive annotation editor.
//!
//! Model: a [`Document`] owns the frozen [`capture::Capture`] plus an ordered
//! list of [`Annotation`]s. Edits are [`history::Edit`]s (undo/redo). Display
//! and export share the single [`render::paint_document`] /
//! [`render::render_to_image`] path so WYSIWYG holds.

pub mod document;
pub mod emoji;
pub mod history;
pub mod meta;
pub mod render;
pub mod tools;

// Re-exports are the public editor API; consumed from phase 4 on.
#[allow(unused_imports)]
pub use document::{Annotation, AnnotationId, AnnotationKind, Document};
#[allow(unused_imports)]
pub use meta::CaptureMeta;
#[allow(unused_imports)]
pub use tools::ToolId;
