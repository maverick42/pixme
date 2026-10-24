//! Undo/redo: edits are reversible operations on the [`Document`].
//!
//! Every mutation goes through [`History::push`], which applies the edit,
//! records it for undo and clears the redo stack.

use super::Document;

/// A reversible document mutation.
pub trait Edit {
    /// Applies the edit.
    fn apply(&self, doc: &mut Document);
    /// Undoes exactly what [`Edit::apply`] did.
    fn revert(&self, doc: &mut Document);
    /// Short description for debugging (`tracing`), not user-facing.
    fn describe(&self) -> &'static str;
}

/// The undo/redo stacks of one document.
#[derive(Default)]
pub struct History {
    past: Vec<Box<dyn Edit>>,
    future: Vec<Box<dyn Edit>>,
}

impl History {
    /// Applies `edit` and records it for undo. (Phase 4: apply + push.)
    pub fn push(&mut self, _doc: &mut Document, _edit: Box<dyn Edit>) {}
    /// Reverts the most recent edit, if any. (Phase 4.)
    pub fn undo(&mut self, _doc: &mut Document) {}
    /// Re-applies the most recently undone edit, if any. (Phase 4.)
    pub fn redo(&mut self, _doc: &mut Document) {}
}
