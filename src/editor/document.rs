//! The editor document: a captured image plus non-destructive annotations.

use egui::{Color32, Pos2};

use crate::capture::{Capture, PhysicalRect};

/// Unique id of an annotation inside a [`Document`].
pub type AnnotationId = u64;

/// Geometry/content of one annotation. All coordinates are in the capture's
/// physical pixels (scale-factor conversion happens at the edge).
#[derive(Clone, Debug)]
pub enum AnnotationKind {
    /// Straight arrow between two points.
    Arrow { from: Pos2, to: Pos2 },
    /// Free text at a point.
    Text {
        pos: Pos2,
        content: String,
        size_pt: f32,
    },
    /// Stroked/filled rectangle.
    Rect { rect: PhysicalRect },
    /// Stroked/filled ellipse inside `rect`.
    Ellipse { rect: PhysicalRect },
    /// Numbered step marker (circle + counter).
    Step { pos: Pos2, number: u32 },
    /// Crop to `rect` (one crop per document).
    Crop { rect: PhysicalRect },
    /// Pixelated blur inside `rect`.
    Blur { rect: PhysicalRect, strength: u8 },
}

/// One annotation with its style.
#[derive(Clone, Debug)]
pub struct Annotation {
    /// Identity for undo/redo and hit-testing.
    pub id: AnnotationId,
    /// Shape and payload.
    pub kind: AnnotationKind,
    /// Stroke colour (theme-independent annotation palette).
    pub color: Color32,
    /// Stroke width, px.
    pub width: f32,
}

/// The open document: base capture + annotations in z-order.
#[derive(Clone, Debug)]
pub struct Document {
    /// The frozen screenshot.
    pub base: Capture,
    /// Annotations, bottom to top.
    pub annotations: Vec<Annotation>,
    /// Bumped on every edit; caches key off it.
    pub revision: u64,
    /// Next annotation id.
    next_id: AnnotationId,
}

impl Document {
    /// Wraps a capture in an empty document.
    pub fn new(base: Capture) -> Self {
        Self {
            base,
            annotations: Vec::new(),
            revision: 0,
            next_id: 0,
        }
    }

    /// Allocates a fresh annotation id.
    pub fn alloc_id(&mut self) -> AnnotationId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}
