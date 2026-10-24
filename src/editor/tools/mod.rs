//! Editor tools: which tool is active and how pointer events become edits.
//!
//! A tool turns drag/click gestures into [`super::history::Edit`]s pushed on
//! the document history. Tool behaviours land in phase 4.

/// The selectable tools of the annotation editor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolId {
    /// Pick/move/edit existing annotations.
    #[default]
    Select,
    /// Draw an arrow.
    Arrow,
    /// Place text.
    Text,
    /// Draw a rectangle.
    Rect,
    /// Draw an ellipse.
    Ellipse,
    /// Drop a numbered step marker.
    Step,
    /// Crop the image.
    Crop,
    /// Blur a region.
    Blur,
}

/// Pointer interaction between the editor surface and a tool.
pub trait ToolBehaviour {
    /// Which tool this is.
    fn id(&self) -> ToolId;
    /// Handles one pointer event inside the document surface.
    /// Returns `true` when the event changed tool state (repaint needed).
    fn pointer_event(&mut self, _doc: &mut super::Document, _pos: egui::Pos2) -> bool {
        false
    }
}
