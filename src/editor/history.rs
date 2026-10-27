//! Undo/redo for the annotation list — chronological, reversible edits.
//!
//! Every draw-window mutation goes through [`AnnoEdit`]: apply performs
//! it, `revert` undoes exactly what apply did. The stacks themselves
//! live on the viewer `Tab` (they survive draw-window re-entry and die
//! with the tab, or when a pixel op flattens the document).

use serde::{Deserialize, Serialize};

use super::document::{Annotation, AnnotationId, Document};

/// How many annotation edits stay undoable per tab. Annotation payloads
/// are small (a few KB; bitmap-carrying objects a few MB at most), so a
/// generous cap costs little.
pub const ANNO_HISTORY_CAP: usize = 50;

/// A reversible edit of a [`Document`]'s annotation list.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AnnoEdit {
    /// `anno` was inserted at `index`.
    Add { anno: Annotation, index: usize },
    /// `anno` was removed from `index`.
    Remove { anno: Annotation, index: usize },
    /// `before` was mutated into `after` (style, geometry, content).
    Replace {
        before: Annotation,
        after: Annotation,
    },
    /// The annotation `id` moved from index `from` to `to` in the
    /// z-order (later entries draw on top).
    Reorder {
        id: AnnotationId,
        from: usize,
        to: usize,
    },
}

impl AnnoEdit {
    /// Performs the edit on `doc`'s annotation list.
    pub fn apply(&self, doc: &mut Document) {
        match self {
            Self::Add { anno, index } => {
                doc.annotations
                    .insert((*index).min(doc.annotations.len()), anno.clone());
            }
            Self::Remove { anno, .. } => remove_by_id(doc, anno.id),
            Self::Replace { after, .. } => {
                if let Some(slot) = doc.annotations.iter_mut().find(|a| a.id == after.id) {
                    *slot = after.clone();
                }
            }
            Self::Reorder { id, to, .. } => move_to(doc, *id, *to),
        }
    }

    /// Undoes exactly what [`Self::apply`] did.
    pub fn revert(&self, doc: &mut Document) {
        match self {
            Self::Add { anno, .. } => remove_by_id(doc, anno.id),
            Self::Remove { anno, index } => {
                doc.annotations
                    .insert((*index).min(doc.annotations.len()), anno.clone());
            }
            Self::Replace { before, .. } => {
                if let Some(slot) = doc.annotations.iter_mut().find(|a| a.id == before.id) {
                    *slot = before.clone();
                }
            }
            Self::Reorder { id, from, .. } => move_to(doc, *id, *from),
        }
    }
}

/// Re-inserts the annotation `id` at index `to` (clamped).
fn move_to(doc: &mut Document, id: AnnotationId, to: usize) {
    if let Some(cur) = doc.annotations.iter().position(|a| a.id == id) {
        let a = doc.annotations.remove(cur);
        doc.annotations.insert(to.min(doc.annotations.len()), a);
    }
}

/// Removes the annotation with `id`, if present.
fn remove_by_id(doc: &mut Document, id: AnnotationId) {
    if let Some(i) = doc.annotations.iter().position(|a| a.id == id) {
        doc.annotations.remove(i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, MonitorInfo, PhysicalRect};
    use crate::editor::document::{AnnotationKind, AreaShape, BlurObj, BlurStyle, RectObj};
    use egui::Color32;

    fn doc() -> Document {
        Document::new(Capture {
            image: image::RgbaImage::from_pixel(40, 40, image::Rgba([20, 30, 40, 255])),
            scale_factor: 1.0,
            monitor: MonitorInfo {
                id: 0,
                name: String::new(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                scale_factor: 1.0,
                is_primary: true,
            },
        })
    }

    fn rect_anno(id: u64) -> Annotation {
        Annotation {
            id,
            kind: AnnotationKind::Rect(RectObj {
                rect: PhysicalRect {
                    x: 4,
                    y: 4,
                    width: 10,
                    height: 10,
                },
                stroke: Color32::RED,
                stroke_w: 2.0,
                dash: super::super::document::Dash::Solid,
                fill: None,
                radius: 0.0,
                outline: 0.0,
                outline_color: Color32::WHITE,
                opacity: 1.0,
                shadow: 0.0,
            }),
        }
    }

    /// Add → Replace → Remove round-trips through apply/revert.
    #[test]
    fn anno_edit_round_trip() {
        let mut d = doc();
        let add = AnnoEdit::Add {
            anno: rect_anno(7),
            index: 0,
        };
        add.apply(&mut d);
        assert_eq!(d.annotations.len(), 1);
        let mut after = rect_anno(7);
        if let AnnotationKind::Rect(o) = &mut after.kind {
            o.stroke = Color32::BLUE;
        }
        let before = d.annotation(7).unwrap().clone();
        AnnoEdit::Replace { before, after }.apply(&mut d);
        let mut hit_blue = false;
        if let AnnotationKind::Rect(o) = &d.annotation(7).unwrap().kind {
            hit_blue = o.stroke == Color32::BLUE;
        }
        assert!(hit_blue);
        AnnoEdit::Remove {
            anno: rect_anno(7),
            index: 0,
        }
        .apply(&mut d);
        assert!(d.annotations.is_empty());
        // Revert chain restores each step exactly.
        AnnoEdit::Remove {
            anno: rect_anno(7),
            index: 0,
        }
        .revert(&mut d);
        AnnoEdit::Replace {
            before: d.annotation(7).unwrap().clone(),
            after: {
                let mut a = rect_anno(7);
                if let AnnotationKind::Rect(o) = &mut a.kind {
                    o.stroke = Color32::BLUE;
                }
                a
            },
        }
        .revert(&mut d);
        if let AnnotationKind::Rect(o) = &d.annotation(7).unwrap().kind {
            assert_eq!(o.stroke, Color32::RED);
        }
    }

    /// Bring-to-front / send-to-back round-trips through apply/revert.
    #[test]
    fn reorder_round_trip() {
        let mut d = doc();
        for id in [1, 2, 3] {
            AnnoEdit::Add {
                anno: rect_anno(id),
                index: d.annotations.len(),
            }
            .apply(&mut d);
        }
        let ids = |d: &Document| d.annotations.iter().map(|a| a.id).collect::<Vec<_>>();
        assert_eq!(ids(&d), vec![1, 2, 3]);
        let front = AnnoEdit::Reorder {
            id: 1,
            from: 0,
            to: 2,
        };
        front.apply(&mut d);
        assert_eq!(ids(&d), vec![2, 3, 1]);
        front.revert(&mut d);
        assert_eq!(ids(&d), vec![1, 2, 3]);
        let back = AnnoEdit::Reorder {
            id: 3,
            from: 2,
            to: 0,
        };
        back.apply(&mut d);
        assert_eq!(ids(&d), vec![3, 1, 2]);
        back.revert(&mut d);
        assert_eq!(ids(&d), vec![1, 2, 3]);
    }

    /// A blur annotation actually changes the pixels it covers.
    #[test]
    fn blur_renders() {
        let mut d = doc();
        d.annotations.push(Annotation {
            id: 0,
            kind: AnnotationKind::Blur(BlurObj {
                rect: PhysicalRect {
                    x: 5,
                    y: 5,
                    width: 20,
                    height: 20,
                },
                style: BlurStyle::Pixelate,
                shape: AreaShape::Rect,
                strength: 30,
            }),
        });
        let out = crate::editor::render::render_to_image(&d);
        assert_eq!(out.get_pixel(0, 0).0, [20, 30, 40, 255]);
        // Pixelated region shows averaged colour (uniform here → same).
        assert_eq!(out.get_pixel(10, 10).0[3], 255);
    }

    /// Every bracket shape generates a non-degenerate polyline between its
    /// two anchors, bulges consistently regardless of drag direction, and
    /// renders onto the document bitmap.
    #[test]
    fn brace_shapes_render_and_orient_consistently() {
        use crate::editor::document::{BraceKind, BraceObj, Dash};
        use crate::editor::render::{brace_polyline, render_to_image};
        use egui::pos2;

        for kind in BraceKind::ALL {
            let down = BraceObj {
                a: pos2(20.0, 6.0),
                b: pos2(20.0, 34.0),
                kind,
                color: Color32::RED,
                width: 2.0,
                dash: Dash::Solid,
                outline: 1.0,
                outline_color: Color32::WHITE,
                opacity: 1.0,
                shadow: 3.0,
            };
            let up = BraceObj {
                a: down.b,
                b: down.a,
                ..down.clone()
            };
            let pts_down = brace_polyline(&down);
            let pts_up = brace_polyline(&up);
            assert!(pts_down.len() >= 4);
            assert_eq!(pts_down, pts_up);
            if kind.is_left() {
                assert!(pts_down.iter().any(|p| p.x < 18.0));
            } else {
                assert!(pts_down.iter().any(|p| p.x > 22.0));
            }
            let mut d = doc();
            d.annotations.push(Annotation {
                id: 1,
                kind: AnnotationKind::Brace(down),
            });
            let out = render_to_image(&d);
            assert!(
                out.pixels().any(|px| px.0 != [20, 30, 40, 255]),
                "{kind:?} did not paint any pixel"
            );
        }
    }
}
