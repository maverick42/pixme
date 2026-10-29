//! The "Dessiner" annotation window: a separate deferred viewport with
//! a two-column tool strip (PhotoCraft styling), the composited
//! document canvas, a FastStone-compatible contextual options bar and
//! OK/Annuler. The session edits a clone of the viewer document — OK
//! writes it back (annotations stay re-editable on re-entry), Annuler
//! drops it. Chronological undo/redo uses `AnnoEdit` stacks stored on
//! the viewer `Tab`, so they survive editor close/reopen and die with
//! the tab or on flattening pixel ops.

use std::collections::{HashMap, VecDeque};

use egui::{
    Color32, ColorImage, CornerRadius, Key, Pos2, Rect, Response, ScrollArea, Sense, Stroke,
    StrokeKind, TextureHandle, TextureId, TextureOptions, Ui, Vec2, pos2, vec2,
};
use rust_i18n::t;

use crate::app::Shared;
use crate::capture::PhysicalRect;
use crate::editor::document::{
    Annotation, AnnotationId, AnnotationKind, AreaObj, AreaShape, BlurObj, BlurStyle, BoxObj,
    BraceKind, BraceObj, Dash, Document, EllipseObj, EmojiObj, EraserMode, FreeObj, LineEnd,
    LineObj, PicObj, RectObj, StepMode, StepObj, StepSize, StepStyle, TextAlign, TextObj, TipShape,
};
use crate::editor::history::{ANNO_HISTORY_CAP, AnnoEdit};
use crate::editor::render;
use crate::ui::icons::{self, Icon};
use crate::ui::{theme, widgets};

/// One drawable tool of the strip (two-column grid).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tool {
    Select,
    /// The marquee: frames a region for cut/copy/paste/delete/blur/fill.
    SelectArea,
    TextBoxRect,
    TextBoxOval,
    Text,
    Step,
    Arrow,
    Pencil,
    Brace,
    Eraser,
    Rect,
    RectFill,
    Ellipse,
    EllipseFill,
    HighlightArea,
    Blur,
    /// Colour eyedropper: the loupe follows the cursor, a click picks
    /// the pixel's colour, `C` copies its `#RRGGBB`.
    Pipette,
    /// Emoji sticker: click places it at the remembered/default size,
    /// dragging frames a free square. Sticky — Esc or another tool
    /// leaves it.
    Emoji,
}

impl Tool {
    /// The strip grid in display order (row-major, two columns).
    const ALL: [Tool; 18] = [
        Self::Select,
        Self::SelectArea,
        Self::TextBoxRect,
        Self::TextBoxOval,
        Self::Text,
        Self::Step,
        Self::Arrow,
        Self::Pencil,
        Self::Brace,
        Self::Eraser,
        Self::Rect,
        Self::RectFill,
        Self::Ellipse,
        Self::EllipseFill,
        Self::HighlightArea,
        Self::Blur,
        Self::Pipette,
        Self::Emoji,
    ];

    fn icon(self) -> Icon {
        match self {
            Self::Select => Icon::MousePointer,
            Self::SelectArea => Icon::SquareDashed,
            Self::TextBoxRect => Icon::MessageSquare,
            Self::TextBoxOval => Icon::MessageCircle,
            Self::Text => Icon::Type,
            Self::Step => Icon::Info,
            Self::Arrow => Icon::ArrowUpRight,
            Self::Pencil => Icon::Pencil,
            Self::Brace => Icon::Braces,
            Self::Eraser => Icon::Eraser,
            Self::Rect => Icon::Square,
            Self::RectFill => Icon::SquareFill,
            Self::Ellipse => Icon::Circle,
            Self::EllipseFill => Icon::CircleFill,
            Self::HighlightArea => Icon::Highlighter,
            Self::Blur => Icon::Pixelate,
            Self::Pipette => Icon::Pipette,
            Self::Emoji => Icon::Smile,
        }
    }

    fn tip(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Select => t!("draw.tool.select"),
            Self::SelectArea => t!("draw.tool.selarea"),
            Self::TextBoxRect => t!("draw.tool.textbox_rect"),
            Self::TextBoxOval => t!("draw.tool.textbox_oval"),
            Self::Text => t!("draw.tool.text"),
            Self::Step => t!("draw.tool.step"),
            Self::Arrow => t!("draw.tool.arrow"),
            Self::Pencil => t!("draw.tool.pencil"),
            Self::Brace => t!("draw.tool.brace"),
            Self::Eraser => t!("draw.tool.eraser"),
            Self::Rect => t!("draw.tool.rect"),
            Self::RectFill => t!("draw.tool.rectfill"),
            Self::Ellipse => t!("draw.tool.ellipse"),
            Self::EllipseFill => t!("draw.tool.ellipsefill"),
            Self::HighlightArea => t!("draw.tool.harea"),
            Self::Blur => t!("draw.tool.blur"),
            Self::Pipette => t!("draw.tool.pipette"),
            Self::Emoji => t!("draw.tool.emoji"),
        }
    }
}

/// All tool options, held for the session's lifetime so each tool
/// remembers its settings between uses (like the reference's bars).
#[derive(Clone, Debug)]
struct Opts {
    // Shared style state.
    color: Color32,
    fill_color: Color32,
    width: f32,
    dash: Dash,
    shadow: f32,
    opacity: f32,
    // Line ends (Fin = start, Début = end in the reference UI).
    line_start: LineEnd,
    line_end: LineEnd,
    outline: f32,
    line_outline_color: Color32,
    line_shadow: bool,
    // Shape specifics.
    radius: f32,
    tip: TipShape,
    brace_kind: BraceKind,
    shape: AreaShape,
    /// Line / area highlighter coverage (defaults translucent).
    hl_opacity: f32,
    // Eraser tool.
    eraser_color: Color32,
    eraser_size: f32,
    eraser_mode: EraserMode,
    eraser_pick: bool,
    // Blur tool.
    blur_style: BlurStyle,
    blur_strength: u8,
    // Text (seeded from persisted prefs).
    font: String,
    font_size: f32,
    bold: bool,
    italic: bool,
    outline_w: f32,
    outline_color: Color32,
    align: TextAlign,
    // Step markers.
    step_mode: StepMode,
    step_style: StepStyle,
    step_size: StepSize,
    /// Next number counter ("Étape 1.2.3…"); auto-increments on click.
    step_next_num: u32,
    /// Next letter counter ("Étape A.B.C…"); auto-increments on click.
    step_next_let: u32,
    step_bg: Color32,
    step_fg: Color32,
    /// Outline thickness 0..=10 (5 by default).
    step_ring: f32,
    /// Outline colour.
    step_ring_color: Color32,
    step_shadow: bool,
    step_lower: bool,
    // Emoji tool.
    /// Emoji id (Unicode code point) the tool places.
    emoji_id: u32,
    /// Last side the user dragged an emoji to, image px — reused by the
    /// next click placement (session memory, not persisted).
    emoji_size: Option<f32>,
}

impl Opts {
    /// Seeds the text defaults from `prefs.json` so the user's last
    /// text styling survives across sessions.
    fn from_settings(p: &crate::config::Settings) -> Self {
        let [r, g, b] = p.text_color;
        let [or, og, ob] = p.text_outline_color;
        Self {
            color: Color32::from_rgb(r, g, b),
            font: if p.text_font.is_empty() {
                "Arial".to_owned()
            } else {
                p.text_font.clone()
            },
            font_size: (p.text_size.clamp(8, 72)) as f32,
            bold: p.text_bold,
            italic: p.text_italic,
            outline_w: (p.text_outline.min(10)) as f32,
            outline_color: Color32::from_rgb(or, og, ob),
            ..Self::default()
        }
    }
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            color: Color32::from_rgb(225, 40, 40),
            fill_color: Color32::from_rgb(80, 160, 235),
            width: 3.0,
            dash: Dash::Solid,
            shadow: 0.0,
            opacity: 1.0,
            line_start: LineEnd::None,
            line_end: LineEnd::Arrow,
            outline: 2.0,
            line_outline_color: Color32::WHITE,
            line_shadow: true,
            radius: 8.0,
            tip: TipShape::Round,
            brace_kind: BraceKind::CurlyLeft,
            shape: AreaShape::Rect,
            hl_opacity: 0.45,
            eraser_color: Color32::WHITE,
            eraser_size: 12.0,
            eraser_mode: EraserMode::Color,
            eraser_pick: false,
            blur_style: BlurStyle::Blur,
            blur_strength: 75,
            font: "Arial".to_owned(),
            font_size: 12.0,
            bold: false,
            italic: false,
            outline_w: 0.0,
            outline_color: Color32::WHITE,
            align: TextAlign::Left,
            step_mode: StepMode::Numbers,
            step_style: StepStyle::Circle,
            step_size: StepSize::Medium,
            step_next_num: 1,
            step_next_let: 1,
            step_bg: Color32::from_rgb(225, 40, 40),
            step_fg: Color32::WHITE,
            step_ring: 5.0,
            step_ring_color: Color32::WHITE,
            step_shadow: true,
            step_lower: false,
            emoji_id: 0x1F449, // 👉 — a good annotation default
            emoji_size: None,
        }
    }
}

/// What the pointer is doing on the canvas.
#[derive(Clone, Debug)]
/// An arrow-nudge burst: `before` feeds the single undo step, `live`
/// travels under the overlay while doc+revision stay frozen.
struct NudgeBurst {
    id: AnnotationId,
    before: Annotation,
    live: Annotation,
    at: std::time::Instant,
}

enum Drag {
    /// A shape/line drag: from `anchor` to current pos.
    Shape { anchor: Pos2 },
    /// Pencil/eraser/highlight strokes accumulate points.
    Freehand { points: Vec<Pos2> },
    /// Moving an existing object. `live` is the working copy painted
    /// by the overlay — the document itself stays untouched (and its
    /// revision frozen) until release.
    Moving {
        id: AnnotationId,
        before: Annotation,
        live: Annotation,
        last: Pos2,
    },
    /// Resizing via a corner handle (0..=7).
    Resizing {
        id: AnnotationId,
        before: Annotation,
        live: Annotation,
        handle: usize,
    },
    /// Dragging a line/fancy/text anchor (false = `a`, true = `b`;
    /// `top` marks a top-edge corner of a text quad).
    Endpoint {
        id: AnnotationId,
        before: Annotation,
        live: Annotation,
        which_b: bool,
        top: bool,
    },
    /// Dragging a text-box tail anchor.
    Tail {
        id: AnnotationId,
        before: Annotation,
        live: Annotation,
    },
    /// Framing a marquee region (SelectArea tool).
    Marquee { anchor: Pos2 },
    /// Press inside a live marquee — becomes a pixel move once the drag
    /// travels (a plain click keeps the selection).
    MoveSel { start: Pos2 },
    /// Dragging one of the marquee's 8 resize anchors (0..=7, same
    /// order as object handles).
    SelResize { handle: usize },
    /// Press on empty canvas with the Select tool while the image
    /// overflows the window — the pointer scrolls the viewport.
    Pan,
}

/// The marquee's live region (SelectArea tool) — a transient frame over
/// the canvas, not an annotation: its ops act on the rendered pixels.
#[derive(Clone, Copy, Debug)]
struct SelArea {
    /// Selected bounds, image physical px (normalised).
    rect: Rect,
    /// Colour of the pixel where the drag started — the fill colour
    /// painted under the region on move / cut / delete.
    seed: Color32,
}

/// The floating "Texte" edit panel: edits the object live and commits
/// one Replace on close, exactly like the reference's text window.
struct TextPanel {
    id: AnnotationId,
    is_box: bool,
    /// Object state when the panel opened — the Replace's `before`.
    before: Annotation,
    /// One-shot: focus the text field when the panel first appears.
    focus_pending: bool,
}

/// How the session ended — read by the app to commit or drop.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// OK: write `doc` + history back into the tab.
    Ok,
    /// Annuler: drop everything.
    Cancel,
}

/// The live draw-window session.
pub struct DrawSession {
    /// Working copy of the tab's document.
    pub doc: Document,
    /// Index of the tab being edited.
    pub tab: usize,
    /// Title of the capture being edited (used as the Draw window title).
    pub title: String,
    /// Chronological stacks — moved out of the tab at open, written
    /// back on OK, restored untouched on Annuler.
    pub undo: VecDeque<AnnoEdit>,
    pub redo: VecDeque<AnnoEdit>,
    tool: Tool,
    opts: Opts,
    selected: Option<AnnotationId>,
    /// Object the z-order context menu was opened on (right-click) —
    /// remembered because `hit` reports nothing while the pointer sits
    /// over the popup itself.
    ctx_menu_id: Option<AnnotationId>,
    drag: Option<Drag>,
    /// The image is bigger than the viewport — Select's empty-canvas
    /// press becomes a pan gesture (set per frame by `canvas`).
    pan_ok: bool,
    /// The live marquee (SelectArea) — cleared on tool switch, Escape,
    /// click-outside, or when the region is lifted into an object.
    sel_area: Option<SelArea>,
    /// Ids of the fill patches painted under moved/cut regions: they are
    /// annotations (chronological undo) but pixel-like — not pickable.
    sel_patches: Vec<AnnotationId>,
    /// True when `Event::Paste` already fired on the key-down of ⌘/Ctrl+V,
    /// so the matching `Key::V` release does not paste a second time.
    paste_on_press: bool,
    text_panel: Option<TextPanel>,
    zoom: Option<f32>,
    texture: Option<TextureHandle>,
    rendered_img: Option<image::RgbaImage>,
    loupe_tex: Option<TextureHandle>,
    copied_at: Option<std::time::Instant>,
    rendered_rev: u64,
    outcome: Option<Outcome>,
    /// The unsaved-changes confirmation is up (Échap/Annuler/close with
    /// edits pending).
    confirm_quit: bool,
    /// Postcard snapshot of `doc.annotations` at open — the session is
    /// dirty only when the current annotations differ from this baseline
    /// (so an edit followed by Undo returns to clean).
    open_snapshot: Vec<u8>,
    /// Last pointer position in image coords (shape-drag end point).
    last_pointer: Option<Pos2>,
    /// Whether Ctrl or Shift is held this frame (horizontal/vertical snap).
    snap_hv: bool,
    /// The canvas rect of the document image (set every frame).
    img_rect: Rect,
    /// Canvas zoom actually used for painting (screen→image px for grabs).
    canvas_scale: f32,
    /// Lazily decoded emoji textures (strip cells, ghost, drag preview).
    /// `None` caches a known-bad id so it is not re-decoded every frame.
    emoji_texs: HashMap<u32, Option<TextureHandle>>,
    /// Sticker being dragged out of the options-bar strip — drops on
    /// release inside the image (anywhere else: quietly cancelled).
    emoji_dnd: Option<u32>,
    /// The shared full-frame scratch behind region patches and the
    /// live-drag preview — allocated once, cleared bounded.
    region_scratch: Option<render::DragPreview>,
    /// Texture carrying the per-frame live-drag preview patch.
    preview_tex: Option<TextureHandle>,
    /// `(origin_x, origin_y, width, height)` of the last uploaded
    /// [`Self::preview_tex`] patch — reused without re-rasterizing or
    /// re-uploading when `live` has not moved since the last frame.
    preview_patch: Option<(u32, u32, u32, u32)>,
    /// Whether `live` mutated since [`Self::preview_tex`] was last updated.
    preview_dirty: bool,
    /// The region currently excluded from the canvas texture by a
    /// live drag (patched back on release or Esc).
    drag_region: Option<(u32, u32, u32, u32)>,
    /// Active emoji-section tab in the bottom bar (index into
    /// [`crate::editor::emoji::sections`]).
    emoji_section: usize,
    /// An in-flight arrow-nudge burst: `before` for the undo entry,
    /// `live` the travelling copy painted by the overlay (doc and
    /// revision frozen until the burst commits — same mechanism as
    /// object drags).
    nudge: Option<NudgeBurst>,
}

impl DrawSession {
    /// Opens a session on `tab`'s document, adopting its history stacks.
    pub fn open(
        doc: Document,
        tab: usize,
        title: String,
        undo: VecDeque<AnnoEdit>,
        redo: VecDeque<AnnoEdit>,
        prefs: &crate::config::Settings,
    ) -> Self {
        let open_snapshot = postcard::to_allocvec(&doc.annotations).unwrap_or_default();
        Self {
            doc,
            tab,
            title,
            undo,
            redo,
            tool: Tool::Select,
            opts: Opts::from_settings(prefs),
            selected: None,
            ctx_menu_id: None,
            drag: None,
            pan_ok: false,
            sel_area: None,
            sel_patches: Vec::new(),
            paste_on_press: false,
            text_panel: None,
            zoom: None,
            texture: None,
            rendered_img: None,
            loupe_tex: None,
            copied_at: None,
            rendered_rev: u64::MAX,
            outcome: None,
            confirm_quit: false,
            open_snapshot,
            last_pointer: None,
            snap_hv: false,
            img_rect: Rect::NOTHING,
            canvas_scale: 1.0,
            emoji_texs: HashMap::new(),
            emoji_dnd: None,
            emoji_section: 0,
            region_scratch: None,
            preview_tex: None,
            preview_patch: None,
            preview_dirty: false,
            drag_region: None,
            nudge: None,
        }
    }

    /// The resolved end-of-session, if any.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// Commits any in-flight text panel edit or arrow-nudge burst into
    /// `self.doc` and `self.undo` before the session is inspected or committed.
    pub fn finalize_pending(&mut self) {
        self.commit_nudge();
        self.close_text_panel();
    }

    /// Whether the annotations currently differ from the state at `open`.
    pub fn has_uncommitted_edits(&self) -> bool {
        self.nudge.is_some()
            || postcard::to_allocvec(&self.doc.annotations).unwrap_or_default()
                != self.open_snapshot
    }

    /// Cancels the session — discarding edits is only silent when there
    /// are none; otherwise the user confirms through a warning box.
    /// Called by Annuler, Escape and the window's close control.
    pub fn request_cancel(&mut self) {
        self.commit_nudge();
        self.close_text_panel();
        if !self.has_uncommitted_edits() {
            self.outcome = Some(Outcome::Cancel);
        } else {
            self.confirm_quit = true;
        }
    }

    /// An object-drag has just started: patch the dragged object's
    /// painted region WITHOUT it (base + every other annotation) so
    /// the canvas texture loses the object at its old spot — while
    /// `live` travels on top via the overlay. The doc's revision is
    /// untouched: no full re-render, no thumbnail churn.
    fn begin_live_drag(&mut self) {
        let Some(before) = self.drag_before() else {
            return;
        };
        self.begin_nudge_live(&before);
    }

    /// The `before` annotation of the live object-drag in progress.
    fn drag_before(&self) -> Option<Annotation> {
        match &self.drag {
            Some(Drag::Moving { before, .. })
            | Some(Drag::Resizing { before, .. })
            | Some(Drag::Endpoint { before, .. })
            | Some(Drag::Tail { before, .. }) => Some(before.clone()),
            _ => None,
        }
    }

    /// The `live` annotation currently travelling — object drag or
    /// arrow-nudge burst. `None` outside those gestures.
    fn live_anno(&self) -> Option<&Annotation> {
        let from_drag = match &self.drag {
            Some(Drag::Moving { live, .. })
            | Some(Drag::Resizing { live, .. })
            | Some(Drag::Endpoint { live, .. })
            | Some(Drag::Tail { live, .. }) => Some(live),
            _ => None,
        };
        from_drag.or_else(|| self.nudge.as_ref().map(|n| &n.live))
    }

    /// Re-composites `region` (absolute image px, already expanded)
    /// into `rendered_img` and uploads JUST that rect to the texture —
    /// the bounded re-render behind live drags, nudges, cancels and
    /// releases. `skip` drops one annotation (the dragged object).
    fn patch_canvas_region(&mut self, region: (u32, u32, u32, u32), skip: Option<u64>) {
        self.patch_canvas_region_skip(region, skip.unwrap_or(u64::MAX));
    }

    /// [`Self::patch_canvas_region`] taking the skip id raw
    /// (`u64::MAX` = none — never a real id).
    fn patch_canvas_region_skip(&mut self, region: (u32, u32, u32, u32), skip: u64) {
        let (rx, ry, rw, rh) = region;
        if rw == 0 || rh == 0 {
            self.rendered_rev = self.doc.revision;
            return;
        }
        let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
        let Some(img) = self.rendered_img.as_mut() else {
            return;
        };
        let scratch = self
            .region_scratch
            .get_or_insert_with(|| render::DragPreview::new(iw, ih));
        let patch = scratch.render_region(&self.doc, region, skip);
        image::imageops::replace(img, &patch, rx as i64, ry as i64);
        if let Some(tex) = self.texture.as_mut() {
            tex.set_partial(
                [rx as usize, ry as usize],
                egui::ColorImage::from_rgba_unmultiplied(
                    [rw as usize, rh as usize],
                    patch.as_raw(),
                ),
                egui::TextureOptions::LINEAR,
            );
        }
        // The patched texture IS current — canvas() must not re-render.
        self.rendered_rev = self.doc.revision;
    }

    /// Full-canvas re-render with an optional skipped annotation — used
    /// when `expand_for_effects` exceeds the 50%-of-image threshold so
    /// the dragged object still leaves its original spot cleanly.
    fn render_canvas_filtered(&mut self, skip: Option<u64>) {
        if self.rendered_img.is_none() {
            return;
        }
        let img = render::render_to_image_filtered(&self.doc, skip);
        if let Some(tex) = self.texture.as_mut() {
            let (w, h) = (img.width() as usize, img.height() as usize);
            tex.set(
                ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()),
                TextureOptions::LINEAR,
            );
        }
        self.rendered_img = Some(img);
        self.rendered_rev = self.doc.revision;
    }

    /// Esc mid-object-drag: restore the exclusion patch (object at its
    /// original spot) — no revision bump, no undo entry, no blink.
    fn cancel_live_drag(&mut self) {
        self.drag = None;
        self.preview_patch = None;
        self.preview_dirty = false;
        if let Some(region) = self.drag_region.take() {
            self.patch_canvas_region(region, None);
        } else {
            self.render_canvas_filtered(None);
        }
    }

    /// Commits a finished live-drag or arrow-nudge `Replace` and
    /// patches the effect-expanded union of old and new bounds on the
    /// same frame.
    fn commit_live_replace(&mut self, before: Annotation, after: Annotation) {
        let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
        let had_region = self.drag_region.is_some();
        let old_r = self
            .drag_region
            .take()
            .or_else(|| render::layer_bounds(&before, iw, ih))
            .unwrap_or((0, 0, 0, 0));
        let new_r = render::layer_bounds(&after, iw, ih).unwrap_or((0, 0, 0, 0));
        let union = render::union_rect(old_r, new_r);
        self.preview_patch = None;
        self.preview_dirty = false;
        let edit = AnnoEdit::Replace { before, after };
        edit.apply(&mut self.doc);
        self.undo.push_back(edit);
        if self.undo.len() > ANNO_HISTORY_CAP {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.doc.revision += 1;
        if had_region
            && let Some(region) = render::expand_for_effects(&self.doc, union, iw, ih, 0.5)
        {
            self.patch_canvas_region(region, None);
        } else {
            self.render_canvas_filtered(None);
        }
    }

    /// Applies an edit chronologically (kills redo, bumps revision).
    fn apply(&mut self, edit: AnnoEdit) {
        // A pending arrow-nudge burst belongs BEFORE this edit on the
        // undo stack — flush it first so history stays chronological.
        self.commit_nudge();
        edit.apply(&mut self.doc);
        self.undo.push_back(edit);
        if self.undo.len() > ANNO_HISTORY_CAP {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.doc.revision += 1;
    }

    pub fn undo_once(&mut self) {
        self.commit_nudge();
        if let Some(edit) = self.undo.pop_back() {
            edit.revert(&mut self.doc);
            self.redo.push_back(edit);
            self.doc.revision += 1;
            self.selected = None;
        }
    }

    pub fn redo_once(&mut self) {
        self.commit_nudge();
        if let Some(edit) = self.redo.pop_back() {
            edit.apply(&mut self.doc);
            self.undo.push_back(edit);
            self.doc.revision += 1;
            self.selected = None;
        }
    }

    /// Deletes the selected object (Suppr key and the X button).
    fn delete_selected(&mut self) {
        self.commit_nudge();
        if let Some(id) = self.selected
            && let Some(idx) = self.doc.index_of(id)
        {
            let anno = self.doc.annotations[idx].clone();
            self.apply(AnnoEdit::Remove { anno, index: idx });
            self.selected = None;
        }
    }

    /// Moves `id` to the very top (`front`) or bottom (`back`) of the
    /// z-order — undoable like every annotation edit.
    fn reorder(&mut self, id: AnnotationId, front: bool) {
        let Some(from) = self.doc.index_of(id) else {
            return;
        };
        let to = if front {
            self.doc.annotations.len().saturating_sub(1)
        } else {
            0
        };
        if from != to {
            self.apply(AnnoEdit::Reorder { id, from, to });
        }
    }

    /// Adds a finished object and selects it.
    fn add(&mut self, kind: AnnotationKind) {
        let id = self.doc.alloc_id();
        let anno = Annotation { id, kind };
        let index = self.doc.annotations.len();
        self.apply(AnnoEdit::Add { anno, index });
        self.selected = Some(id);
    }

    /// Topmost annotation under `p` (image coords), inflated by stroke.
    fn hit(&self, p: Pos2, scale: f32) -> Option<AnnotationId> {
        let live = self.live_anno();
        for doc_a in self.doc.annotations.iter().rev() {
            let a = live.filter(|l| l.id == doc_a.id).unwrap_or(doc_a);
            // Marquee fill patches read as pixels, not objects.
            if self.sel_patches.contains(&a.id) {
                continue;
            }
            let extra = match &a.kind {
                AnnotationKind::Line(o) => {
                    if [o.a, o.ka, o.kb, o.b]
                        .iter()
                        .any(|q| p.distance(*q) < 12.0 / scale.max(0.05))
                    {
                        return Some(a.id);
                    }
                    o.width * 2.0 + o.outline + 8.0
                }
                AnnotationKind::Brace(o) => {
                    if [o.a, o.b]
                        .iter()
                        .any(|q| p.distance(*q) < 12.0 / scale.max(0.05))
                    {
                        return Some(a.id);
                    }
                    o.width * 2.0 + o.outline + 8.0
                }
                AnnotationKind::Rect(o) => o.stroke_w * 0.5 + o.outline + 6.0,
                AnnotationKind::Ellipse(o) => o.stroke_w * 0.5 + o.outline + 6.0,
                AnnotationKind::Freehand(o) => o.width / 2.0 + 4.0,
                AnnotationKind::Text(o) => {
                    // Anchor dots can sit right on the AABB edge — count
                    // them in, the bounds test below covers the body.
                    let (ta, tb, tta, ttb) = text_anchors(o);
                    if [ta, tb, tta, ttb]
                        .iter()
                        .any(|q| p.distance(*q) < 12.0 / scale.max(0.05))
                    {
                        return Some(a.id);
                    }
                    4.0
                }
                _ => 4.0,
            };
            let b = a.kind.bounds();
            if p.x >= b.x as f32 - extra
                && p.x <= (b.x + b.width as i32) as f32 + extra
                && p.y >= b.y as f32 - extra
                && p.y <= (b.y + b.height as i32) as f32 + extra
            {
                // Lines/strokes need a real distance check, not bounds.
                match &a.kind {
                    AnnotationKind::Line(o) => {
                        let tab = render::cubic_table(o.a, o.ka, o.kb, o.b, 32);
                        let tol = o.width.max(4.0) * 1.5 + o.outline + 6.0;
                        if tab
                            .windows(2)
                            .any(|w| seg_distance(p, w[0].1, w[1].1) <= tol)
                        {
                            return Some(a.id);
                        }
                    }
                    AnnotationKind::Brace(o) => {
                        let pts = render::brace_polyline(o);
                        let tol = o.width.max(4.0) * 1.5 + o.outline + 6.0;
                        if pts.windows(2).any(|w| seg_distance(p, w[0], w[1]) <= tol) {
                            return Some(a.id);
                        }
                    }
                    AnnotationKind::Text(_) => return Some(a.id),
                    AnnotationKind::Freehand(o) => {
                        let hit = o
                            .points
                            .windows(2)
                            .any(|s| seg_distance(p, s[0], s[1]) <= o.width / 2.0 + 4.0)
                            || o.points.first().is_some_and(|q| p.distance(*q) <= o.width);
                        if hit {
                            return Some(a.id);
                        }
                    }
                    _ => return Some(a.id),
                }
            }
        }
        None
    }

    /// The session's frame — strip + canvas + options bar.
    pub fn show(&mut self, ui: &mut Ui, shared: &Shared) {
        self.hotkeys(ui.ctx());
        egui::Panel::left("draw.strip")
            .resizable(false)
            .exact_size(68.0)
            .show(ui, |ui| self.strip(ui, shared));
        egui::Panel::bottom("draw.opts")
            .resizable(false)
            .exact_size(62.0)
            .show(ui, |ui| self.options_bar(ui, shared));
        egui::CentralPanel::default().show(ui, |ui| self.canvas(ui));
        self.text_panel_window(ui.ctx(), shared);
        self.quit_dialog(ui.ctx());
    }

    /// The unsaved-changes warning — Discard drops the whole clone,
    /// Continue editing stays in the session.
    fn quit_dialog(&mut self, ctx: &egui::Context) {
        if !self.confirm_quit {
            return;
        }
        let resp = widgets::modal_frame(
            ctx,
            egui::Id::new("pixme-draw-quit"),
            &t!("draw.quit.title"),
            340.0,
            |ui| {
                ui.label(t!("draw.quit.body").to_string());
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // Stay is the safe action — primary at far right;
                    // [Jeter][Continuer].
                    if widgets::primary_button(ui, &t!("draw.quit.stay")).clicked() {
                        self.confirm_quit = false;
                    }
                    if widgets::secondary_button(ui, &t!("draw.quit.discard")).clicked() {
                        self.confirm_quit = false;
                        self.outcome = Some(Outcome::Cancel);
                    }
                });
            },
        );
        if resp.should_close() || resp.inner.0 {
            self.confirm_quit = false;
        }
    }

    /// Keyboard: ⌘Z/⇧⌘Z/Ctrl+Y, Delete/Backspace, Esc = Annuler.
    fn hotkeys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        // ⇧⌘Z before ⌘Z — `consume_key(COMMAND, Z)` ignores extra Shift
        // and would swallow ⇧⌘Z if checked first.
        let redo = ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::COMMAND | egui::Modifiers::SHIFT, Key::Z)
                || i.consume_key(egui::Modifiers::COMMAND, Key::Y)
        });
        let undo = !redo && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, Key::Z));
        if redo {
            self.redo_once();
        } else if undo {
            self.undo_once();
        }
        // `egui_winit` intercepts ⌘/Ctrl+X/C/V before emitting `Event::Key`:
        // it emits `Event::Cut` / `Event::Copy` on press, and for paste it
        // only emits `Event::Paste` when the clipboard holds non-empty text
        // (for an image-only clipboard the press is swallowed and only the
        // `Key::V` release reaches egui).
        let (copy, cut, paste) = ctx.input(|i| {
            let copy = i.events.iter().any(|e| matches!(e, egui::Event::Copy))
                || (i.modifiers.command && i.key_pressed(Key::C));
            let cut = i.events.iter().any(|e| matches!(e, egui::Event::Cut))
                || (i.modifiers.command && i.key_pressed(Key::X));
            let paste_ev = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_)))
                || (i.modifiers.command && i.key_pressed(Key::V));
            let v_rel = i.modifiers.command && i.key_released(Key::V);
            let paste = if paste_ev {
                self.paste_on_press = true;
                true
            } else if v_rel {
                !std::mem::replace(&mut self.paste_on_press, false)
            } else {
                false
            };
            (copy, cut, paste)
        });
        if copy && self.sel_area.is_some() {
            self.sel_copy();
        }
        // Eyedropper mode: pressing `C` copies the hovered `#RRGGBB` and
        // picks the colour (armed swatch pick, Pipette tool, or the
        // eraser's pick).
        let pick_id = armed_pick(ctx);
        let pipetting = pick_id.is_some()
            || self.tool == Tool::Pipette
            || (self.tool == Tool::Eraser && self.opts.eraser_pick);
        if pipetting
            && let Some(p) = self.last_pointer
            && ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::C)
                    || i.consume_key(egui::Modifiers::COMMAND, Key::C)
            })
        {
            let c = self.pixel_at(p);
            let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
            if let Ok(mut cb) = arboard::Clipboard::new() {
                let _ = cb.set_text(&hex);
            }
            if let Some(id) = pick_id {
                ctx.data_mut(|d| {
                    d.insert_persisted(pick_done_key(id), c);
                });
                arm_pick(ctx, None);
            } else {
                match self.tool {
                    Tool::Eraser => {
                        self.opts.eraser_color = c;
                        self.opts.eraser_mode = EraserMode::Color;
                    }
                    _ => self.opts.color = c,
                }
            }
            self.copied_at = Some(std::time::Instant::now());
        }
        if cut && self.sel_area.is_some() {
            self.sel_cut();
        }
        if paste {
            self.sel_paste();
        }
        if ctx.input(|i| i.key_pressed(Key::Delete) || i.key_pressed(Key::Backspace)) {
            if self.sel_area.is_some() {
                self.sel_delete();
            } else {
                self.delete_selected();
            }
        }
        // Arrow keys fine-nudge the selected object (1 px, ⇧ = 10 px)
        // in image space — presses in one burst fold into a single
        // Replace committed 400 ms after the last press.
        if self.selected.is_some() && self.text_panel.is_none() {
            let mut d = Vec2::ZERO;
            for (key, dv) in [
                (Key::ArrowLeft, Vec2::new(-1.0, 0.0)),
                (Key::ArrowRight, Vec2::new(1.0, 0.0)),
                (Key::ArrowUp, Vec2::new(0.0, -1.0)),
                (Key::ArrowDown, Vec2::new(0.0, 1.0)),
            ] {
                if ctx.input_mut(|i| {
                    i.consume_key(egui::Modifiers::SHIFT, key)
                        || i.consume_key(egui::Modifiers::NONE, key)
                }) {
                    d += dv;
                }
            }
            if d != Vec2::ZERO
                && let Some(id) = self.selected
            {
                let step = if ctx.input(|i| i.modifiers.shift) {
                    10.0
                } else {
                    1.0
                };
                // Start/refresh the burst BEFORE borrowing the document.
                if matches!(&self.nudge, Some(n) if n.id == id) {
                    if let Some(n) = &mut self.nudge {
                        n.at = std::time::Instant::now();
                    }
                } else {
                    self.commit_nudge();
                    if let Some(a) = self.doc.annotation(id).cloned() {
                        self.nudge = Some(NudgeBurst {
                            id,
                            before: a.clone(),
                            live: a.clone(),
                            at: std::time::Instant::now(),
                        });
                        // Same exclusion patch as an object-drag start:
                        // the object's old spot leaves the texture now.
                        self.begin_nudge_live(&a);
                    }
                }
                if let Some(n) = &mut self.nudge {
                    n.live.kind.translate(d.x * step, d.y * step);
                    self.preview_dirty = true;
                }
            }
        }
        self.commit_nudge_due(ctx);

        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            arm_pick(ctx, None);
            if self.opts.eraser_pick {
                self.opts.eraser_pick = false;
            } else if self.text_panel.is_some() {
                self.close_text_panel();
            } else if matches!(self.drag, Some(Drag::Shape { .. })) {
                // Esc mid-drag cancels the in-flight creation (emoji
                // drag included) — nothing was committed yet.
                self.drag = None;
            } else if matches!(
                self.drag,
                Some(
                    Drag::Moving { .. }
                        | Drag::Resizing { .. }
                        | Drag::Endpoint { .. }
                        | Drag::Tail { .. }
                )
            ) {
                // Esc mid-object-drag restores the exclusion patch —
                // the doc/revision/undo were never touched.
                self.cancel_live_drag();
            } else if self.emoji_dnd.is_some() {
                self.emoji_dnd = None;
            } else if self.sel_area.is_some() {
                self.sel_area = None;
            } else if self.tool == Tool::Emoji {
                // Esc leaves the sticky emoji tool (back to Select).
                self.tool = Tool::Select;
            } else {
                self.request_cancel();
            }
        }
    }

    /// The exclusion patch behind an arrow-nudge burst or live drag —
    /// patches `before`'s expanded region without `before.id`, or falls
    /// back to a filtered full render when the region exceeds 50% of
    /// the image area.
    fn begin_nudge_live(&mut self, before: &Annotation) {
        let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
        let region = render::layer_bounds(before, iw, ih).unwrap_or((0, 0, 0, 0));
        let region = render::expand_for_effects(&self.doc, region, iw, ih, 0.5);
        self.drag_region = region;
        self.preview_dirty = true;
        match region {
            Some(r) => self.patch_canvas_region_skip(r, before.id),
            None => self.render_canvas_filtered(Some(before.id)),
        }
    }

    /// Commits a finished arrow-nudge burst as ONE undoable Replace —
    /// one revision bump, then the union region patched same-frame.
    fn commit_nudge(&mut self) {
        let Some(n) = self.nudge.take() else {
            return;
        };
        self.commit_live_replace(n.before, n.live);
    }

    /// Fires [`Self::commit_nudge`] once the burst has been quiet for
    /// 400 ms or as soon as a pointer button is pressed (keeps repaints
    /// coming while the burst is still open).
    fn commit_nudge_due(&mut self, ctx: &egui::Context) {
        if let Some(n) = &self.nudge {
            if n.at.elapsed() > std::time::Duration::from_millis(400)
                || ctx.input(|i| i.pointer.any_pressed())
            {
                self.commit_nudge();
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    /// Left strip: two columns of tool icons + a bottom utility block.
    fn strip(&mut self, ui: &mut Ui, _shared: &Shared) {
        ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);
            egui::Grid::new("draw.tools")
                .num_columns(2)
                .spacing(vec2(2.0, 2.0))
                .show(ui, |ui| {
                    for (i, tool) in Tool::ALL.iter().enumerate() {
                        let resp = widgets::icon_button(ui, tool.icon(), self.tool == *tool);
                        let resp = resp.on_hover_text(tool.tip());
                        if resp.clicked() {
                            self.pick_tool(*tool, ui.ctx());
                        }
                        if i % 2 == 1 {
                            ui.end_row();
                        }
                    }
                });
            ui.add_space(4.0);
            ui.separator();
            // Utility block: delete + actual-size, undo/redo, zooms, print.
            egui::Grid::new("draw.util")
                .num_columns(2)
                .spacing(vec2(2.0, 2.0))
                .show(ui, |ui| {
                    let del = widgets::icon_button(ui, Icon::X, false)
                        .on_hover_text(t!("draw.del").to_string());
                    if del.clicked() {
                        self.delete_selected();
                    }
                    let one = widgets::icon_button(ui, Icon::Scaling, false)
                        .on_hover_text(t!("zoom.actual").to_string());
                    if one.clicked() {
                        self.zoom = Some(1.0);
                    }
                    ui.end_row();
                    let redo = widgets::icon_button(ui, Icon::Redo2, false)
                        .on_hover_text(t!("menu.edit.redo").to_string());
                    if redo.clicked() {
                        self.redo_once();
                    }
                    let undo = widgets::icon_button(ui, Icon::Undo2, false)
                        .on_hover_text(t!("menu.edit.undo").to_string());
                    if undo.clicked() {
                        self.undo_once();
                    }
                    ui.end_row();
                    let zout = widgets::icon_button(ui, Icon::ZoomOut, false)
                        .on_hover_text(t!("vbar.zoom_out.tooltip").to_string());
                    if zout.clicked() {
                        self.bump_zoom(1.0 / 1.25);
                    }
                    let zin = widgets::icon_button(ui, Icon::ZoomIn, false)
                        .on_hover_text(t!("vbar.zoom_in.tooltip").to_string());
                    if zin.clicked() {
                        self.bump_zoom(1.25);
                    }
                    ui.end_row();
                    let print = widgets::icon_button(ui, Icon::Printer, false)
                        .on_hover_text(t!("vbar.print.tooltip").to_string());
                    if print.clicked() {
                        let _ =
                            crate::export::file::print_image(&render::render_to_image(&self.doc));
                    }
                });
        });
    }

    /// Handles a tool click.
    fn pick_tool(&mut self, tool: Tool, ctx: &egui::Context) {
        self.tool = tool;
        self.sel_area = None;
        self.selected = None;
        self.close_text_panel();
        // A pending arrow-nudge commits before the tool switch; a
        // half-finished emoji drag-out dies with it too.
        self.commit_nudge();
        self.emoji_dnd = None;
        // A swatch's armed pick dies with the tool change.
        arm_pick(ctx, None);
    }

    /// Zoom ×factor, starting from the current effective scale.
    fn bump_zoom(&mut self, factor: f32) {
        let cur = self.zoom.unwrap_or(1.0);
        self.zoom = Some((cur * factor).clamp(0.1, 8.0));
    }

    // ── Marquee selection (SelectArea) ─────────────────────────────

    /// Lazily decoded emoji texture (picker cells, ghost, drag preview).
    /// A `None` entry caches a known-bad id so it is not re-decoded
    /// every frame. Static so field-level borrows of `self` suffice.
    fn emoji_tex(
        map: &mut HashMap<u32, Option<TextureHandle>>,
        ctx: &egui::Context,
        id: u32,
    ) -> Option<TextureId> {
        map.entry(id)
            .or_insert_with(|| {
                crate::editor::emoji::image(id).map(|img| {
                    ctx.load_texture(
                        format!("emoji-{id:x}"),
                        ColorImage::from_rgba_unmultiplied(
                            [img.width() as usize, img.height() as usize],
                            img.as_raw(),
                        ),
                        TextureOptions::LINEAR,
                    )
                })
            })
            .as_ref()
            .map(|t| t.id())
    }

    /// Places the armed emoji centred on `p` (image px) at the
    /// remembered/default side — shared by the click gesture and the
    /// strip drag-out; one placement = one undo step, stays selected.
    fn place_emoji(&mut self, p: Pos2) {
        let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
        let side = emoji_default_side(
            iw.min(ih) as f32,
            self.doc.base.scale_factor,
            self.opts.emoji_size,
        );
        self.add(AnnotationKind::Emoji(EmojiObj {
            rect: emoji_centered(p, side, iw, ih),
            id: self.opts.emoji_id,
        }));
        // One shot per pick: after a placement the tool drops back to
        // Select — placing again means arming another emoji below.
        self.tool = Tool::Select;
    }

    /// Largest emoji side this document allows, image px: 2× the 128 px
    /// source (beyond that the PNG smears), never past the small side.
    fn emoji_side_cap(&self) -> f32 {
        EMOJI_MAX_SIDE.min(
            self.doc
                .base
                .image
                .width()
                .min(self.doc.base.image.height()) as f32,
        )
    }

    /// Colour of the rendered document pixel at `p` — what the marquee
    /// memorises to fill the hole a move/cut/delete leaves behind.
    fn pixel_at(&self, p: Pos2) -> Color32 {
        let fallback;
        let img = match &self.rendered_img {
            Some(img) => img,
            None => {
                fallback = render::render_to_image(&self.doc);
                &fallback
            }
        };
        let (w, h) = (img.width() as i64, img.height() as i64);
        let x = (p.x as i64).clamp(0, (w - 1).max(0)) as u32;
        let y = (p.y as i64).clamp(0, (h - 1).max(0)) as u32;
        let px = img.get_pixel(x, y).0;
        Color32::from_rgba_unmultiplied(px[0], px[1], px[2], 255)
    }

    /// Which of the marquee's 8 resize anchors sits under `p` (image
    /// coords) — same index order as object handles.
    fn sel_handle_at(&self, p: Pos2, scale: f32) -> Option<usize> {
        let sel = self.sel_area?;
        let r = sel.rect;
        let grab = 10.0 / scale.max(0.05);
        let pts = [
            r.left_top(),
            pos2(r.center().x, r.top()),
            r.right_top(),
            pos2(r.right(), r.center().y),
            r.right_bottom(),
            pos2(r.center().x, r.bottom()),
            r.left_bottom(),
            pos2(r.left(), r.center().y),
        ];
        pts.iter().position(|q| p.distance(*q) < grab)
    }

    /// The live marquee clamped to the image bounds, as a physical rect.
    fn sel_phys(&self) -> Option<PhysicalRect> {
        let sel = self.sel_area?;
        let bounds = Rect::from_min_size(
            Pos2::ZERO,
            vec2(
                self.doc.base.image.width() as f32,
                self.doc.base.image.height() as f32,
            ),
        );
        let r = sel.rect.intersect(bounds);
        if r.width() < 1.0 || r.height() < 1.0 {
            return None;
        }
        Some(PhysicalRect {
            x: r.min.x as i32,
            y: r.min.y as i32,
            width: r.width() as u32,
            height: r.height() as u32,
        })
    }

    /// The marquee's rendered pixels (base + annotations), cropped.
    fn sel_pixels(&self) -> Option<image::RgbaImage> {
        let r = self.sel_phys()?;
        let img = render::render_to_image(&self.doc);
        Some(image::imageops::crop_imm(&img, r.x as u32, r.y as u32, r.width, r.height).to_image())
    }

    /// Paints `color` over `r` — a plain filled-rect annotation, so it
    /// stays a chronological undoable edit. The patch reads as pixels:
    /// its id is recorded so hit-testing skips it.
    fn fill_rect(&mut self, r: PhysicalRect, color: Color32) {
        let index = self.doc.annotations.len();
        let id = self.doc.alloc_id();
        self.sel_patches.push(id);
        self.apply(AnnoEdit::Add {
            anno: Annotation {
                id,
                kind: AnnotationKind::Rect(RectObj {
                    rect: r,
                    stroke: color,
                    stroke_w: 0.0,
                    dash: Dash::Solid,
                    fill: Some(color),
                    radius: 0.0,
                    outline: 0.0,
                    outline_color: Color32::WHITE,
                    opacity: 1.0,
                    shadow: 0.0,
                }),
            },
            index,
        });
    }

    /// Paints `color` over the live marquee.
    fn sel_fill(&mut self, color: Color32) {
        if let Some(r) = self.sel_phys() {
            self.fill_rect(r, color);
        }
    }

    /// Copier: the marquee's pixels go to the OS clipboard. The frame
    /// stays live so another op can follow.
    fn sel_copy(&mut self) {
        if let Some(img) = self.sel_pixels()
            && let Err(e) = crate::export::clipboard::copy_image(&img)
        {
            tracing::warn!("selection copy failed: {e}");
        }
    }

    /// Couper: copy, then fill the hole with the seed colour.
    fn sel_cut(&mut self) {
        let Some(sel) = self.sel_area else { return };
        if let Some(img) = self.sel_pixels()
            && let Err(e) = crate::export::clipboard::copy_image(&img)
        {
            tracing::warn!("selection cut failed: {e}");
        }
        self.sel_fill(sel.seed);
        self.sel_area = None;
    }

    /// Supprimer: the region is filled with the seed colour.
    fn sel_delete(&mut self) {
        let Some(sel) = self.sel_area else { return };
        self.sel_fill(sel.seed);
        self.sel_area = None;
    }

    /// Flou: a parametric blur annotation covering the marquee.
    fn sel_blur(&mut self, pct: u8) {
        let Some(r) = self.sel_phys() else { return };
        let index = self.doc.annotations.len();
        let id = self.doc.alloc_id();
        self.apply(AnnoEdit::Add {
            anno: Annotation {
                id,
                kind: AnnotationKind::Blur(BlurObj {
                    rect: r,
                    style: BlurStyle::Blur,
                    shape: AreaShape::Rect,
                    strength: pct,
                }),
            },
            index,
        });
    }

    /// Coller: the clipboard image drops in as a movable Picture object
    /// (at the marquee's corner when one is live, centred otherwise).
    fn sel_paste(&mut self) {
        let Ok(Some(img)) = crate::export::clipboard::clipboard_image() else {
            return;
        };
        let (doc_w, doc_h) = (
            self.doc.base.image.width() as i32,
            self.doc.base.image.height() as i32,
        );
        let pos = self
            .sel_area
            .map(|s| (s.rect.min.x as i32, s.rect.min.y as i32));
        self.sel_area = None;
        // The pasted object is meant to be moved right away — hand the
        // pointer tool over so a drag grabs it instead of framing a new
        // marquee.
        self.tool = Tool::Select;
        let (w, h) = (img.width(), img.height());
        let (x, y) = pos.unwrap_or((
            ((doc_w - w as i32) / 2).max(0),
            ((doc_h - h as i32) / 2).max(0),
        ));
        self.add(AnnotationKind::Picture(PicObj {
            rect: PhysicalRect {
                x,
                y,
                width: w,
                height: h,
            },
            image: img,
            opacity: 1.0,
            shadow: 0.0,
        }));
    }

    /// A drag inside the marquee lifts its pixels: the region is cropped
    /// out of the rendered document, the hole is filled with the seed
    /// colour, and the pixels become a movable Picture object.
    fn sel_lift(&mut self, start: Pos2) {
        let Some(sel) = self.sel_area else {
            return;
        };
        let Some(r) = self.sel_phys() else {
            return;
        };
        let Some(img) = self.sel_pixels() else {
            return;
        };
        self.sel_area = None;
        // The hole left behind is painted with the seed pixel's colour —
        // the rect was computed while the marquee was still live.
        self.fill_rect(r, sel.seed);
        self.add(AnnotationKind::Picture(PicObj {
            rect: r,
            image: img,
            opacity: 1.0,
            shadow: 0.0,
        }));
        // Continue the press as a plain object move on the new picture;
        // after release the pointer tool keeps it re-grabbable.
        self.tool = Tool::Select;
        if let Some(id) = self.selected
            && let Some(before) = self.doc.annotation(id).cloned()
        {
            self.drag = Some(Drag::Moving {
                id,
                live: before.clone(),
                before,
                last: start,
            });
            self.begin_live_drag();
        }
    }

    /// The canvas: composited texture + interaction overlay.
    fn canvas(&mut self, ui: &mut Ui) {
        // Re-render only on revision changes — one path for display and
        // export.
        if self.rendered_rev != self.doc.revision {
            let img = render::render_to_image(&self.doc);
            let (w, h) = (img.width() as usize, img.height() as usize);
            let color = ColorImage::from_rgba_unmultiplied([w, h], img.as_raw());
            match &mut self.texture {
                Some(tex) => tex.set(color, TextureOptions::LINEAR),
                None => {
                    self.texture = Some(ui.ctx().load_texture(
                        "draw.doc",
                        color,
                        TextureOptions::LINEAR,
                    ));
                }
            }
            self.rendered_img = Some(img);
            self.rendered_rev = self.doc.revision;
        }
        let Some(tex_id) = self.texture.as_ref().map(|t| t.id()) else {
            return;
        };
        let (iw, ih) = (
            self.doc.base.image.width() as f32,
            self.doc.base.image.height() as f32,
        );
        let avail = ui.available_size();
        let scale = self
            .zoom
            .unwrap_or_else(|| ((avail.x - 24.0) / iw).min((avail.y - 24.0) / ih).max(0.05));
        let size = vec2(iw * scale, ih * scale);
        self.pan_ok = size.x > avail.x || size.y > avail.y;
        let mut out = ScrollArea::both()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (resp, painter) =
                    ui.allocate_painter(size.max(vec2(8.0, 8.0)), Sense::click_and_drag());
                let img_rect = resp.rect;
                painter.image(
                    tex_id,
                    img_rect,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
                let to_img = |p: Pos2| {
                    pos2(
                        (p.x - img_rect.min.x) / scale,
                        (p.y - img_rect.min.y) / scale,
                    )
                };
                self.pointer(ui, &resp, &painter, to_img, scale);
                self.draw_overlay(&painter, img_rect, scale);
                let menu_id = self.ctx_menu_id;
                egui::Popup::context_menu(&resp).show(|ui| {
                    let Some(id) = menu_id else {
                        ui.close();
                        return;
                    };
                    if ui.button(t!("draw.ctx.front").to_string()).clicked() {
                        self.reorder(id, true);
                        ui.close();
                    }
                    if ui.button(t!("draw.ctx.back").to_string()).clicked() {
                        self.reorder(id, false);
                        ui.close();
                    }
                });
            });
        // A Select pan drag scrolls the viewport by the pointer delta —
        // the same drag-to-pan as the viewer's tabs canvas.
        if matches!(self.drag, Some(Drag::Pan)) {
            let delta = ui.input(|i| i.pointer.delta());
            out.state.offset -= delta;
            out.state.store(ui.ctx(), out.id);
        }
        // Edge autoscroll: any drag reaching the viewport's edge keeps
        // scrolling toward it — strokes, shapes and moves can extend
        // past the visible area while zoomed. Speed follows how deep
        // the pointer presses into the 24 px margin band.
        if self.drag.is_some()
            && let Some(pos) = ui.input(|i| i.pointer.latest_pos())
        {
            const EDGE: f32 = 24.0;
            let r = out.inner_rect;
            let mut d = Vec2::ZERO;
            if pos.x < r.min.x + EDGE {
                d.x = pos.x - r.min.x - EDGE;
            }
            if pos.x > r.max.x - EDGE {
                d.x = pos.x - r.max.x + EDGE;
            }
            if pos.y < r.min.y + EDGE {
                d.y = pos.y - r.min.y - EDGE;
            }
            if pos.y > r.max.y - EDGE {
                d.y = pos.y - r.max.y + EDGE;
            }
            if d != Vec2::ZERO {
                out.state.offset += d * 0.5;
                out.state.store(ui.ctx(), out.id);
                // Keep frames coming while the button is held still at
                // the edge — pointer delta alone would stop repaints.
                ui.ctx().request_repaint();
            }
        }
    }

    /// Pointer routing per tool on the canvas.
    fn pointer(
        &mut self,
        ui: &mut Ui,
        resp: &Response,
        painter: &egui::Painter,
        to_img: impl Fn(Pos2) -> Pos2,
        scale: f32,
    ) {
        // A sticker dragged out of the options-bar strip drops on
        // release: inside the image it is placed, anywhere else the
        // drag-out is quietly cancelled.
        if self.emoji_dnd.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.emoji_dnd = None;
            if let Some(pos) = ui.input(|i| i.pointer.latest_pos()) {
                let p = to_img(pos);
                let (iw, ih) = (
                    self.doc.base.image.width() as f32,
                    self.doc.base.image.height() as f32,
                );
                if p.x >= 0.0 && p.y >= 0.0 && p.x < iw && p.y < ih {
                    self.place_emoji(p);
                }
            }
            return;
        }
        let Some(hover) = resp.hover_pos().or_else(|| {
            // While dragging, keep tracking even under the floating
            // text panel (its layer shadows hover_pos).
            if self.drag.is_some() {
                ui.ctx().input(|i| i.pointer.latest_pos())
            } else {
                None
            }
        }) else {
            self.last_pointer = None;
            // Pointer left the canvas — settle any freehand drag.
            if let Some(Drag::Freehand { points }) = self.drag.take() {
                self.finish_freehand(points);
            }
            return;
        };
        self.canvas_scale = scale;
        let img_p = to_img(hover);
        self.last_pointer = Some(img_p);
        self.snap_hv = ui.ctx().input(|i| i.modifiers.ctrl || i.modifiers.shift);

        // Cursor feedback: a movable object under the pointer shows the
        // move icon (like the reference editor); crosshair for drawing.
        // Cursor: anchors ALWAYS show a grab cursor (any tool); over any
        // existing object (except pure pixel tools) show Move so the user
        // always sees it can be selected/moved; otherwise the tool's cursor.
        let panel_hides_chrome = self
            .text_panel
            .as_ref()
            .is_some_and(|p| p.is_box && Some(p.id) == self.selected);
        let on_handle = !panel_hides_chrome
            && (self
                .selected
                .and_then(|id| self.handle_at(id, img_p, scale))
                .is_some()
                || self.sel_handle_at(img_p, scale).is_some());
        let hit_id = self.hit(img_p, scale);
        // The marquee tool works on regions, not objects — it never
        // picks them (same for the pure pixel tools).
        let can_pick_existing = !matches!(self.tool, Tool::Eraser | Tool::SelectArea)
            && !matches!(self.drag, Some(Drag::Shape { .. } | Drag::Freehand { .. }));
        let in_sel = self.sel_area.is_some_and(|s| s.rect.contains(img_p));
        let cursor = if on_handle {
            egui::CursorIcon::PointingHand
        } else if matches!(self.drag, Some(Drag::Pan)) {
            egui::CursorIcon::Grabbing
        } else if matches!(self.drag, Some(Drag::Moving { .. } | Drag::MoveSel { .. }))
            || (can_pick_existing && hit_id.is_some())
            || in_sel
        {
            egui::CursorIcon::Move
        } else if self.tool == Tool::Select && self.pan_ok && hit_id.is_none() {
            egui::CursorIcon::Grab
        } else if self.tool == Tool::Select {
            egui::CursorIcon::Default
        } else {
            egui::CursorIcon::Crosshair
        };
        ui.ctx().set_cursor_icon(cursor);

        // Right-click an object: it becomes the selection and arms the
        // z-order menu drawn in `canvas` (it must run even while the
        // pointer is off-canvas over the popup, or it would close).
        if resp.secondary_clicked() {
            self.ctx_menu_id = hit_id.filter(|_| can_pick_existing);
            if self.ctx_menu_id.is_some() {
                self.selected = self.ctx_menu_id;
            }
        }

        // Commit a pending move/resize on release.
        if !resp.dragged()
            && !resp.is_pointer_button_down_on()
            && let Some(drag) = self.drag.take()
        {
            self.finish_drag(drag);
        }

        if resp.double_clicked() {
            // Double-click on a text/box object: commit any previous open
            // panel, select the target, and open its floating text editor.
            if let Some(id) = hit_id {
                if self.text_panel.as_ref().is_some_and(|p| p.id != id) {
                    self.close_text_panel();
                }
                self.selected = Some(id);
                self.open_text_panel(id);
            }
        } else if resp.drag_started_by(egui::PointerButton::Primary) {
            self.start_drag(img_p, ui.ctx());
        } else if resp.dragged() {
            self.continue_drag(img_p);
        } else if resp.clicked() && self.drag.is_none() {
            self.click(img_p, ui.ctx());
        }

        // The emoji drag preview needs its texture — fetched BEFORE the
        // `&self.drag` borrow so `emoji_tex` can take `&mut` of the cache.
        let emoji_tex = if self.tool == Tool::Emoji {
            Self::emoji_tex(&mut self.emoji_texs, ui.ctx(), self.opts.emoji_id)
        } else {
            None
        };
        // Live preview while dragging.
        if let Some(drag) = &self.drag {
            self.preview(painter, drag, img_p, scale, emoji_tex);
        }
    }

    /// Press: begin a creation drag or grab an object.
    fn start_drag(&mut self, p: Pos2, ctx: &egui::Context) {
        self.commit_nudge();
        // An armed swatch pick consumes the press: the pixel colour goes
        // to the swatch's pending slot and the picker disarms.
        if let Some(id) = armed_pick(ctx) {
            let c = self.pixel_at(p);
            ctx.data_mut(|d| {
                d.insert_persisted(pick_done_key(id), c);
            });
            arm_pick(ctx, None);
            return;
        }
        // Handles of the currently selected object ALWAYS grab first,
        // unless hidden while a box's floating text panel is open.
        let panel_hides_chrome = self
            .text_panel
            .as_ref()
            .is_some_and(|tp| tp.is_box && Some(tp.id) == self.selected);
        if !panel_hides_chrome
            && let Some(id) = self.selected
            && let Some(handle) = self.handle_at(id, p, self.canvas_scale)
            && let Some(before) = self.doc.annotation(id).cloned()
        {
            self.drag = Some(match handle {
                Handle::Endpoint(which_b, top) => Drag::Endpoint {
                    id,
                    live: before.clone(),
                    before,
                    which_b,
                    top,
                },
                Handle::Tail => Drag::Tail {
                    id,
                    live: before.clone(),
                    before,
                },
                Handle::Corner(h) => Drag::Resizing {
                    id,
                    live: before.clone(),
                    before,
                    handle: h,
                },
            });
            self.begin_live_drag();
            return;
        }
        // Any press outside the floating panel closes it first (boxes stay
        // even when empty) and then proceeds with picking or creating.
        if self.text_panel.is_some() {
            self.close_text_panel();
        }
        // Dragging over an existing object selects and moves it, regardless
        // of which creation tool is armed (except pure pixel tools and the
        // marquee, which frames regions rather than grabbing objects).
        if !matches!(self.tool, Tool::Eraser | Tool::SelectArea | Tool::Pipette)
            && let Some(id) = self.hit(p, self.canvas_scale)
            && let Some(before) = self.doc.annotation(id).cloned()
        {
            self.selected = Some(id);
            self.drag = Some(Drag::Moving {
                id,
                live: before.clone(),
                before,
                last: p,
            });
            self.begin_live_drag();
            return;
        }
        self.selected = None;
        match self.tool {
            Tool::SelectArea => {
                // A press on an anchor resizes the frame; inside it
                // lifts the pixels on move; anywhere else starts a new
                // frame and memorises the pressed pixel's colour.
                if let Some(h) = self.sel_handle_at(p, self.canvas_scale) {
                    self.drag = Some(Drag::SelResize { handle: h });
                } else if self.sel_area.is_some_and(|s| s.rect.contains(p)) {
                    self.drag = Some(Drag::MoveSel { start: p });
                } else {
                    let seed = self.pixel_at(p);
                    self.sel_area = Some(SelArea {
                        rect: Rect::from_min_max(p, p),
                        seed,
                    });
                    self.drag = Some(Drag::Marquee { anchor: p });
                }
            }
            Tool::Select => {
                // Press on empty canvas while the image overflows the
                // window: the pointer pans the viewport (like the
                // viewer's drag-to-pan) instead of doing nothing.
                if self.pan_ok {
                    self.drag = Some(Drag::Pan);
                }
            }
            Tool::Text | Tool::Step => {}
            Tool::Pipette => {
                // A press picks the pixel's colour into the shared
                // stroke colour — `C` copies its `#RRGGBB`.
                self.opts.color = self.pixel_at(p);
            }
            Tool::Eraser => {
                if self.opts.eraser_pick {
                    self.opts.eraser_color = self.pixel_at(p);
                    self.opts.eraser_mode = EraserMode::Color;
                    self.opts.eraser_pick = false;
                } else {
                    self.drag = Some(Drag::Freehand { points: vec![p] });
                }
            }
            Tool::Pencil => {
                self.drag = Some(Drag::Freehand { points: vec![p] });
            }
            _ => self.drag = Some(Drag::Shape { anchor: p }),
        }
    }

    /// Move: apply the drag delta or extend a stroke.
    fn continue_drag(&mut self, p: Pos2) {
        // A drag that started inside the marquee lifts the region's
        // pixels into a movable Picture once it has really travelled.
        if let Some(Drag::MoveSel { start }) = self.drag
            && p.distance(start) > 2.0
        {
            self.sel_lift(start);
        }
        // Needed inside the `&mut self.drag` match — fetch it first.
        let emoji_cap = self.emoji_side_cap();
        match &mut self.drag {
            Some(Drag::Freehand { points }) => points.push(p),
            // Object drags mutate `live` ONLY — the document (and its
            // revision, and the thumbnail key) stays frozen until the
            // release commits one Replace. The overlay paints `live`.
            Some(Drag::Moving { last, live, .. }) => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                live.kind.translate(dx, dy);
                self.preview_dirty = true;
            }
            Some(Drag::Resizing { handle, live, .. }) => {
                let h = *handle;
                let mut r = live.kind.bounds();
                if matches!(live.kind, AnnotationKind::Emoji(_)) {
                    // Emojis stay square — aspect-locked resize.
                    resize_square(&mut r, h, p, EMOJI_MIN_SIDE, emoji_cap);
                } else {
                    resize_rect(&mut r, h, p);
                }
                live.kind.set_bounds(r);
                self.preview_dirty = true;
            }
            Some(Drag::Endpoint {
                live, which_b, top, ..
            }) => {
                let (wb, top) = (*which_b, *top);
                let snap = self.snap_hv;
                match &mut live.kind {
                    AnnotationKind::Line(o) => {
                        drag_bezier_endpoint(&mut o.a, &mut o.ka, &mut o.kb, &mut o.b, wb, top, p);
                    }
                    AnnotationKind::Text(o) => {
                        drag_bezier_endpoint(&mut o.a, &mut o.ka, &mut o.kb, &mut o.b, wb, top, p);
                    }
                    AnnotationKind::Brace(o) => {
                        if wb {
                            let next = snap_hv_pt(o.a, p, snap);
                            if (next - o.a).length() >= 8.0 {
                                o.b = next;
                            }
                        } else {
                            let next = snap_hv_pt(o.b, p, snap);
                            if (next - o.b).length() >= 8.0 {
                                o.a = next;
                            }
                        }
                    }
                    _ => {}
                }
                self.preview_dirty = true;
            }
            Some(Drag::Tail { live, .. }) => {
                if let AnnotationKind::TextBox(o) = &mut live.kind {
                    o.tail = Some(p);
                }
                self.preview_dirty = true;
            }
            Some(Drag::Marquee { anchor }) => {
                if let Some(sel) = &mut self.sel_area {
                    sel.rect = Rect::from_two_pos(*anchor, p);
                }
            }
            Some(Drag::SelResize { handle }) => {
                if let Some(sel) = &mut self.sel_area {
                    resize_sel(&mut sel.rect, *handle, p);
                }
            }
            Some(Drag::MoveSel { .. } | Drag::Shape { .. } | Drag::Pan) | None => {}
        }
    }

    /// Release: convert the drag into one chronological edit.
    fn finish_drag(&mut self, drag: Drag) {
        match drag {
            Drag::Moving {
                id,
                before,
                mut live,
                ..
            }
            | Drag::Resizing {
                id,
                before,
                mut live,
                ..
            }
            | Drag::Endpoint {
                id,
                before,
                mut live,
                ..
            }
            | Drag::Tail {
                id,
                before,
                mut live,
                ..
            } => {
                // Keep a minimum baseline span so the object never
                // collapses to a point.
                if let AnnotationKind::Text(o) = &mut live.kind
                    && (o.b - o.a).length() < 16.0
                {
                    let d = unit_dir(o.a, o.b);
                    o.b = pos2(o.a.x + d.x * 16.0, o.a.y + d.y * 16.0);
                }
                // ONE revision bump for the whole gesture, then patch
                // the effect-expanded union of old+new bounds — same
                // frame, no flicker.
                self.commit_live_replace(before, live);
                self.selected = Some(id);
            }
            Drag::Freehand { points } => self.finish_freehand(points),
            Drag::Marquee { .. } => {
                // Keep only a real frame — a stray click leaves no
                // selection behind.
                if self
                    .sel_area
                    .is_some_and(|s| s.rect.width() < 2.0 || s.rect.height() < 2.0)
                {
                    self.sel_area = None;
                }
            }
            Drag::MoveSel { .. } | Drag::SelResize { .. } | Drag::Pan => {}
            Drag::Shape { anchor } => {
                // Release position is implicit: the last pointer move
                // already gave the preview; use the release point via
                // the stored drag end.
                let end = self.last_pointer.unwrap_or(anchor);
                self.create_shape(anchor, end);
            }
        }
    }

    /// Release-point memory for shape drags (set every frame).
    /// Kept as a field to keep `pointer` signatures small.
    // last_pointer is declared below on the struct… see impl note.
    /// Turns a finished freehand stroke into an object.
    fn finish_freehand(&mut self, points: Vec<Pos2>) {
        if points.is_empty() || (self.tool != Tool::Eraser && points.len() < 2) {
            return;
        }
        let is_eraser = self.tool == Tool::Eraser;
        let kind = match self.tool {
            Tool::Eraser => AnnotationKind::Freehand(FreeObj {
                points,
                color: self.opts.eraser_color,
                width: self.opts.eraser_size,
                tip: TipShape::Round,
                dash: Dash::Solid,
                outline: 0.0,
                outline_color: Color32::WHITE,
                shadow: 0.0,
                eraser_mode: self.opts.eraser_mode,
                soft: false,
                opacity: 1.0,
            }),
            _ => AnnotationKind::Freehand(FreeObj {
                points,
                color: self.opts.color,
                width: self.opts.width,
                tip: self.opts.tip,
                dash: self.opts.dash,
                outline: self.opts.outline,
                outline_color: self.opts.line_outline_color,
                shadow: if self.opts.line_shadow { 3.0 } else { 0.0 },
                eraser_mode: EraserMode::Color,
                soft: false,
                opacity: self.opts.opacity,
            }),
        };
        self.add(kind);
        if is_eraser && let Some(id) = self.selected {
            self.sel_patches.push(id);
        }
        self.selected = None;
    }

    /// Builds a shape annotation from the drag rectangle/segment.
    fn create_shape(&mut self, a: Pos2, b: Pos2) {
        let r = PhysicalRect {
            x: a.x.min(b.x) as i32,
            y: a.y.min(b.y) as i32,
            width: (a.x - b.x).abs().max(1.0) as u32,
            height: (a.y - b.y).abs().max(1.0) as u32,
        };
        let o = &self.opts;
        // The emoji arm can't write `self.opts.emoji_size` while `o`
        // borrows `self.opts` — stashed here, applied after the match.
        let mut emoji_side: Option<f32> = None;
        let kind = match self.tool {
            Tool::Rect => AnnotationKind::Rect(RectObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width.max(1.0),
                dash: o.dash,
                fill: None,
                radius: o.radius,
                outline: o.outline,
                outline_color: o.line_outline_color,
                opacity: o.opacity,
                shadow: if o.line_shadow { 3.0 } else { 0.0 },
            }),
            Tool::RectFill => AnnotationKind::Rect(RectObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: Some(o.fill_color),
                radius: o.radius,
                outline: 0.0,
                outline_color: o.line_outline_color,
                opacity: o.opacity,
                shadow: if o.line_shadow { 3.0 } else { 0.0 },
            }),
            Tool::Ellipse => AnnotationKind::Ellipse(EllipseObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width.max(1.0),
                dash: o.dash,
                fill: None,
                outline: o.outline,
                outline_color: o.line_outline_color,
                opacity: o.opacity,
                shadow: if o.line_shadow { 3.0 } else { 0.0 },
            }),
            Tool::EllipseFill => AnnotationKind::Ellipse(EllipseObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: Some(o.fill_color),
                outline: 0.0,
                outline_color: o.line_outline_color,
                opacity: o.opacity,
                shadow: if o.line_shadow { 3.0 } else { 0.0 },
            }),
            Tool::Arrow => {
                let (ka, kb) = bezier_rest_knobs(a, b);
                AnnotationKind::Line(LineObj {
                    a,
                    b,
                    ka,
                    kb,
                    color: o.color,
                    width: o.width,
                    dash: o.dash,
                    start: o.line_start,
                    end: o.line_end,
                    outline: o.outline,
                    outline_color: o.line_outline_color,
                    opacity: o.opacity,
                    shadow: if o.line_shadow { 3.0 } else { 0.0 },
                })
            }
            Tool::Brace => {
                let mut b = snap_hv_pt(a, b, self.snap_hv);
                if (b - a).length() < 8.0 {
                    let d = if (b - a).length() < 1e-3 {
                        vec2(0.0, 1.0)
                    } else {
                        unit_dir(a, b)
                    };
                    b = a + d * 24.0;
                }
                AnnotationKind::Brace(BraceObj {
                    a,
                    b,
                    kind: o.brace_kind,
                    color: o.color,
                    width: o.width,
                    dash: o.dash,
                    outline: o.outline,
                    outline_color: o.line_outline_color,
                    opacity: o.opacity,
                    shadow: if o.line_shadow { 3.0 } else { 0.0 },
                })
            }
            Tool::HighlightArea => AnnotationKind::HighlightArea(AreaObj {
                rect: r,
                color: o.color,
                shape: o.shape,
                border: None,
                dash: Dash::Solid,
                soft: false,
                opacity: o.hl_opacity,
            }),
            Tool::Blur => AnnotationKind::Blur(BlurObj {
                rect: r,
                style: o.blur_style,
                shape: o.shape,
                strength: o.blur_strength,
            }),
            Tool::Emoji => {
                let r = emoji_square(a, b, EMOJI_MIN_SIDE, self.emoji_side_cap());
                emoji_side = Some(r.width as f32);
                AnnotationKind::Emoji(EmojiObj {
                    rect: r,
                    id: o.emoji_id,
                })
            }
            Tool::TextBoxRect | Tool::TextBoxOval => {
                let oval = self.tool == Tool::TextBoxOval;
                let id = self.doc.alloc_id();
                let anno = Annotation {
                    id,
                    kind: AnnotationKind::TextBox(BoxObj {
                        rect: r,
                        oval,
                        content: String::new(),
                        bg: o.fill_color,
                        border: o.color,
                        border_w: o.width,
                        radius: o.radius,
                        fg: o.outline_color,
                        size: o.font_size,
                        bold: o.bold,
                        italic: o.italic,
                        align: TextAlign::Center,
                        valign: crate::editor::document::BoxVAlign::default(),
                        line_gap: 2.0,
                        tail: None,
                        opacity: o.opacity,
                        shadow: o.shadow,
                    }),
                };
                let index = self.doc.annotations.len();
                self.apply(AnnoEdit::Add { anno, index });
                self.selected = Some(id);
                if let Some(before) = self.doc.annotation(id).cloned() {
                    self.text_panel = Some(TextPanel {
                        id,
                        is_box: true,
                        before,
                        focus_pending: true,
                    });
                }
                return;
            }
            _ => return,
        };
        // A dragged emoji size becomes the next click's remembered side.
        if let Some(side) = emoji_side {
            self.opts.emoji_size = Some(side);
        }
        let one_shot = matches!(kind, AnnotationKind::Emoji(_));
        self.add(kind);
        if one_shot {
            self.tool = Tool::Select;
        }
    }

    /// A plain click (no drag): clicking an existing object selects it
    /// (any non-pixel tool); clicking empty canvas closes an open text
    /// panel or places a point tool.
    fn click(&mut self, p: Pos2, ctx: &egui::Context) {
        self.commit_nudge();
        // Same armed-pick interception as `start_drag` — a click may not
        // have crossed the drag threshold first.
        if let Some(id) = armed_pick(ctx) {
            let c = self.pixel_at(p);
            ctx.data_mut(|d| {
                d.insert_persisted(pick_done_key(id), c);
            });
            arm_pick(ctx, None);
            return;
        }
        let had_panel = self.text_panel.is_some();
        if had_panel {
            self.close_text_panel();
        }
        if !matches!(self.tool, Tool::Eraser | Tool::SelectArea | Tool::Pipette)
            && let Some(id) = self.hit(p, self.canvas_scale)
        {
            self.selected = Some(id);
            self.sync_text_panel();
            return;
        }
        if had_panel {
            return;
        }
        self.selected = None;
        match self.tool {
            Tool::Select => {
                self.sync_text_panel();
            }
            Tool::Emoji => self.place_emoji(p),
            Tool::Eraser => {
                if self.opts.eraser_pick {
                    self.opts.eraser_color = self.pixel_at(p);
                    self.opts.eraser_mode = EraserMode::Color;
                    self.opts.eraser_pick = false;
                } else {
                    self.finish_freehand(vec![p]);
                }
            }
            Tool::Pipette => {
                self.opts.color = self.pixel_at(p);
            }
            Tool::SelectArea => {
                // Inside the frame it stays; a click elsewhere drops it.
                if self.sel_area.is_some_and(|s| !s.rect.contains(p)) {
                    self.sel_area = None;
                }
            }
            Tool::Text => {
                let id = self.doc.alloc_id();
                let anno = Annotation {
                    id,
                    kind: AnnotationKind::Text(TextObj {
                        a: p,
                        b: pos2(p.x + 260.0, p.y),
                        ka: pos2(p.x + 40.0, p.y),
                        kb: pos2(p.x + 220.0, p.y),
                        content: String::new(),
                        font: self.opts.font.clone(),
                        color: self.opts.color,
                        size: self.opts.font_size,
                        bold: self.opts.bold,
                        italic: self.opts.italic,
                        align: TextAlign::Left,
                        outline: self.opts.outline_w,
                        outline_color: self.opts.outline_color,
                        opacity: self.opts.opacity,
                        shadow: self.opts.shadow,
                    }),
                };
                let index = self.doc.annotations.len();
                self.apply(AnnoEdit::Add { anno, index });
                self.selected = Some(id);
                if let Some(before) = self.doc.annotation(id).cloned() {
                    self.text_panel = Some(TextPanel {
                        id,
                        is_box: false,
                        before,
                        focus_pending: true,
                    });
                }
            }
            Tool::Step => {
                let count = match self.opts.step_mode {
                    StepMode::Numbers => {
                        let c = self.opts.step_next_num.max(1);
                        self.opts.step_next_num = c + 1;
                        c
                    }
                    StepMode::Letters => {
                        let c = self.opts.step_next_let.max(1);
                        self.opts.step_next_let = c + 1;
                        c
                    }
                };
                let id = self.doc.alloc_id();
                let anno = Annotation {
                    id,
                    kind: AnnotationKind::Step(StepObj {
                        pos: p,
                        count,
                        mode: self.opts.step_mode,
                        style: self.opts.step_style,
                        size: self.opts.step_size,
                        bg: self.opts.step_bg,
                        fg: self.opts.step_fg,
                        ring: self.opts.step_ring,
                        ring_color: self.opts.step_ring_color,
                        opacity: self.opts.opacity,
                        shadow: if self.opts.step_shadow { 3.0 } else { 0.0 },
                        lower: self.opts.step_lower,
                    }),
                };
                let index = self.doc.annotations.len();
                self.apply(AnnoEdit::Add { anno, index });
            }
            _ => {}
        }
        let _ = ctx;
    }

    /// Which handle (if any) of `id` sits under `p`. `scale` converts
    /// the grab radius from screen px into image px so anchors stay
    /// reachable at any zoom.
    fn handle_at(&self, id: AnnotationId, p: Pos2, scale: f32) -> Option<Handle> {
        let a = self
            .live_anno()
            .filter(|a| a.id == id)
            .or_else(|| self.doc.annotation(id))?;
        let grab = 10.0 / scale.max(0.05);
        match &a.kind {
            AnnotationKind::Line(o) => bezier_handle_at(o.a, o.ka, o.kb, o.b, p, grab),
            AnnotationKind::Text(o) => bezier_handle_at(o.a, o.ka, o.kb, o.b, p, grab),
            AnnotationKind::Brace(o) => {
                if p.distance(o.a) < grab {
                    Some(Handle::Endpoint(false, true))
                } else if p.distance(o.b) < grab {
                    Some(Handle::Endpoint(true, true))
                } else {
                    None
                }
            }
            AnnotationKind::TextBox(o) => {
                if o.tail.is_some_and(|t| p.distance(t) < grab) {
                    return Some(Handle::Tail);
                }
                corner_handle(o.rect, p, grab)
            }
            _ => corner_handle(a.kind.bounds(), p, grab),
        }
    }

    /// Live drag preview drawn over the canvas.
    fn preview(
        &self,
        painter: &egui::Painter,
        drag: &Drag,
        cur: Pos2,
        scale: f32,
        emoji_tex: Option<TextureId>,
    ) {
        // Text anchors: `draw_overlay` already draws the live Bézier +
        // both tangent levers (`a→ka`, `b→kb`).
        if let Drag::Endpoint { id, .. } = drag
            && matches!(
                self.doc.annotation(*id).map(|a| &a.kind),
                Some(AnnotationKind::Text(_))
            )
        {
            return;
        }
        let to_canvas =
            |p: Pos2, rect: Rect| pos2(rect.min.x + p.x * scale, rect.min.y + p.y * scale);
        // The marquee-in-progress paints the same dashed chrome as the
        // live selection.
        if let Drag::Marquee { anchor } = drag {
            let r = Rect::from_two_pos(
                to_canvas(*anchor, self.img_rect),
                to_canvas(cur, self.img_rect),
            );
            sel_chrome(painter, r, theme::tokens(painter.ctx()).accent);
            return;
        }
        // Live freehand stroke preview while dragging pencil / eraser / highlighter.
        if let Drag::Freehand { points } = drag
            && !points.is_empty()
        {
            let o = &self.opts;
            let img_rect = self.img_rect;
            let cpts: Vec<Pos2> = points.iter().map(|&p| to_canvas(p, img_rect)).collect();
            let (col, w_px, out_px) = match self.tool {
                Tool::Eraser => {
                    let preview_col = match o.eraser_mode {
                        EraserMode::Color => o.eraser_color.gamma_multiply(o.opacity),
                        EraserMode::Blur | EraserMode::Pixelate => {
                            Color32::from_rgba_unmultiplied(120, 175, 240, 110)
                        }
                    };
                    (preview_col, (o.eraser_size * scale).max(2.0), 0.0)
                }
                _ => (
                    o.color.gamma_multiply(o.opacity),
                    (o.width * scale).max(1.0),
                    o.outline.max(0.0) * 0.65 * 2.0 * scale,
                ),
            };
            if cpts.len() == 1 {
                if out_px > 0.0 {
                    painter.circle_filled(
                        cpts[0],
                        (w_px + out_px) * 0.5,
                        o.line_outline_color.gamma_multiply(o.opacity),
                    );
                }
                painter.circle_filled(cpts[0], w_px * 0.5, col);
            } else {
                if out_px > 0.0 {
                    painter.add(egui::Shape::line(
                        cpts.clone(),
                        Stroke::new(
                            w_px + out_px,
                            o.line_outline_color.gamma_multiply(o.opacity),
                        ),
                    ));
                }
                painter.add(egui::Shape::line(cpts, Stroke::new(w_px, col)));
            }
            return;
        }
        // Shapes preview from anchor → current point.
        if let Drag::Shape { anchor } = drag {
            let o = &self.opts;
            let img_rect = self.img_rect;
            let ca = to_canvas(*anchor, img_rect);
            let cb = to_canvas(cur, img_rect);
            let accent = theme::tokens(painter.ctx()).accent;
            match self.tool {
                Tool::Rect | Tool::RectFill | Tool::Ellipse | Tool::EllipseFill => {
                    let r = Rect::from_two_pos(ca, cb);
                    let filled = matches!(self.tool, Tool::RectFill | Tool::EllipseFill);
                    let w_px = if filled {
                        (o.width * scale).max(0.0)
                    } else {
                        (o.width.max(1.0) * scale).max(1.0)
                    };
                    let out_px = if filled {
                        0.0
                    } else {
                        o.outline.max(0.0) * 0.65 * 2.0 * scale
                    };
                    let stroke_col = o.color.gamma_multiply(o.opacity);
                    let out_col = o.line_outline_color.gamma_multiply(o.opacity);
                    match self.tool {
                        Tool::Ellipse | Tool::EllipseFill => {
                            let radii = r.size() / 2.0;
                            if filled {
                                painter.add(egui::epaint::EllipseShape::filled(
                                    r.center(),
                                    radii,
                                    o.fill_color.gamma_multiply(o.opacity),
                                ));
                            }
                            if w_px > 0.0 {
                                if out_px > 0.0 {
                                    painter.add(egui::epaint::EllipseShape::stroke(
                                        r.center(),
                                        radii,
                                        Stroke::new(w_px + out_px, out_col),
                                    ));
                                }
                                painter.add(egui::epaint::EllipseShape::stroke(
                                    r.center(),
                                    radii,
                                    Stroke::new(w_px, stroke_col),
                                ));
                            }
                        }
                        _ => {
                            let max_r = (r.width().min(r.height()) * 0.5).min(255.0);
                            let cr = CornerRadius::same(
                                (o.radius * scale).round().clamp(0.0, max_r) as u8,
                            );
                            if filled {
                                painter.rect_filled(r, cr, o.fill_color.gamma_multiply(o.opacity));
                            }
                            if w_px > 0.0 {
                                if out_px > 0.0 {
                                    painter.rect_stroke(
                                        r,
                                        cr,
                                        Stroke::new(w_px + out_px, out_col),
                                        StrokeKind::Middle,
                                    );
                                }
                                painter.rect_stroke(
                                    r,
                                    cr,
                                    Stroke::new(w_px, stroke_col),
                                    StrokeKind::Middle,
                                );
                            }
                        }
                    }
                }
                Tool::HighlightArea => {
                    let r = Rect::from_two_pos(ca, cb);
                    let fill_col = o.color.gamma_multiply(o.hl_opacity);
                    if o.shape == AreaShape::Rect {
                        painter.rect_filled(r, CornerRadius::ZERO, fill_col);
                        painter.rect_stroke(
                            r,
                            CornerRadius::ZERO,
                            Stroke::new(1.5, accent),
                            StrokeKind::Inside,
                        );
                    } else {
                        let radii = r.size() / 2.0;
                        painter.add(egui::epaint::EllipseShape::filled(
                            r.center(),
                            radii,
                            fill_col,
                        ));
                        painter.add(egui::epaint::EllipseShape::stroke(
                            r.center(),
                            radii,
                            Stroke::new(1.5, accent),
                        ));
                    }
                }
                Tool::Blur | Tool::TextBoxRect | Tool::TextBoxOval => {
                    let r = Rect::from_two_pos(ca, cb);
                    painter.rect_stroke(
                        r,
                        CornerRadius::same(
                            if matches!(self.tool, Tool::TextBoxRect | Tool::TextBoxOval) {
                                o.radius as u8
                            } else {
                                0
                            },
                        ),
                        Stroke::new(1.5, accent),
                        StrokeKind::Inside,
                    );
                }
                Tool::Arrow => {
                    painter.line_segment([ca, cb], Stroke::new(o.width.max(1.0), o.color));
                }
                Tool::Brace => {
                    let b_img = snap_hv_pt(*anchor, cur, self.snap_hv);
                    let cb = to_canvas(b_img, img_rect);
                    let w_px = (o.width * scale).max(1.5);
                    painter.line_segment(
                        [ca, cb],
                        Stroke::new(w_px, o.color.gamma_multiply(o.opacity)),
                    );
                    for end_c in [ca, cb] {
                        painter.circle_filled(end_c, 4.0, Color32::WHITE);
                        painter.circle_stroke(end_c, 4.0, Stroke::new(1.0, Color32::from_gray(60)));
                    }
                }
                Tool::Emoji => {
                    // The dragged square at its real content — what the
                    // release will commit, cap included.
                    let r = emoji_square(*anchor, cur, EMOJI_MIN_SIDE, self.emoji_side_cap());
                    let cr = Rect::from_min_max(
                        to_canvas(pos2(r.x as f32, r.y as f32), img_rect),
                        to_canvas(
                            pos2(
                                (r.x + r.width as i32) as f32,
                                (r.y + r.height as i32) as f32,
                            ),
                            img_rect,
                        ),
                    );
                    if let Some(tex) = emoji_tex {
                        painter.image(
                            tex,
                            cr,
                            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                            Color32::from_white_alpha(215),
                        );
                    }
                    painter.rect_stroke(
                        cr,
                        CornerRadius::ZERO,
                        Stroke::new(1.5, accent),
                        StrokeKind::Inside,
                    );
                }
                _ => {}
            }
        }
    }

    /// Selection chrome + handles + text edit overlay.
    fn draw_overlay(&mut self, painter: &egui::Painter, img_rect: Rect, scale: f32) {
        self.img_rect = img_rect;
        let accent = theme::tokens(painter.ctx()).accent;
        // Live-drag / nudge overlay: the travelling annotation rendered
        // by the shared bounded pipeline (same pixels as the committed
        // render — z-order is the one documented approximation). Region
        // effects (blur) have no layered render → their live selection
        // chrome below frames the moved bounds.
        if self.live_anno().is_some() {
            if self.preview_dirty || self.preview_tex.is_none() {
                let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
                let anno = self.live_anno().cloned();
                let scratch = self
                    .region_scratch
                    .get_or_insert_with(|| render::DragPreview::new(iw, ih));
                self.preview_patch =
                    anno.as_ref()
                        .and_then(|a| scratch.render(a))
                        .map(|(patch, ox, oy)| {
                            let (pw, ph) = (patch.width(), patch.height());
                            let ci = egui::ColorImage::from_rgba_unmultiplied(
                                [pw as usize, ph as usize],
                                patch.as_raw(),
                            );
                            let tex = self.preview_tex.get_or_insert_with(|| {
                                painter.ctx().load_texture(
                                    "draw-live-preview",
                                    egui::ColorImage::filled([1, 1], Color32::TRANSPARENT),
                                    egui::TextureOptions::LINEAR,
                                )
                            });
                            tex.set(ci, egui::TextureOptions::LINEAR);
                            (ox, oy, pw, ph)
                        });
                self.preview_dirty = false;
            }
            if let (Some(tex), Some((ox, oy, pw, ph))) = (&self.preview_tex, self.preview_patch) {
                painter.image(
                    tex.id(),
                    Rect::from_min_max(
                        pos2(
                            img_rect.min.x + ox as f32 * scale,
                            img_rect.min.y + oy as f32 * scale,
                        ),
                        pos2(
                            img_rect.min.x + (ox + pw) as f32 * scale,
                            img_rect.min.y + (oy + ph) as f32 * scale,
                        ),
                    ),
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
        }
        // Eraser brush circle or eyedropper magnifier under the cursor.
        if self.tool == Tool::Eraser
            && let Some(p) = self.last_pointer
        {
            let cp = pos2(img_rect.min.x + p.x * scale, img_rect.min.y + p.y * scale);
            if self.opts.eraser_pick {
                self.paint_eyedropper_loupe(painter, cp, p);
            } else {
                let r = (self.opts.eraser_size * scale * 0.5).max(2.0);
                painter.circle_stroke(
                    cp,
                    r + 1.0,
                    Stroke::new(1.0, Color32::from_black_alpha(190)),
                );
                painter.circle_stroke(cp, r, Stroke::new(1.2, Color32::WHITE));
            }
        }
        // The Pipette tool — or a swatch's armed pick — gets the same
        // loupe as the capture overlay: zoomed pixels, crosshair, coords,
        // `#RRGGBB`, `C` hint.
        if (self.tool == Tool::Pipette || armed_pick(painter.ctx()).is_some())
            && let Some(p) = self.last_pointer
        {
            let cp = pos2(img_rect.min.x + p.x * scale, img_rect.min.y + p.y * scale);
            self.paint_eyedropper_loupe(painter, cp, p);
        }
        // The emoji tool rides the cursor with a translucent ghost at
        // the size a click would place — aim the real pixels first.
        // During a strip drag-out the hover position may be missing;
        // fall back to the global pointer position.
        // No resting ghost under the cursor — the only travelling
        // emoji is the strip drag-out's payload itself.
        if self.emoji_dnd.is_some()
            && let Some(pos) = painter.ctx().input(|i| i.pointer.latest_pos())
        {
            let p = pos2(
                (pos.x - img_rect.min.x) / scale,
                (pos.y - img_rect.min.y) / scale,
            );
            let (iw, ih) = (self.doc.base.image.width(), self.doc.base.image.height());
            let side = emoji_default_side(
                iw.min(ih) as f32,
                self.doc.base.scale_factor,
                self.opts.emoji_size,
            );
            let r = emoji_centered(p, side, iw, ih);
            let cr = Rect::from_min_max(
                pos2(
                    img_rect.min.x + r.x as f32 * scale,
                    img_rect.min.y + r.y as f32 * scale,
                ),
                pos2(
                    img_rect.min.x + (r.x + r.width as i32) as f32 * scale,
                    img_rect.min.y + (r.y + r.height as i32) as f32 * scale,
                ),
            );
            if let Some(tex) =
                Self::emoji_tex(&mut self.emoji_texs, painter.ctx(), self.opts.emoji_id)
            {
                painter.image(
                    tex,
                    cr,
                    Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                    Color32::from_white_alpha(140),
                );
            }
            painter.rect_stroke(
                cr,
                CornerRadius::ZERO,
                Stroke::new(1.0, accent),
                StrokeKind::Inside,
            );
        }
        // The live marquee frames its region above the document — with
        // 8 resize anchors once the initial framing drag is over.
        if let Some(sel) = self.sel_area {
            let r = Rect::from_min_max(
                pos2(
                    img_rect.min.x + sel.rect.min.x * scale,
                    img_rect.min.y + sel.rect.min.y * scale,
                ),
                pos2(
                    img_rect.min.x + sel.rect.max.x * scale,
                    img_rect.min.y + sel.rect.max.y * scale,
                ),
            );
            sel_chrome(painter, r, accent);
            if !matches!(self.drag, Some(Drag::Marquee { .. })) {
                for c in handle_centers(r) {
                    painter.rect_filled(c, CornerRadius::same(2), Color32::WHITE);
                    painter.rect_stroke(
                        c,
                        CornerRadius::same(2),
                        Stroke::new(1.0, accent),
                        StrokeKind::Inside,
                    );
                }
            }
        }
        let Some(id) = self.selected else { return };
        // While a box's floating editor is open its selection frame
        // stays hidden (matches the reference editor).
        if self
            .text_panel
            .as_ref()
            .is_some_and(|p| p.is_box && p.id == id)
        {
            return;
        }
        // Track the live copy during an object drag / arrow-nudge burst
        // so Bézier levers, green/grey anchors and resize handles follow
        // the gesture in real time.
        let Some(a) = self
            .live_anno()
            .filter(|a| a.id == id)
            .cloned()
            .or_else(|| self.doc.annotation(id).cloned())
        else {
            return;
        };
        let to_canvas = |p: Pos2| pos2(img_rect.min.x + p.x * scale, img_rect.min.y + p.y * scale);
        let b = a.kind.bounds();
        let cb = Rect::from_min_max(
            pos2(
                img_rect.min.x + b.x as f32 * scale,
                img_rect.min.y + b.y as f32 * scale,
            ),
            pos2(
                img_rect.min.x + (b.x + b.width as i32) as f32 * scale,
                img_rect.min.y + (b.y + b.height as i32) as f32 * scale,
            ),
        );
        let is_bezier = matches!(
            a.kind,
            AnnotationKind::Text(_) | AnnotationKind::Line(_) | AnnotationKind::Brace(_)
        );
        if !is_bezier {
            if matches!(
                &a.kind,
                AnnotationKind::HighlightArea(o) if o.shape != AreaShape::Rect
            ) {
                painter.add(egui::epaint::EllipseShape::stroke(
                    cb.center(),
                    cb.size() / 2.0,
                    Stroke::new(1.0, accent),
                ));
            } else {
                painter.rect_stroke(
                    cb,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, accent),
                    StrokeKind::Outside,
                );
            }
        }
        // Handles.
        match &a.kind {
            AnnotationKind::Line(o) => {
                paint_bezier_chrome(painter, &to_canvas, o.a, o.ka, o.kb, o.b, accent, false);
            }
            AnnotationKind::Text(o) => {
                paint_bezier_chrome(painter, &to_canvas, o.a, o.ka, o.kb, o.b, accent, true);
            }
            AnnotationKind::Brace(o) => {
                for q in [o.a, o.b] {
                    let c = to_canvas(q);
                    painter.circle_filled(c, 4.5, Color32::WHITE);
                    painter.circle_stroke(c, 4.5, Stroke::new(1.2, Color32::from_gray(60)));
                }
            }
            _ => {
                for c in handle_centers(cb) {
                    painter.rect_filled(c, CornerRadius::same(2), Color32::WHITE);
                    painter.rect_stroke(
                        c,
                        CornerRadius::same(2),
                        Stroke::new(1.0, accent),
                        StrokeKind::Inside,
                    );
                }
                if let AnnotationKind::TextBox(o) = &a.kind
                    && let Some(t) = o.tail
                {
                    painter.circle_filled(to_canvas(t), 4.0, Color32::from_rgb(60, 170, 80));
                }
            }
        }
    }

    /// Magnifier card for the eraser's eyedropper — identical to the
    /// capture overlay loupe (zoomed 21×21 pixels, crosshair, coords,
    /// `#RRGGBB` and `C` copy hint).
    fn paint_eyedropper_loupe(&mut self, painter: &egui::Painter, pos: Pos2, img_p: Pos2) {
        const LOUPE_SIDE: f32 = 126.0;
        const LOUPE_SAMPLE: i32 = 21;
        const LOUPE_GAP: f32 = 18.0;
        const LOUPE_CROSS: Color32 = Color32::from_rgb(240, 70, 70);

        let clip = painter.clip_rect();
        let text_h = 78.0_f32;
        let size = vec2(LOUPE_SIDE, LOUPE_SIDE + text_h);
        let mut min = pos + Vec2::splat(LOUPE_GAP);
        if min.x + size.x > clip.max.x - 8.0 {
            min.x = pos.x - LOUPE_GAP - size.x;
        }
        if min.y + size.y > clip.max.y - 8.0 {
            min.y = pos.y - LOUPE_GAP - size.y;
        }
        let card = Rect::from_min_size(min, size);
        let zoom_rect = Rect::from_min_size(card.min, Vec2::splat(LOUPE_SIDE));
        let text_rect = Rect::from_min_max(pos2(card.min.x, zoom_rect.max.y), card.max);
        painter.rect_filled(card, 8.0, Color32::from_black_alpha(235));
        painter.rect_stroke(
            card,
            8.0,
            Stroke::new(1.0, Color32::from_gray(70)),
            StrokeKind::Outside,
        );

        if let Some(img) = &self.rendered_img {
            let (w, h) = (img.width() as i32, img.height() as i32);
            let (cx, cy) = (img_p.x as i32, img_p.y as i32);
            let half = LOUPE_SAMPLE / 2;
            let mut sample = Vec::with_capacity((LOUPE_SAMPLE * LOUPE_SAMPLE * 4) as usize);
            for dy in -half..=half {
                for dx in -half..=half {
                    let (x, y) = (cx + dx, cy + dy);
                    let px = if x >= 0 && y >= 0 && x < w && y < h {
                        img.get_pixel(x as u32, y as u32).0
                    } else {
                        [0, 0, 0, 255]
                    };
                    sample.extend_from_slice(&px);
                }
            }
            let zoomed = ColorImage::from_rgba_unmultiplied(
                [LOUPE_SAMPLE as usize, LOUPE_SAMPLE as usize],
                &sample,
            );
            let ctx = painter.ctx().clone();
            let tex = self.loupe_tex.get_or_insert_with(|| {
                ctx.load_texture(
                    "draw.loupe",
                    ColorImage::filled(
                        [LOUPE_SAMPLE as usize, LOUPE_SAMPLE as usize],
                        Color32::BLACK,
                    ),
                    TextureOptions::NEAREST,
                )
            });
            tex.set(zoomed, TextureOptions::NEAREST);
            painter.image(
                tex.id(),
                zoom_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            let centre = zoom_rect.center();
            painter.hline(zoom_rect.x_range(), centre.y, Stroke::new(1.0, LOUPE_CROSS));
            painter.vline(centre.x, zoom_rect.y_range(), Stroke::new(1.0, LOUPE_CROSS));
            painter.hline(
                card.x_range(),
                text_rect.min.y,
                Stroke::new(1.0, Color32::from_gray(70)),
            );
        }

        let c = self.pixel_at(img_p);
        let hex = format!("#{:02X}{:02X}{:02X}", c.r(), c.g(), c.b());
        let font = egui::FontId::monospace(13.0);
        let centre_x = text_rect.center().x;
        let mut y = text_rect.min.y + 14.0;
        let hint = if self
            .copied_at
            .is_some_and(|t| t.elapsed().as_secs_f32() < 1.2)
        {
            t!("overlay.copied").to_string()
        } else {
            t!("overlay.copy_hint").to_string()
        };
        for (text, color) in [
            (format!("{:.0}, {:.0}", img_p.x, img_p.y), Color32::WHITE),
            (hex, Color32::WHITE),
            (hint, Color32::from_gray(160)),
        ] {
            painter.text(
                pos2(centre_x, y),
                egui::Align2::CENTER_TOP,
                text,
                font.clone(),
                color,
            );
            y += 20.0;
        }
    }

    /// Closes the text panel: one Replace for the whole edit session.
    /// An empty plain-text object is removed, whereas a rectangular or
    /// oval box without text is valid and stays on the canvas. Leaving
    /// the panel also clears the selection chrome.
    fn close_text_panel(&mut self) {
        let Some(panel) = self.text_panel.take() else {
            return;
        };
        let Some(after) = self.doc.annotation(panel.id).cloned() else {
            return;
        };
        let empty = match &after.kind {
            AnnotationKind::Text(o) => o.content.trim().is_empty(),
            AnnotationKind::TextBox(_) => false,
            _ => true,
        };
        if empty {
            if let Some(idx) = self.doc.index_of(panel.id) {
                self.apply(AnnoEdit::Remove {
                    anno: after,
                    index: idx,
                });
            }
            self.selected = None;
            return;
        }
        if format!("{:?}", panel.before) != format!("{:?}", after) {
            self.apply(AnnoEdit::Replace {
                before: panel.before,
                after,
            });
        }
        self.selected = None;
    }

    /// Opens the Texte panel on `id` when it's a text object.
    fn open_text_panel(&mut self, id: AnnotationId) {
        let Some(a) = self.doc.annotation(id) else {
            return;
        };
        let is_box = matches!(a.kind, AnnotationKind::TextBox(_));
        if matches!(a.kind, AnnotationKind::Text(_) | AnnotationKind::TextBox(_)) {
            self.text_panel = Some(TextPanel {
                id,
                is_box,
                before: a.clone(),
                focus_pending: true,
            });
        }
    }

    /// Keeps the panel glued to the selection: selecting another object
    /// commits it, selecting a text object opens it.
    fn sync_text_panel(&mut self) {
        // Single-clicking selects without opening the panel; only a
        // double-click (or new creation) opens it. Moving the selection
        // away closes an open panel.
        if let Some(pid) = self.text_panel.as_ref().map(|p| p.id)
            && self.selected != Some(pid)
        {
            self.close_text_panel();
        }
    }

    /// The floating "Texte" window — fields mirror the reference:
    /// colour, outline `*`, font, size, B/I, alignment, outline
    /// width+colour, and the multi-line edit with its Enter note.
    /// Edits land on the object live; closing commits one Replace.
    fn text_panel_window(&mut self, ctx: &egui::Context, shared: &Shared) {
        let Some(panel) = &self.text_panel else {
            return;
        };
        let id = panel.id;
        // Default position: right below the object's baseline on screen;
        // when there is not enough room below, open above it — never on
        // top of the object.
        let mut open = true;
        let panel_h = 150.0;
        let panel_w = if panel.is_box { 560.0 } else { 420.0 };
        let outer_h = panel_h + 34.0;
        let outer_w = panel_w + 16.0;
        let screen = ctx.content_rect();
        let default_pos = self
            .doc
            .annotation(id)
            .map(|a| {
                let b = a.kind.bounds();
                let left = self.img_rect.min.x + b.x as f32 * self.canvas_scale;
                let right = self.img_rect.min.x + (b.x + b.width as i32) as f32 * self.canvas_scale;
                let top = self.img_rect.min.y + b.y as f32 * self.canvas_scale;
                let bottom =
                    self.img_rect.min.y + (b.y + b.height as i32) as f32 * self.canvas_scale;
                let gap = if panel.is_box { 10.0 } else { 24.0 };
                let x_aligned =
                    left.clamp(screen.min.x + 8.0, (screen.max.x - outer_w - 8.0).max(8.0));
                let below = bottom + gap;
                let above = top - outer_h - gap;
                if below + outer_h <= screen.max.y {
                    pos2(x_aligned, below)
                } else if above >= screen.min.y + 4.0 {
                    pos2(x_aligned, above)
                } else if right + 12.0 + outer_w <= screen.max.x {
                    pos2(
                        right + 12.0,
                        top.clamp(screen.min.y + 8.0, (screen.max.y - outer_h - 8.0).max(8.0)),
                    )
                } else if left - 12.0 - outer_w >= screen.min.x {
                    pos2(
                        left - outer_w - 12.0,
                        top.clamp(screen.min.y + 8.0, (screen.max.y - outer_h - 8.0).max(8.0)),
                    )
                } else if screen.max.y - bottom >= top - screen.min.y {
                    pos2(x_aligned, (screen.max.y - outer_h - 4.0).max(8.0))
                } else {
                    pos2(x_aligned, screen.min.y + 8.0)
                }
            })
            .unwrap_or(pos2(120.0, 220.0));
        let mut focus_first = self
            .text_panel
            .as_mut()
            .map(|p| std::mem::take(&mut p.focus_pending))
            .unwrap_or(false);
        let mut persist: Option<(Color32, String, u8, bool, bool, u8, Color32)> = None;
        let mut win = egui::Window::new(t!("draw.text.title").to_string())
            .id(egui::Id::new(("draw.text.win", id)))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_size(vec2(panel_w, panel_h))
            .fixed_size(vec2(panel_w, panel_h))
            .default_pos(default_pos);
        if focus_first {
            win = win.current_pos(default_pos);
        }
        win.show(ctx, |ui| {
            let Some(a) = self.doc.annotation_mut(id) else {
                return;
            };
            let mut rev = false;
            let (
                content,
                size,
                bold,
                italic,
                color,
                mut font,
                mut outline,
                mut outline_color,
                mut balign,
                mut bvalign,
            ) = match &mut a.kind {
                AnnotationKind::Text(o) => (
                    &mut o.content,
                    &mut o.size,
                    &mut o.bold,
                    &mut o.italic,
                    &mut o.color,
                    Some(&mut o.font),
                    Some(&mut o.outline),
                    Some(&mut o.outline_color),
                    None,
                    None,
                ),
                AnnotationKind::TextBox(o) => (
                    &mut o.content,
                    &mut o.size,
                    &mut o.bold,
                    &mut o.italic,
                    &mut o.fg,
                    None,
                    Some(&mut o.border_w),
                    Some(&mut o.border),
                    Some(&mut o.align),
                    Some(&mut o.valign),
                ),
                _ => return,
            };
            let mut style_changed = false;
            // 1) Couleur · 2) Fonte · 3) Taille (menu 8..72) ·
            // 4) Bold · 5) Italic · 6) Contour (0..10) · 7) Couleur contour.
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 24.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    if color_btn(ui, color).changed() {
                        rev = true;
                        style_changed = true;
                    }
                    if let Some(font) = &mut font {
                        let prev = (**font).clone();
                        egui::ComboBox::from_id_salt("draw.tp.font")
                            .width(120.0)
                            .selected_text(prev.clone())
                            .show_ui(ui, |ui| {
                                for f in crate::editor::render::FONT_FAMILIES {
                                    ui.selectable_value(*font, (*f).to_owned(), *f);
                                }
                            });
                        if **font != prev {
                            rev = true;
                            style_changed = true;
                        }
                    }
                    if font_size_combo(ui, "draw.tp.size", size) {
                        rev = true;
                        style_changed = true;
                    }
                    if ui.toggle_value(bold, "B").clicked() {
                        rev = true;
                        style_changed = true;
                    }
                    if ui.toggle_value(italic, "I").clicked() {
                        rev = true;
                        style_changed = true;
                    }
                    // Boxes only: justification + vertical alignment
                    // icons on the same row, right after Italic.
                    if let (Some(al), Some(va)) = (&mut balign, &mut bvalign) {
                        ui.separator();
                        for (v, ic, tip) in [
                            (TextAlign::Left, Icon::AlignLeft, t!("draw.opt.align_l")),
                            (TextAlign::Center, Icon::AlignCenter, t!("draw.opt.align_c")),
                            (TextAlign::Right, Icon::AlignRight, t!("draw.opt.align_r")),
                            (
                                TextAlign::Justify,
                                Icon::AlignJustify,
                                t!("draw.opt.align_j"),
                            ),
                        ] {
                            if widgets::icon_button(ui, ic, **al == v)
                                .on_hover_text(tip)
                                .clicked()
                            {
                                **al = v;
                                rev = true;
                            }
                        }
                        ui.separator();
                        for (v, ic, tip) in [
                            (
                                crate::editor::document::BoxVAlign::Top,
                                Icon::AlignStartVertical,
                                t!("draw.opt.valign_top"),
                            ),
                            (
                                crate::editor::document::BoxVAlign::Center,
                                Icon::AlignCenterVertical,
                                t!("draw.opt.valign_center"),
                            ),
                            (
                                crate::editor::document::BoxVAlign::Bottom,
                                Icon::AlignEndVertical,
                                t!("draw.opt.valign_bottom"),
                            ),
                        ] {
                            if widgets::icon_button(ui, ic, **va == v)
                                .on_hover_text(tip)
                                .clicked()
                            {
                                **va = v;
                                rev = true;
                            }
                        }
                        ui.separator();
                    }
                    if let Some(outline) = &mut outline
                        && outline_combo(ui, "draw.tp.outline", outline)
                    {
                        rev = true;
                        style_changed = true;
                    }
                    if let Some(oc) = &mut outline_color
                        && color_btn(ui, oc).changed()
                    {
                        rev = true;
                        style_changed = true;
                    }
                },
            );
            let r = egui::TextEdit::multiline(content)
                .id(egui::Id::new(("draw.text.edit", id)))
                .desired_width(f32::INFINITY)
                .desired_rows(4)
                .show(ui);
            if focus_first {
                r.response.request_focus();
                focus_first = false;
            }
            if r.response.changed() {
                rev = true;
            }
            ui.label(t!("draw.text.note"));
            if style_changed && let AnnotationKind::Text(o) = &a.kind {
                persist = Some((
                    o.color,
                    o.font.clone(),
                    o.size.round().clamp(8.0, 72.0) as u8,
                    o.bold,
                    o.italic,
                    o.outline.round().clamp(0.0, 10.0) as u8,
                    o.outline_color,
                ));
            }
            if rev {
                if let AnnotationKind::Text(o) = &mut a.kind {
                    let (nw, _) = render::text_extent(o);
                    let len = (o.b - o.a).length();
                    if len < nw {
                        let d = unit_dir(o.a, o.b);
                        o.b = pos2(o.a.x + d.x * nw, o.a.y + d.y * nw);
                        let (ra, rb) = text_knob_rest(o);
                        if seg_distance(o.ka, o.a, o.b) < 3.0 {
                            o.ka = ra;
                        }
                        if seg_distance(o.kb, o.a, o.b) < 3.0 {
                            o.kb = rb;
                        }
                    }
                }
                self.doc.revision += 1;
            }
        });
        if let Some((c, font, sz, b, it, out, oc)) = persist {
            self.save_text_prefs(shared, c, font, sz, b, it, out, oc);
        }
        if !open {
            self.close_text_panel();
        }
    }

    /// Mirrors text style changes into `self.opts` + `prefs.json`.
    #[allow(clippy::too_many_arguments)]
    fn save_text_prefs(
        &mut self,
        shared: &Shared,
        color: Color32,
        font: String,
        size: u8,
        bold: bool,
        italic: bool,
        outline: u8,
        outline_color: Color32,
    ) {
        self.opts.color = color;
        self.opts.font = font.clone();
        self.opts.font_size = f32::from(size);
        self.opts.bold = bold;
        self.opts.italic = italic;
        self.opts.outline_w = f32::from(outline);
        self.opts.outline_color = outline_color;
        let mut s = shared.lock_settings();
        s.text_color = [color.r(), color.g(), color.b()];
        s.text_font = font;
        s.text_size = size;
        s.text_bold = bold;
        s.text_italic = italic;
        s.text_outline = outline;
        s.text_outline_color = [outline_color.r(), outline_color.g(), outline_color.b()];
        s.save();
    }

    /// Title of the bottom tab: the selected object's kind when one is
    /// selected ("Texte", …), otherwise the armed tool's name.
    fn bottom_tab_title(&self) -> std::borrow::Cow<'static, str> {
        // A live marquee owns the tab — its ops act on pixels, not an
        // object. The frame's W×H rides the title and stays in sync
        // while its anchors resize it.
        if let Some(sel) = self.sel_area {
            return format!(
                "{} {}×{}",
                t!("draw.sel.title"),
                sel.rect.width().round() as u32,
                sel.rect.height().round() as u32
            )
            .into();
        }
        if let Some(id) = self.selected
            && let Some(a) = self.doc.annotation(id)
        {
            return match &a.kind {
                AnnotationKind::Text(_) => t!("draw.text.title"),
                AnnotationKind::TextBox(o) => {
                    if o.oval {
                        t!("draw.tool.textbox_oval")
                    } else {
                        t!("draw.tool.textbox_rect")
                    }
                }
                AnnotationKind::Line(_) => t!("draw.tool.arrow"),
                AnnotationKind::Freehand(_) => t!("draw.tool.pencil"),
                AnnotationKind::HighlightArea(_) => t!("draw.tool.harea"),
                AnnotationKind::Rect(o) => {
                    if o.fill.is_some() {
                        t!("draw.tool.rectfill")
                    } else {
                        t!("draw.tool.rect")
                    }
                }
                AnnotationKind::Ellipse(o) => {
                    if o.fill.is_some() {
                        t!("draw.tool.ellipsefill")
                    } else {
                        t!("draw.tool.ellipse")
                    }
                }
                AnnotationKind::Step(_) => t!("draw.tool.step"),
                AnnotationKind::Picture(_) => t!("draw.tab.image"),
                AnnotationKind::Blur(_) => t!("draw.tool.blur"),
                AnnotationKind::Brace(_) => t!("draw.brace.title"),
                AnnotationKind::Emoji(_) => t!("draw.tool.emoji"),
            };
        }
        if self.tool == Tool::Brace {
            return t!("draw.brace.title");
        }
        self.tool.tip()
    }

    /// Bottom bar: rendered as a tab (header chip with the object/tool
    /// type + content strip) + OK / Annuler on the right.
    fn options_bar(&mut self, ui: &mut Ui, shared: &Shared) {
        let tk = theme::tokens(ui.ctx());
        let title = self.bottom_tab_title();
        let sel_step_mode = self
            .selected
            .and_then(|id| self.doc.annotation(id))
            .and_then(|a| match &a.kind {
                AnnotationKind::Step(o) => Some(o.mode),
                _ => None,
            });
        let step_bar = sel_step_mode.is_some()
            || (self.sel_area.is_none()
                && self.selected.is_none()
                && matches!(self.tool, Tool::Step));
        // Emoji tabs too: tool armed, or the selection is an emoji.
        let sel_is_emoji = self
            .selected
            .and_then(|id| self.doc.annotation(id))
            .is_some_and(|a| matches!(a.kind, AnnotationKind::Emoji(_)));
        let emoji_bar = sel_is_emoji
            || (self.sel_area.is_none()
                && self.selected.is_none()
                && matches!(self.tool, Tool::Emoji));
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // Air above the tab chips — the panel's top edge must not
            // sit right on them.
            ui.add_space(4.0);
            // Tab header chip (two clickable mode tabs for the step marker);
            // OK/Annuler stay pinned to its right edge, always visible.
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                if step_bar {
                    let cur_mode = sel_step_mode.unwrap_or(self.opts.step_mode);
                    for (mode, key) in [
                        (StepMode::Numbers, "draw.step.tab_num"),
                        (StepMode::Letters, "draw.step.tab_let"),
                    ] {
                        let on = cur_mode == mode;
                        let label = if on {
                            format!("● {}", t!(key))
                        } else {
                            t!(key).to_string()
                        };
                        let inner = egui::Frame::NONE
                            .fill(if on { tk.field } else { tk.dock })
                            .stroke(Stroke::new(
                                if on { 1.5 } else { 1.0 },
                                if on { tk.accent } else { tk.field_border },
                            ))
                            .corner_radius(CornerRadius {
                                nw: 4,
                                ne: 4,
                                sw: 0,
                                se: 0,
                            })
                            .inner_margin(egui::Margin::symmetric(10, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(label).size(11.5).strong().color(if on {
                                        tk.text
                                    } else {
                                        tk.text_dim
                                    }),
                                );
                            });
                        let resp = ui.interact(
                            inner.response.rect,
                            ui.id().with(("pixme-step-tab", mode as u8)),
                            egui::Sense::click(),
                        );
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if resp.clicked() && !on {
                            self.opts.step_mode = mode;
                            if let Some(id) = self.selected
                                && let Some(before) = self.doc.annotation(id).cloned()
                                && let Some(a) = self.doc.annotation_mut(id)
                                && let AnnotationKind::Step(o) = &mut a.kind
                            {
                                o.mode = mode;
                                let after = a.clone();
                                self.apply(AnnoEdit::Replace { before, after });
                            }
                        }
                    }
                } else if emoji_bar {
                    // One tab per manifest section — same chip chrome
                    // as the Step tabs.
                    for (i, sec) in crate::editor::emoji::sections().iter().enumerate() {
                        let on = self.emoji_section == i;
                        let label = emoji_section_label(sec);
                        let inner = egui::Frame::NONE
                            .fill(if on { tk.field } else { tk.dock })
                            .stroke(Stroke::new(
                                if on { 1.5 } else { 1.0 },
                                if on { tk.accent } else { tk.field_border },
                            ))
                            .corner_radius(CornerRadius {
                                nw: 4,
                                ne: 4,
                                sw: 0,
                                se: 0,
                            })
                            .inner_margin(egui::Margin::symmetric(10, 2))
                            .show(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(label.as_ref())
                                        .size(11.5)
                                        .strong()
                                        .color(if on { tk.text } else { tk.text_dim }),
                                );
                            });
                        let resp = ui.interact(
                            inner.response.rect,
                            ui.id().with(("pixme-emoji-tab", i)),
                            egui::Sense::click(),
                        );
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if resp.clicked() && !on {
                            self.emoji_section = i;
                        }
                    }
                } else {
                    egui::Frame::NONE
                        .fill(tk.field)
                        .stroke(Stroke::new(1.0, tk.field_border))
                        .corner_radius(CornerRadius {
                            nw: 4,
                            ne: 4,
                            sw: 0,
                            se: 0,
                        })
                        .inner_margin(egui::Margin::symmetric(10, 2))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(title.as_ref())
                                    .size(11.5)
                                    .strong()
                                    .color(tk.text),
                            );
                        });
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(6.0);
                    if widgets::primary_button(ui, &t!("draw.ok")).clicked() {
                        self.finalize_pending();
                        self.outcome = Some(Outcome::Ok);
                    }
                    if widgets::secondary_button(ui, &t!("draw.cancel")).clicked() {
                        self.request_cancel();
                    }
                });
            });
            // Tab body: a single options line that scrolls horizontally
            // on the mouse wheel when it overflows — no visible scrollbar.
            ui.style_mut().always_scroll_the_only_direction = true;
            egui::ScrollArea::horizontal()
                .id_salt("draw.opts.scroll")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().interact_size.y = 22.0;
                        ui.add_space(8.0);
                        self.bottom_content(ui, shared);
                    });
                });
        });
    }

    /// Content of the bottom tab: a live marquee shows its pixel ops,
    /// a selected object its properties (regardless of the armed tool);
    /// otherwise the armed tool's defaults.
    fn bottom_content(&mut self, ui: &mut Ui, shared: &Shared) {
        if self.sel_area.is_some() {
            self.sel_options(ui);
        } else if self.selected.is_some() {
            // A selected emoji keeps the strip up (icons only — no
            // styling options) so the user can arm another one fast.
            if matches!(
                self.selected
                    .and_then(|id| self.doc.annotation(id))
                    .map(|a| &a.kind),
                Some(AnnotationKind::Emoji(_))
            ) {
                self.emoji_row(ui);
            } else {
                self.selected_options(ui, shared);
            }
        } else {
            self.tool_options(ui, shared);
        }
    }

    /// The marquee's option strip (frame W×H rides the tab title):
    /// blur presets, cut/copy/paste/delete, and a Remplissage button
    /// applying the swatch's colour — the swatch itself just picks it.
    fn sel_options(&mut self, ui: &mut Ui) {
        let Some(_sel) = self.sel_area else { return };
        ui.menu_button(t!("draw.opt.blur"), |ui| {
            for pct in [25u8, 50, 75, 100] {
                if ui.button(format!("{pct}%")).clicked() {
                    self.sel_blur(pct);
                    ui.close();
                }
            }
        });
        if ui.button(t!("draw.sel.cut")).clicked() {
            self.sel_cut();
        }
        if ui.button(t!("draw.sel.copy")).clicked() {
            self.sel_copy();
        }
        if ui.button(t!("draw.sel.paste")).clicked() {
            self.sel_paste();
        }
        if ui.button(t!("draw.sel.delete")).clicked() {
            self.sel_delete();
        }
        ui.separator();
        let do_fill = ui
            .horizontal(|ui| {
                color_btn(ui, &mut self.opts.fill_color);
                ui.button(t!("draw.opt.fill_c")).clicked()
            })
            .inner;
        if do_fill {
            let c = self.opts.fill_color;
            self.sel_fill(c);
        }
    }

    /// The emoji cell row for the ACTIVE section tab (the tab chips
    /// live in the bar header). Click a cell to arm it (accent border);
    /// DRAG one onto the canvas to drop it where it lands. Icons only —
    /// no styling options by design.
    fn emoji_row(&mut self, ui: &mut Ui) {
        let sections = crate::editor::emoji::sections();
        let cur = sections.get(self.emoji_section).copied().unwrap_or("");
        for def in crate::editor::emoji::all() {
            if def.section != cur {
                continue;
            }
            let Some(tex) = Self::emoji_tex(&mut self.emoji_texs, ui.ctx(), def.id) else {
                continue;
            };
            let resp = emoji_cell(ui, tex, 26.0, def.id == self.opts.emoji_id);
            if resp.drag_started_by(egui::PointerButton::Primary) {
                self.opts.emoji_id = def.id;
                self.emoji_dnd = Some(def.id);
            } else if resp.clicked() {
                self.opts.emoji_id = def.id;
            }
        }
    }

    /// Contextual options — one set per tool, matching the reference's
    /// bottom bar fields (labels kept short; full names in tooltips).
    fn tool_options(&mut self, ui: &mut Ui, shared: &Shared) {
        let o = &mut self.opts;
        match self.tool {
            Tool::Select => {
                ui.label(t!("draw.opt.select_hint"));
                if let Some(id) = self.selected
                    && let Some(a) = self.doc.annotation(id)
                {
                    {
                        let b = a.kind.bounds();
                        ui.horizontal(|ui| {
                            ui.separator();
                            ui.label(
                                t!(
                                    "draw.opt.bounds",
                                    x = b.x,
                                    y = b.y,
                                    w = b.width,
                                    h = b.height
                                )
                                .to_string(),
                            );
                        });
                    }
                }
            }
            Tool::Arrow => {
                line_options_row(
                    ui,
                    "draw.bar.line",
                    &mut o.color,
                    &mut o.width,
                    &mut o.line_start,
                    &mut o.line_end,
                    &mut o.dash,
                    &mut o.outline,
                    &mut o.line_outline_color,
                    &mut o.opacity,
                    &mut o.line_shadow,
                );
            }
            Tool::Pencil => {
                pencil_options_row(
                    ui,
                    "draw.bar.pencil",
                    &mut o.color,
                    &mut o.width,
                    &mut o.tip,
                    &mut o.dash,
                    &mut o.outline,
                    &mut o.line_outline_color,
                    &mut o.opacity,
                    &mut o.line_shadow,
                );
            }
            Tool::Brace => {
                brace_options_row(
                    ui,
                    "draw.bar.brace",
                    &mut o.color,
                    &mut o.width,
                    &mut o.brace_kind,
                    &mut o.dash,
                    &mut o.outline,
                    &mut o.line_outline_color,
                    &mut o.opacity,
                    &mut o.line_shadow,
                );
            }
            Tool::Eraser => {
                ui.horizontal(|ui| {
                    ui.label(t!("draw.opt.color"));
                    if color_btn(ui, &mut o.eraser_color).changed() {
                        o.eraser_mode = EraserMode::Color;
                        o.eraser_pick = false;
                    }
                });
                w_edit(ui, t!("draw.opt.size"), &mut o.eraser_size, 1.0..=80.0);

                ui.horizontal(|ui| {
                    ui.label(t!("draw.opt.blur"));
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        let blur_on = o.eraser_mode == EraserMode::Blur;
                        if widgets::icon_button(ui, Icon::Blur, blur_on)
                            .on_hover_text(t!("draw.opt.blur").to_string())
                            .clicked()
                        {
                            o.eraser_mode = if blur_on {
                                EraserMode::Color
                            } else {
                                EraserMode::Blur
                            };
                            o.eraser_pick = false;
                        }
                        let pix_on = o.eraser_mode == EraserMode::Pixelate;
                        if widgets::icon_button(ui, Icon::Pixelate, pix_on)
                            .on_hover_text(t!("draw.opt.pixelate").to_string())
                            .clicked()
                        {
                            o.eraser_mode = if pix_on {
                                EraserMode::Color
                            } else {
                                EraserMode::Pixelate
                            };
                            o.eraser_pick = false;
                        }
                    });
                });
            }
            Tool::Rect | Tool::RectFill | Tool::Ellipse | Tool::EllipseFill => {
                let is_rect = matches!(self.tool, Tool::Rect | Tool::RectFill);
                let is_filled = matches!(self.tool, Tool::RectFill | Tool::EllipseFill);
                let mut fill_opt = is_filled.then_some(o.fill_color);
                let mut radius_opt = is_rect.then_some(&mut o.radius);
                shape_options_row(
                    ui,
                    "draw.bar.shape",
                    &mut o.color,
                    &mut o.width,
                    &mut fill_opt,
                    &mut radius_opt,
                    &mut o.dash,
                    &mut o.outline,
                    &mut o.line_outline_color,
                    &mut o.opacity,
                    &mut o.line_shadow,
                );
                if let Some(fc) = fill_opt {
                    o.fill_color = fc;
                }
            }
            Tool::HighlightArea => {
                highlight_area_options_row(
                    ui,
                    "draw.bar.harea",
                    &mut o.color,
                    &mut o.shape,
                    &mut o.hl_opacity,
                );
            }
            Tool::Blur => {
                blur_options_row(ui, &mut o.blur_style, &mut o.blur_strength, &mut o.shape);
            }
            Tool::Pipette => {
                ui.label(t!("draw.opt.pipette_hint"));
            }
            Tool::Text => {
                let mut ch = false;
                ch |= ui
                    .horizontal(|ui| {
                        ui.label(t!("draw.opt.color"));
                        color_btn(ui, &mut o.color).changed()
                    })
                    .inner;
                ch |= ui
                    .horizontal(|ui| {
                        ui.label(t!("draw.opt.size"));
                        font_size_combo(ui, "draw.bar.tsize", &mut o.font_size)
                    })
                    .inner;
                ch |= ui
                    .toggle_value(&mut o.bold, "B")
                    .on_hover_text(t!("draw.opt.bold"))
                    .changed();
                ch |= ui
                    .toggle_value(&mut o.italic, "I")
                    .on_hover_text(t!("draw.opt.italic"))
                    .changed();
                ch |= ui
                    .horizontal(|ui| {
                        ui.label(t!("draw.opt.outline"));
                        outline_combo(ui, "draw.bar.toutline", &mut o.outline_w)
                            | color_btn(ui, &mut o.outline_color).changed()
                    })
                    .inner;
                ch |= opacity_pct_slider(ui, &mut o.opacity);
                let mut sh_on = o.shadow > 0.0;
                if ui.checkbox(&mut sh_on, t!("draw.opt.shadow")).changed() {
                    o.shadow = if sh_on { 3.0 } else { 0.0 };
                }
                if ch {
                    let (c, f, sz, b, it, out, oc) = (
                        o.color,
                        o.font.clone(),
                        o.font_size.round().clamp(8.0, 72.0) as u8,
                        o.bold,
                        o.italic,
                        o.outline_w.round().clamp(0.0, 10.0) as u8,
                        o.outline_color,
                    );
                    self.save_text_prefs(shared, c, f, sz, b, it, out, oc);
                }
            }
            Tool::TextBoxRect | Tool::TextBoxOval => {
                // Text colour + box interior.
                ui.horizontal(|ui| {
                    ui.label(t!("draw.opt.fg"));
                    color_btn(ui, &mut o.outline_color);
                });
                ui.horizontal(|ui| {
                    ui.label(t!("draw.opt.bg"));
                    color_btn(ui, &mut o.fill_color);
                });
                // Outline: width (1..10) + colour.
                ui.horizontal(|ui| {
                    ui.label(t!("draw.opt.outline"));
                    outline_combo(ui, "draw.bar.boxw", &mut o.width);
                    color_btn(ui, &mut o.color);
                });
                ui.add_space(12.0);
                opacity_pct_slider(ui, &mut o.opacity);
            }
            Tool::Step => {
                let count = match o.step_mode {
                    StepMode::Numbers => &mut o.step_next_num,
                    StepMode::Letters => &mut o.step_next_let,
                };
                step_options_row(
                    ui,
                    "draw.bar.step",
                    o.step_mode,
                    count,
                    &mut o.step_lower,
                    &mut o.step_bg,
                    &mut o.step_fg,
                    &mut o.step_style,
                    &mut o.step_size,
                    &mut o.step_ring,
                    &mut o.step_ring_color,
                    &mut o.opacity,
                    &mut o.step_shadow,
                );
            }
            Tool::Emoji => self.emoji_row(ui),
            Tool::SelectArea => {
                ui.label(t!("draw.sel.hint"));
            }
        }
    }

    /// Style options of the SELECTED object (re-editable, FS-style).
    fn selected_options(&mut self, ui: &mut Ui, shared: &Shared) {
        let Some(id) = self.selected else { return };
        let Some(before) = self.doc.annotation(id).cloned() else {
            return;
        };
        let mut changed = false;
        let mut persist: Option<(Color32, String, u8, bool, bool, u8, Color32)> = None;
        if let Some(a) = self.doc.annotation_mut(id) {
            match &mut a.kind {
                AnnotationKind::Rect(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    let mut radius_opt = Some(&mut o.radius);
                    if shape_options_row(
                        ui,
                        "draw.sel.rect",
                        &mut o.stroke,
                        &mut o.stroke_w,
                        &mut o.fill,
                        &mut radius_opt,
                        &mut o.dash,
                        &mut o.outline,
                        &mut o.outline_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Ellipse(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    let mut radius_opt: Option<&mut f32> = None;
                    if shape_options_row(
                        ui,
                        "draw.sel.ellipse",
                        &mut o.stroke,
                        &mut o.stroke_w,
                        &mut o.fill,
                        &mut radius_opt,
                        &mut o.dash,
                        &mut o.outline,
                        &mut o.outline_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Line(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    if line_options_row(
                        ui,
                        "draw.sel.line",
                        &mut o.color,
                        &mut o.width,
                        &mut o.start,
                        &mut o.end,
                        &mut o.dash,
                        &mut o.outline,
                        &mut o.outline_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Freehand(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    if pencil_options_row(
                        ui,
                        "draw.sel.pencil",
                        &mut o.color,
                        &mut o.width,
                        &mut o.tip,
                        &mut o.dash,
                        &mut o.outline,
                        &mut o.outline_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::HighlightArea(o) => {
                    if highlight_area_options_row(
                        ui,
                        "draw.sel.harea",
                        &mut o.color,
                        &mut o.shape,
                        &mut o.opacity,
                    ) {
                        self.opts.color = o.color;
                        self.opts.shape = o.shape;
                        self.opts.hl_opacity = o.opacity;
                        changed = true;
                    }
                }
                AnnotationKind::Text(o) => {
                    // 1) Couleurs · 2) Taille (8..72) · 3) Bold · 4) Italic ·
                    // 5) Contour (0..10) · 6) Couleur contour · 7) Opacité ·
                    // 8) Case Ombre (9=OK, 10=Annuler are on the bar's right).
                    let mut style_ch = false;
                    let mut sc = ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.color"));
                            color_btn(ui, &mut o.color).changed()
                        })
                        .inner;
                    changed |= sc;
                    style_ch |= sc;
                    sc = ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.size"));
                            font_size_combo(ui, "draw.sel.tsize", &mut o.size)
                        })
                        .inner;
                    changed |= sc;
                    style_ch |= sc;
                    if ui
                        .toggle_value(&mut o.bold, "B")
                        .on_hover_text(t!("draw.opt.bold"))
                        .changed()
                    {
                        changed = true;
                        style_ch = true;
                    }
                    if ui
                        .toggle_value(&mut o.italic, "I")
                        .on_hover_text(t!("draw.opt.italic"))
                        .changed()
                    {
                        changed = true;
                        style_ch = true;
                    }
                    sc = ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.outline"));
                            outline_combo(ui, "draw.sel.toutline", &mut o.outline)
                                | color_btn(ui, &mut o.outline_color).changed()
                        })
                        .inner;
                    changed |= sc;
                    style_ch |= sc;
                    changed |= opacity_pct_slider(ui, &mut o.opacity);
                    let mut sh_on = o.shadow > 0.0;
                    if ui.checkbox(&mut sh_on, t!("draw.opt.shadow")).changed() {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                    if style_ch {
                        persist = Some((
                            o.color,
                            o.font.clone(),
                            o.size.round().clamp(8.0, 72.0) as u8,
                            o.bold,
                            o.italic,
                            o.outline.round().clamp(0.0, 10.0) as u8,
                            o.outline_color,
                        ));
                    }
                }
                AnnotationKind::TextBox(o) => {
                    changed |= ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.fg"));
                            color_btn(ui, &mut o.fg).changed()
                        })
                        .inner;
                    changed |= ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.bg"));
                            color_btn(ui, &mut o.bg).changed()
                        })
                        .inner;
                    changed |= ui
                        .horizontal(|ui| {
                            ui.label(t!("draw.opt.outline"));
                            outline_combo(ui, "draw.sel.boxw", &mut o.border_w)
                                | color_btn(ui, &mut o.border).changed()
                        })
                        .inner;
                    ui.add_space(12.0);
                    changed |= opacity_pct_slider(ui, &mut o.opacity);
                }
                AnnotationKind::Step(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    if step_options_row(
                        ui,
                        "draw.sel.step",
                        o.mode,
                        &mut o.count,
                        &mut o.lower,
                        &mut o.bg,
                        &mut o.fg,
                        &mut o.style,
                        &mut o.size,
                        &mut o.ring,
                        &mut o.ring_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Picture(o) => {
                    changed |= opacity_pct_slider(ui, &mut o.opacity);
                    let mut sh_on = o.shadow > 0.0;
                    if ui.checkbox(&mut sh_on, t!("draw.opt.shadow")).changed() {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Emoji(_) => {
                    // No per-object options — emojis are icons, nothing
                    // to style (handled by `bottom_content` instead:
                    // the section strip shows again for fast placing).
                }
                AnnotationKind::Blur(o) => {
                    if blur_options_row(ui, &mut o.style, &mut o.strength, &mut o.shape) {
                        self.opts.blur_style = o.style;
                        self.opts.blur_strength = o.strength;
                        self.opts.shape = o.shape;
                        changed = true;
                    }
                }
                AnnotationKind::Brace(o) => {
                    let mut sh_on = o.shadow > 0.0;
                    if brace_options_row(
                        ui,
                        "draw.sel.brace",
                        &mut o.color,
                        &mut o.width,
                        &mut o.kind,
                        &mut o.dash,
                        &mut o.outline,
                        &mut o.outline_color,
                        &mut o.opacity,
                        &mut sh_on,
                    ) {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        self.opts.brace_kind = o.kind;
                        changed = true;
                    }
                }
            }
        }
        if let Some((c, f, sz, b, it, out, oc)) = persist {
            self.save_text_prefs(shared, c, f, sz, b, it, out, oc);
        }
        if changed && let Some(after) = self.doc.annotation(id).cloned() {
            self.apply(AnnoEdit::Replace { before, after });
            self.selected = Some(id);
        }
    }
}

/// Handle kinds under the pointer.
enum Handle {
    Corner(usize),
    /// (which_b, top-corner) — text quads expose 4 anchors.
    Endpoint(bool, bool),
    Tail,
}

/// The 8 resize-handle rects around `b` (canvas space).
fn handle_centers(b: Rect) -> [Rect; 8] {
    let c = |p: Pos2| Rect::from_center_size(p, vec2(8.0, 8.0));
    [
        c(b.left_top()),
        c(pos2(b.center().x, b.top())),
        c(b.right_top()),
        c(pos2(b.right(), b.center().y)),
        c(b.right_bottom()),
        c(pos2(b.center().x, b.bottom())),
        c(b.left_bottom()),
        c(pos2(b.left(), b.center().y)),
    ]
}

/// Corner-handle index under `p` for rect `r` (image space), or None.
fn corner_handle(r: PhysicalRect, p: Pos2, grab: f32) -> Option<Handle> {
    let pts = [
        pos2(r.x as f32, r.y as f32),
        pos2(r.x as f32 + r.width as f32 / 2.0, r.y as f32),
        pos2(r.x as f32 + r.width as f32, r.y as f32),
        pos2(
            r.x as f32 + r.width as f32,
            r.y as f32 + r.height as f32 / 2.0,
        ),
        pos2(r.x as f32 + r.width as f32, r.y as f32 + r.height as f32),
        pos2(
            r.x as f32 + r.width as f32 / 2.0,
            r.y as f32 + r.height as f32,
        ),
        pos2(r.x as f32, r.y as f32 + r.height as f32),
        pos2(r.x as f32, r.y as f32 + r.height as f32 / 2.0),
    ];
    pts.iter()
        .position(|q| p.distance(*q) < grab)
        .map(Handle::Corner)
}

// ── Emoji tool ─────────────────────────────────────────────────────

/// Smallest emoji side, image px.
const EMOJI_MIN_SIDE: f32 = 8.0;
/// Largest emoji side, image px — 2× the 128 px source; dragged or
/// resized past it the square just stops growing (the PNG would smear).
const EMOJI_MAX_SIDE: f32 = 256.0;
/// Click-placed side = this fraction of the image's small side…
const EMOJI_DEFAULT_FRACTION: f32 = 0.05;
/// …bounded to the Step-badge footprint range, scaled by the capture's
/// device-pixel factor (Step uses fixed 12/18/26 px radii — on a
/// Retina ×2 frame the same *point* size needs ×2 the device px).
const EMOJI_DEFAULT_MIN_PT: f32 = 24.0;
const EMOJI_DEFAULT_MAX_PT: f32 = 96.0;

/// Side of a click-placed emoji, image px: `last` when the user has
/// dragged one before, else `fraction` of the image's small side —
/// always inside the scaled Step-badge bounds (and the image itself).
fn emoji_default_side(min_side: f32, scale_factor: f32, last: Option<f32>) -> f32 {
    let hi = (EMOJI_DEFAULT_MAX_PT * scale_factor).min(min_side);
    let lo = (EMOJI_DEFAULT_MIN_PT * scale_factor).min(hi);
    last.unwrap_or(min_side * EMOJI_DEFAULT_FRACTION)
        .clamp(lo, hi)
}

/// Square from an emoji drag: `anchor` is the fixed corner, the square
/// extends toward `cur` on both axes (`max(|dx|, |dy|)`, sign-following),
/// side clamped to `[min, cap]`.
fn emoji_square(anchor: Pos2, cur: Pos2, min: f32, cap: f32) -> PhysicalRect {
    let (dx, dy) = (cur.x - anchor.x, cur.y - anchor.y);
    let side = dx.abs().max(dy.abs()).clamp(min, cap.max(min));
    PhysicalRect {
        x: (anchor.x - if dx < 0.0 { side } else { 0.0 }) as i32,
        y: (anchor.y - if dy < 0.0 { side } else { 0.0 }) as i32,
        width: side.max(1.0) as u32,
        height: side.max(1.0) as u32,
    }
}

/// `side`-sided square centred on `c`, shifted to stay inside `w×h`
/// (image px) — clicking near an edge never loses half the emoji.
fn emoji_centered(c: Pos2, side: f32, w: u32, h: u32) -> PhysicalRect {
    let side = side.min(w.min(h) as f32).max(1.0);
    let x = (c.x - side * 0.5).clamp(0.0, (w as f32 - side).max(0.0));
    let y = (c.y - side * 0.5).clamp(0.0, (h as f32 - side).max(0.0));
    PhysicalRect {
        x: x as i32,
        y: y as i32,
        width: side as u32,
        height: side as u32,
    }
}

/// Aspect-locked resize for square objects: the handle's opposite
/// point (corner, or opposite edge's centre for edge handles) stays
/// fixed and the square side follows the larger drag delta, clamped.
fn resize_square(r: &mut PhysicalRect, h: usize, p: Pos2, min: f32, max: f32) {
    let (x1, y1) = (r.x as f32, r.y as f32);
    let (x2, y2) = (x1 + r.width as f32, y1 + r.height as f32);
    let (mx, my) = ((x1 + x2) * 0.5, (y1 + y2) * 0.5);
    let (ax, ay) = match h {
        0 => (x2, y2),
        1 => (mx, y2),
        2 => (x1, y2),
        3 => (x1, my),
        4 => (x1, y1),
        5 => (mx, y1),
        6 => (x2, y1),
        _ => (x2, my),
    };
    let (dx, dy) = (p.x - ax, p.y - ay);
    let side = dx.abs().max(dy.abs()).clamp(min, max.max(min));
    *r = PhysicalRect {
        x: (ax - if dx < 0.0 { side } else { 0.0 }) as i32,
        y: (ay - if dy < 0.0 { side } else { 0.0 }) as i32,
        width: side.max(1.0) as u32,
        height: side.max(1.0) as u32,
    };
}

/// Translated label of a manifest section (the tab chip text).
fn emoji_section_label(sec: &str) -> std::borrow::Cow<'static, str> {
    match sec {
        "validation / alerts" => t!("draw.emoji.sec.validation"),
        "pointing" => t!("draw.emoji.sec.pointing"),
        "markers" => t!("draw.emoji.sec.markers"),
        "reactions / gestures" => t!("draw.emoji.sec.reactions"),
        "faces" => t!("draw.emoji.sec.faces"),
        "ideas / objects" => t!("draw.emoji.sec.objects"),
        _ => t!("draw.emoji.sec.objects"),
    }
}

/// One emoji grid cell — [`widgets::icon_button`] chrome (same theme
/// tokens) wrapped around a centred emoji image.
fn emoji_cell(ui: &mut Ui, tex: TextureId, size: f32, selected: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(size, size), Sense::click_and_drag());
    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if response.is_pointer_button_down_on() {
            (t.pressed, Stroke::NONE)
        } else if selected {
            (t.accent_soft, Stroke::new(1.0, t.accent_border))
        } else if response.hovered() {
            (t.hover, Stroke::NONE)
        } else {
            (Color32::TRANSPARENT, Stroke::NONE)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_sm),
            fill,
            stroke,
            StrokeKind::Inside,
        );
        let img_rect = rect.shrink(4.0);
        ui.painter().image(
            tex,
            img_rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "emoji")
    });
    response
}

/// Applies handle `h` drag of `p` onto rect `r`.
fn resize_rect(r: &mut PhysicalRect, h: usize, p: Pos2) {
    let (mut x1, mut y1) = (r.x as f32, r.y as f32);
    let (mut x2, mut y2) = (x1 + r.width as f32, y1 + r.height as f32);
    match h {
        0 => {
            x1 = p.x;
            y1 = p.y;
        }
        1 => y1 = p.y,
        2 => {
            x2 = p.x;
            y1 = p.y;
        }
        3 => x2 = p.x,
        4 => {
            x2 = p.x;
            y2 = p.y;
        }
        5 => y2 = p.y,
        6 => {
            x1 = p.x;
            y2 = p.y;
        }
        7 => x1 = p.x,
        _ => {}
    }
    r.x = x1.min(x2) as i32;
    r.y = y1.min(y2) as i32;
    r.width = (x1 - x2).abs().max(4.0) as u32;
    r.height = (y1 - y2).abs().max(4.0) as u32;
}

/// Distance from `p` to segment `a→b`.
/// Unit direction a→b (falls back to +x for a zero-length baseline).
fn unit_dir(a: Pos2, b: Pos2) -> Vec2 {
    let d = b - a;
    let len = d.length();
    if len < 1e-3 {
        Vec2::new(1.0, 0.0)
    } else {
        d / len
    }
}

/// Snaps `p` horizontally or vertically relative to `anchor` when `snap` is true.
fn snap_hv_pt(anchor: Pos2, p: Pos2, snap: bool) -> Pos2 {
    if !snap {
        return p;
    }
    if (p.x - anchor.x).abs() >= (p.y - anchor.y).abs() {
        pos2(p.x, anchor.y)
    } else {
        pos2(anchor.x, p.y)
    }
}

/// Default 1/3 and 2/3 cubic-Bézier control points along `a..b`.
fn bezier_rest_knobs(a: Pos2, b: Pos2) -> (Pos2, Pos2) {
    (
        pos2(a.x + (b.x - a.x) / 3.0, a.y + (b.y - a.y) / 3.0),
        pos2(a.x + (b.x - a.x) * 2.0 / 3.0, a.y + (b.y - a.y) * 2.0 / 3.0),
    )
}

/// Hit-tests the four cubic-Bézier anchors (`a`, `ka`, `kb`, `b`) — green
/// inner knobs first so they stay reachable when close to the endpoints.
fn bezier_handle_at(a: Pos2, ka: Pos2, kb: Pos2, b: Pos2, p: Pos2, grab: f32) -> Option<Handle> {
    if p.distance(ka) < grab {
        Some(Handle::Endpoint(false, false))
    } else if p.distance(kb) < grab {
        Some(Handle::Endpoint(true, false))
    } else if p.distance(a) < grab {
        Some(Handle::Endpoint(false, true))
    } else if p.distance(b) < grab {
        Some(Handle::Endpoint(true, true))
    } else {
        None
    }
}

/// Updates the dragged cubic-Bézier anchor (`a`, `ka`, `kb`, `b`); moving
/// a grey endpoint carries its adjacent green tangent handle along.
fn drag_bezier_endpoint(
    a: &mut Pos2,
    ka: &mut Pos2,
    kb: &mut Pos2,
    b: &mut Pos2,
    wb: bool,
    top: bool,
    p: Pos2,
) {
    match (wb, top) {
        (false, false) => *ka = p,
        (true, false) => *kb = p,
        (false, true) => {
            let d = p - *a;
            *a = p;
            *ka += d;
        }
        (true, true) => {
            let d = p - *b;
            *b = p;
            *kb += d;
        }
    }
}

/// Draws the 4-anchor cubic-Bézier selection chrome (dashed levers + two
/// grey endpoints + two green control knobs, plus the dashed baseline when
/// `draw_curve_guide` is set).
#[allow(clippy::too_many_arguments)]
fn paint_bezier_chrome(
    painter: &egui::Painter,
    to_canvas: &dyn Fn(Pos2) -> Pos2,
    a: Pos2,
    ka: Pos2,
    kb: Pos2,
    b: Pos2,
    accent: Color32,
    draw_curve_guide: bool,
) {
    if draw_curve_guide {
        let curve: Vec<Pos2> = (0..=32)
            .map(|i| to_canvas(render::cubic_eval(a, ka, kb, b, i as f32 / 32.0).0))
            .collect();
        painter.extend(egui::Shape::dashed_line(
            &curve,
            Stroke::new(1.0, accent),
            5.0,
            4.0,
        ));
    }
    for (p0, p1) in [(a, ka), (b, kb)] {
        painter.extend(egui::Shape::dashed_line(
            &[to_canvas(p0), to_canvas(p1)],
            Stroke::new(1.0, accent),
            6.0,
            4.0,
        ));
    }
    for (i, q) in [a, ka, kb, b].iter().enumerate() {
        let c = to_canvas(*q);
        let fill = if i == 1 || i == 2 {
            Color32::from_rgb(60, 170, 80)
        } else {
            Color32::from_rgb(200, 200, 200)
        };
        painter.circle_filled(c, 5.0, fill);
        painter.circle_stroke(c, 5.0, Stroke::new(1.0, Color32::WHITE));
    }
}

/// The four draggable anchors of a text object, like the reference:
/// the GREEN pair sits at the text baseline ends (glued to the real
/// text extent), the GREY pair at the extremities of the dashed guide
/// line extended past the text on both sides. Dragging any of them
/// gives the text an angle; the stored `a`/`b` only carry direction.
fn text_anchors(o: &crate::editor::document::TextObj) -> (Pos2, Pos2, Pos2, Pos2) {
    // Stored: a/b = grey extremities of the guide line; ka/kb = the
    // green levers — free knobs that may sit OFF the baseline.
    // Order: (grey left, green left, green right, grey right)
    (o.a, o.ka, o.kb, o.b)
}

/// Rest positions of the green knobs: on the baseline at the text
/// edges (inset from the grey extremities).
fn text_knob_rest(o: &crate::editor::document::TextObj) -> (Pos2, Pos2) {
    let (_, d, w, _) = render::text_layout(o);
    let len = (o.b - o.a).length().max(w + 16.0);
    let inset = ((len - w) / 2.0).max(0.0);
    (
        pos2(o.a.x + d.x * inset, o.a.y + d.y * inset),
        pos2(o.a.x + d.x * (inset + w), o.a.y + d.y * (inset + w)),
    )
}

fn seg_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.distance(pos2(a.x + t * dx, a.y + t * dy))
}

// ---------- options-bar widget helpers ----------

/// The marquee chrome — alternating white / accent dashes (marching
/// ants), readable on any screenshot.
fn sel_chrome(painter: &egui::Painter, rect: Rect, accent: Color32) {
    let (a, b) = (rect.min, rect.max);
    for (p, q) in [
        (a, pos2(b.x, a.y)),
        (pos2(b.x, a.y), b),
        (b, pos2(a.x, b.y)),
        (pos2(a.x, b.y), a),
    ] {
        let d = q - p;
        let len = d.length();
        if len < 1.0 {
            continue;
        }
        let dir = d / len;
        let (mut s, mut i) = (0.0, 0u32);
        while s < len {
            let e = (s + 6.0).min(len);
            let color = if i % 2 == 0 { Color32::WHITE } else { accent };
            painter.line_segment([p + dir * s, p + dir * e], Stroke::new(1.5, color));
            s += 9.0; // 6 px dash + 3 px gap
            i += 1;
        }
    }
}

/// Applies handle `h` drag of `p` onto marquee `r` (image px, f32) —
/// same 0..=7 index order as `handle_centers`.
fn resize_sel(r: &mut Rect, h: usize, p: Pos2) {
    let (mut x1, mut y1) = (r.min.x, r.min.y);
    let (mut x2, mut y2) = (r.max.x, r.max.y);
    match h {
        0 => {
            x1 = p.x;
            y1 = p.y;
        }
        1 => y1 = p.y,
        2 => {
            x2 = p.x;
            y1 = p.y;
        }
        3 => x2 = p.x,
        4 => {
            x2 = p.x;
            y2 = p.y;
        }
        5 => y2 = p.y,
        6 => {
            x1 = p.x;
            y2 = p.y;
        }
        7 => x1 = p.x,
        _ => {}
    }
    let nr = Rect::from_min_max(pos2(x1.min(x2), y1.min(y2)), pos2(x1.max(x2), y1.max(y2)));
    if nr.width() >= 2.0 && nr.height() >= 2.0 {
        *r = nr;
    }
}

/// Label + dragger bound to an f32 in `range` — FS "Epaisseur" style.
fn w_edit(
    ui: &mut Ui,
    label: impl Into<std::borrow::Cow<'static, str>>,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Response {
    ui.horizontal(|ui| {
        ui.label(label.into());
        ui.add(
            egui::DragValue::new(v)
                .range(range)
                .speed(0.2)
                .max_decimals(1),
        )
    })
    .inner
}

/// Label + slider.
fn slider(
    ui: &mut Ui,
    label: std::borrow::Cow<'static, str>,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Response {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::Slider::new(v, range)
                .show_value(false)
                .trailing_fill(true),
        )
    })
    .inner
}

/// Ctx-memory key holding the `Id` of the swatch whose pipette is armed:
/// `color_btn` is a free function, so the target is parked here and the
/// session writes the result into [`pick_done_key`].
const PICK_ARMED: &str = "draw.pick_armed";

fn pick_done_key(id: egui::Id) -> egui::Id {
    egui::Id::new(("draw.pick_done", id))
}

/// The swatch id currently targeted by the armed picker.
fn armed_pick(ctx: &egui::Context) -> Option<egui::Id> {
    ctx.data_mut(|d| d.get_persisted(egui::Id::new(PICK_ARMED)))
}

fn arm_pick(ctx: &egui::Context, id: Option<egui::Id>) {
    ctx.data_mut(|d| match id {
        Some(id) => d.insert_persisted(egui::Id::new(PICK_ARMED), id),
        None => d.remove::<egui::Id>(egui::Id::new(PICK_ARMED)),
    });
}

/// Compact pipette riding beside a colour swatch — 15 px with a 10 px
/// icon, 2 px right of the swatch's top-right corner so it hugs the
/// picker rather than filling the option line like a toolbar button.
fn pipette_btn(ui: &mut Ui, armed: bool, anchor: Rect) -> Response {
    let t = theme::tokens(ui.ctx());
    let rect = Rect::from_min_size(pos2(anchor.max.x + 2.0, anchor.min.y), Vec2::splat(15.0));
    let resp = ui.allocate_rect(rect, Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if resp.is_pointer_button_down_on() {
            (t.pressed, Stroke::NONE)
        } else if armed {
            (t.accent_soft, Stroke::new(1.0, t.accent_border))
        } else if resp.hovered() {
            (t.hover, Stroke::NONE)
        } else {
            (Color32::TRANSPARENT, Stroke::NONE)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_sm),
            fill,
            stroke,
            StrokeKind::Inside,
        );
        icons::paint(ui, rect, Icon::Pipette, 10.0, t.icon);
    }
    resp
}

/// Colour swatch button with an explicit border so light swatches
/// (e.g. white outline) remain visible on the light theme. The trailing
/// pipette arms a pick targeting THIS swatch.
fn color_btn(ui: &mut Ui, c: &mut Color32) -> Response {
    let border = theme::tokens(ui.ctx()).field_border;
    let id = ui.next_auto_id();
    let mut changed = false;
    // A pick armed for this swatch lands here on the next frame.
    let picked = ui.ctx().data_mut(|d| {
        let v = d.get_persisted::<Color32>(pick_done_key(id));
        d.remove::<Color32>(pick_done_key(id));
        v
    });
    if let Some(picked) = picked {
        *c = picked;
        changed = true;
    }
    let mut hsva: egui::epaint::Hsva = ui
        .ctx()
        .data_mut(|d| d.get_temp(id))
        .filter(|cached: &egui::epaint::Hsva| Color32::from(*cached) == *c)
        .unwrap_or_else(|| egui::epaint::Hsva::from(*c));
    let old_h = hsva.h;
    let mut r = egui::widgets::color_picker::color_edit_button_hsva(
        ui,
        &mut hsva,
        egui::widgets::color_picker::Alpha::Opaque,
    );
    if (hsva.h - old_h).abs() > 1e-4 && (hsva.s < 0.02 || hsva.v < 0.02) {
        hsva.s = 1.0;
        hsva.v = 1.0;
    }
    *c = Color32::from(hsva);
    ui.ctx().data_mut(|d| d.insert_temp(id, hsva));
    ui.painter().rect_stroke(
        r.rect,
        CornerRadius::same(3),
        Stroke::new(1.0, border),
        StrokeKind::Outside,
    );
    let armed = armed_pick(ui.ctx()) == Some(id);
    if pipette_btn(ui, armed, r.rect)
        .on_hover_text(t!("draw.opt.pick").to_string())
        .clicked()
    {
        arm_pick(ui.ctx(), (!armed).then_some(id));
    }
    if changed {
        r.mark_changed();
    }
    r
}

/// Font-size dropdown (8..=72, default 12).
fn font_size_combo(ui: &mut Ui, salt: &'static str, size: &mut f32) -> bool {
    const SIZES: &[u8] = &[
        8, 9, 10, 11, 12, 14, 16, 18, 20, 22, 24, 28, 32, 36, 42, 48, 56, 64, 72,
    ];
    let cur = size.round().clamp(8.0, 72.0) as u8;
    let mut picked = cur;
    egui::ComboBox::from_id_salt(salt)
        .width(52.0)
        .selected_text(format!("{cur}"))
        .show_ui(ui, |ui| {
            for &s in SIZES {
                ui.selectable_value(&mut picked, s, format!("{s}"));
            }
        });
    if picked != cur {
        *size = f32::from(picked);
        true
    } else {
        false
    }
}

/// Outline-width dropdown (0..=10).
/// Outline-width combo: a `0..=10` menu.
fn outline_combo(ui: &mut Ui, salt: &'static str, outline: &mut f32) -> bool {
    let cur = outline.round().clamp(0.0, 10.0) as u8;
    let mut picked = cur;
    egui::ComboBox::from_id_salt(salt)
        .width(46.0)
        .selected_text(format!("{cur}"))
        .show_ui(ui, |ui| {
            for w in 0u8..=10 {
                ui.selectable_value(&mut picked, w, format!("{w}"));
            }
        });
    if picked != cur {
        *outline = f32::from(picked);
        true
    } else {
        false
    }
}

/// Opacity slider displayed as 10..=100% (100% by default).
fn opacity_pct_slider(ui: &mut Ui, opacity: &mut f32) -> bool {
    ui.horizontal(|ui| {
        ui.label(t!("draw.opt.opacity"));
        let mut pct = (*opacity * 100.0).round().clamp(10.0, 100.0);
        let r = ui.add(
            egui::Slider::new(&mut pct, 10.0..=100.0)
                .trailing_fill(true)
                .suffix("%")
                .max_decimals(0),
        );
        if r.changed() {
            *opacity = (pct / 100.0).clamp(0.1, 1.0);
            true
        } else {
            false
        }
    })
    .inner
}

/// Dash-style combo (Solid / Dash / Dot / DashDot).
fn dash_combo(ui: &mut Ui, dash: &mut Dash) -> bool {
    let cur = *dash;
    enum_combo(
        ui,
        "draw.opt.dash",
        cur,
        &[
            (Dash::Solid, "—".into()),
            (Dash::Dash, "— —".into()),
            (Dash::Dot, "···".into()),
            (Dash::DashDot, "—·".into()),
        ],
        |v| *dash = v,
    );
    *dash != cur
}

/// Line/arrow option strip shared by `Tool::Arrow` and a selected `LineObj`.
#[allow(clippy::too_many_arguments)]
fn line_options_row(
    ui: &mut Ui,
    salt: &'static str,
    color: &mut Color32,
    width: &mut f32,
    start: &mut LineEnd,
    end: &mut LineEnd,
    dash: &mut Dash,
    outline: &mut f32,
    outline_color: &mut Color32,
    opacity: &mut f32,
    shadow_on: &mut bool,
) -> bool {
    const PRESETS: &[((LineEnd, LineEnd), &str)] = &[
        ((LineEnd::None, LineEnd::None), "———"),
        ((LineEnd::None, LineEnd::Arrow), "——→"),
        ((LineEnd::Dot, LineEnd::Arrow), "●—→"),
        ((LineEnd::Ring, LineEnd::Arrow), "○—→"),
        ((LineEnd::Dot, LineEnd::Dot), "●—●"),
        ((LineEnd::Ring, LineEnd::Ring), "○—○"),
        ((LineEnd::Arrow, LineEnd::None), "←——"),
        ((LineEnd::Arrow, LineEnd::Dot), "←—●"),
        ((LineEnd::Arrow, LineEnd::Ring), "←—○"),
        ((LineEnd::Arrow, LineEnd::Arrow), "←—→"),
    ];
    let mut changed = false;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.color"));
            color_btn(ui, color).changed()
        })
        .inner;
    changed |= w_edit(ui, t!("draw.opt.width"), width, 1.0..=20.0).changed();

    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.start"));
            let cur = (*start, *end);
            let mut picked = cur;
            let cur_label = PRESETS
                .iter()
                .find(|(p, _)| *p == cur)
                .map_or("——→", |(_, l)| *l);
            egui::ComboBox::from_id_salt((salt, "ends"))
                .width(68.0)
                .selected_text(cur_label)
                .show_ui(ui, |ui| {
                    for &(pair, label) in PRESETS {
                        ui.selectable_value(&mut picked, pair, label);
                    }
                });
            if picked != cur {
                *start = picked.0;
                *end = picked.1;
                true
            } else {
                false
            }
        })
        .inner;

    changed |= dash_combo(ui, dash);

    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.outline"));
            outline_combo(ui, salt, outline) | color_btn(ui, outline_color).changed()
        })
        .inner;

    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}

/// Freehand pencil option strip shared by `Tool::Pencil` and a selected `FreeObj`.
#[allow(clippy::too_many_arguments)]
fn pencil_options_row(
    ui: &mut Ui,
    salt: &'static str,
    color: &mut Color32,
    width: &mut f32,
    tip: &mut TipShape,
    dash: &mut Dash,
    outline: &mut f32,
    outline_color: &mut Color32,
    opacity: &mut f32,
    shadow_on: &mut bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.color"));
            color_btn(ui, color).changed()
        })
        .inner;
    changed |= w_edit(ui, t!("draw.opt.width"), width, 1.0..=30.0).changed();
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.style"));
            tip_combo(ui, tip)
        })
        .inner;
    changed |= dash_combo(ui, dash);
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.outline"));
            outline_combo(ui, salt, outline) | color_btn(ui, outline_color).changed()
        })
        .inner;
    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}

/// Option strip shared by `Tool::Brace` and a selected `BraceObj`.
#[allow(clippy::too_many_arguments)]
fn brace_options_row(
    ui: &mut Ui,
    salt: &'static str,
    color: &mut Color32,
    width: &mut f32,
    kind: &mut BraceKind,
    dash: &mut Dash,
    outline: &mut f32,
    outline_color: &mut Color32,
    opacity: &mut f32,
    shadow_on: &mut bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.color"));
            color_btn(ui, color).changed()
        })
        .inner;
    changed |= w_edit(ui, t!("draw.opt.width"), width, 1.0..=20.0).changed();
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.shape"));
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                let mut ch = false;
                for candidate in BraceKind::ALL {
                    if brace_kind_button(ui, candidate, *kind == candidate).clicked()
                        && *kind != candidate
                    {
                        *kind = candidate;
                        ch = true;
                    }
                }
                ch
            })
            .inner
        })
        .inner;
    changed |= dash_combo(ui, dash);
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.outline"));
            outline_combo(ui, salt, outline) | color_btn(ui, outline_color).changed()
        })
        .inner;
    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}

/// Clickable mini bracket-shape button for the `Forme:` row.
fn brace_kind_button(ui: &mut Ui, kind: BraceKind, selected: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let (fill, stroke) = if response.is_pointer_button_down_on() {
            (t.pressed, Stroke::NONE)
        } else if selected {
            (t.accent_soft, Stroke::new(1.0, t.accent_border))
        } else if response.hovered() {
            (t.hover, Stroke::NONE)
        } else {
            (Color32::TRANSPARENT, Stroke::NONE)
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(t.radius_sm),
            fill,
            stroke,
            StrokeKind::Inside,
        );
        let c = rect.center();
        let half_h = 6.5_f32;
        let x_shift = if kind.is_left() { 2.0 } else { -2.0 };
        let sample = BraceObj {
            a: pos2(c.x + x_shift, c.y - half_h),
            b: pos2(c.x + x_shift, c.y + half_h),
            kind,
            color: t.icon,
            width: 1.5,
            dash: Dash::Solid,
            outline: 0.0,
            outline_color: Color32::TRANSPARENT,
            opacity: 1.0,
            shadow: 0.0,
        };
        let pts = render::brace_polyline(&sample);
        ui.painter()
            .add(egui::Shape::line(pts, Stroke::new(1.5, t.icon)));
    }
    response
}

/// Option strip shared by `Rect`, `RectFill`, `Ellipse`, `EllipseFill` and
/// their selected `RectObj` / `EllipseObj` counterparts.
#[allow(clippy::too_many_arguments)]
fn shape_options_row(
    ui: &mut Ui,
    salt: &'static str,
    stroke: &mut Color32,
    stroke_w: &mut f32,
    fill: &mut Option<Color32>,
    radius: &mut Option<&mut f32>,
    dash: &mut Dash,
    outline: &mut f32,
    outline_color: &mut Color32,
    opacity: &mut f32,
    shadow_on: &mut bool,
) -> bool {
    let mut changed = false;
    if let Some(fc) = fill {
        changed |= ui
            .horizontal(|ui| {
                ui.label(t!("draw.opt.fill_c"));
                color_btn(ui, fc).changed()
            })
            .inner;
        changed |= ui
            .horizontal(|ui| {
                ui.label(t!("draw.opt.border"));
                color_btn(ui, stroke).changed()
            })
            .inner;
        changed |= w_edit(ui, t!("draw.opt.width"), stroke_w, 0.0..=20.0).changed();
    } else {
        changed |= ui
            .horizontal(|ui| {
                ui.label(t!("draw.opt.color"));
                color_btn(ui, stroke).changed()
            })
            .inner;
        changed |= w_edit(ui, t!("draw.opt.width"), stroke_w, 1.0..=20.0).changed();
    }
    if let Some(rad) = radius {
        changed |= ui
            .horizontal(|ui| {
                ui.label(t!("draw.opt.radius"));
                ui.scope(|ui| {
                    ui.spacing_mut().slider_width = 65.0;
                    ui.add(
                        egui::Slider::new(*rad, 0.0..=60.0)
                            .trailing_fill(true)
                            .max_decimals(0),
                    )
                })
                .inner
                .changed()
            })
            .inner;
    }
    changed |= dash_combo(ui, dash);
    if fill.is_none() {
        changed |= ui
            .horizontal(|ui| {
                ui.label(t!("draw.opt.outline"));
                outline_combo(ui, salt, outline) | color_btn(ui, outline_color).changed()
            })
            .inner;
    }
    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}

/// Tip-shape combo (round/square nib).
fn tip_combo(ui: &mut Ui, tip: &mut TipShape) -> bool {
    let cur = *tip;
    egui::ComboBox::from_id_salt("draw.tip")
        .selected_text(match cur {
            TipShape::Round => "●",
            TipShape::Square => "■",
        })
        .width(46.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(tip, TipShape::Round, "●");
            ui.selectable_value(tip, TipShape::Square, "■");
        });
    *tip != cur
}

/// Option strip shared by `Tool::HighlightArea` and a selected `AreaObj`.
fn highlight_area_options_row(
    ui: &mut Ui,
    salt: &'static str,
    color: &mut Color32,
    shape: &mut AreaShape,
    opacity: &mut f32,
) -> bool {
    let mut changed = false;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.color"));
            color_btn(ui, color).changed()
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.style"));
            let cur = if *shape == AreaShape::Rect {
                AreaShape::Rect
            } else {
                AreaShape::Ellipse
            };
            let mut picked = cur;
            egui::ComboBox::from_id_salt((salt, "shape"))
                .selected_text(match cur {
                    AreaShape::Rect => t!("draw.shape.rect").to_string(),
                    _ => t!("draw.shape.circle").to_string(),
                })
                .width(96.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut picked,
                        AreaShape::Rect,
                        t!("draw.shape.rect").to_string(),
                    );
                    ui.selectable_value(
                        &mut picked,
                        AreaShape::Ellipse,
                        t!("draw.shape.circle").to_string(),
                    );
                });
            if picked != *shape {
                *shape = picked;
            }
            picked != cur
        })
        .inner;
    changed |= opacity_pct_slider(ui, opacity);
    changed
}

/// Area-shape combo.
fn shape_combo(ui: &mut Ui, shape: &mut AreaShape) {
    let cur = *shape;
    egui::ComboBox::from_id_salt("draw.shape")
        .selected_text(match cur {
            AreaShape::Rect => t!("draw.shape.rect").to_string(),
            AreaShape::Round => t!("draw.shape.round").to_string(),
            AreaShape::Ellipse => t!("draw.shape.oval").to_string(),
        })
        .width(90.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(shape, AreaShape::Rect, t!("draw.shape.rect").to_string());
            ui.selectable_value(shape, AreaShape::Round, t!("draw.shape.round").to_string());
            ui.selectable_value(shape, AreaShape::Ellipse, t!("draw.shape.oval").to_string());
        });
}

/// Blur option strip shared by `Tool::Blur` and a selected `BlurObj`.
fn blur_options_row(
    ui: &mut Ui,
    style: &mut BlurStyle,
    strength: &mut u8,
    shape: &mut AreaShape,
) -> bool {
    let cur_style = *style;
    enum_combo(
        ui,
        "draw.opt.style",
        cur_style,
        &[
            (BlurStyle::Blur, t!("draw.opt.blur")),
            (BlurStyle::Pixelate, t!("draw.opt.pixelate")),
        ],
        |v| *style = v,
    );
    let mut s = f32::from(*strength);
    let str_changed = slider(ui, t!("draw.opt.strength"), &mut s, 1.0..=100.0).changed();
    if str_changed {
        *strength = s as u8;
    }
    let cur_shape = *shape;
    shape_combo(ui, shape);
    *style != cur_style || str_changed || *shape != cur_shape
}

/// Text-align combo.
fn align_combo(ui: &mut Ui, align: &mut TextAlign) {
    let cur = *align;
    egui::ComboBox::from_id_salt("draw.align")
        .selected_text(match cur {
            TextAlign::Left | TextAlign::Justify => "◧",
            TextAlign::Center => "☰",
            TextAlign::Right => "◨",
        })
        .width(60.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(align, TextAlign::Left, "◧");
            ui.selectable_value(align, TextAlign::Center, "☰");
            ui.selectable_value(align, TextAlign::Right, "◨");
        });
}

/// Generic small enum combo labelled by `key`.
fn enum_combo<T: Copy + PartialEq>(
    ui: &mut Ui,
    key: &str,
    cur: T,
    items: &[(T, std::borrow::Cow<'static, str>)],
    set: impl FnOnce(T),
) {
    ui.horizontal(|ui| {
        ui.label(t!(key));
        let mut choice = cur;
        egui::ComboBox::from_id_salt(key)
            .selected_text(
                items
                    .iter()
                    .find(|(v, _)| *v == cur)
                    .map(|(_, l)| l.to_string())
                    .unwrap_or_default(),
            )
            .width(90.0)
            .show_ui(ui, |ui| {
                for (v, l) in items {
                    ui.selectable_value(&mut choice, *v, l.to_string());
                }
            });
        if choice != cur {
            set(choice);
        }
    });
}

/// Single-row option strip shared by `Tool::Step` and a selected `StepObj`.
#[allow(clippy::too_many_arguments)]
fn step_options_row(
    ui: &mut Ui,
    salt: &'static str,
    mode: StepMode,
    count: &mut u32,
    lower: &mut bool,
    bg: &mut Color32,
    fg: &mut Color32,
    style: &mut StepStyle,
    size: &mut StepSize,
    ring: &mut f32,
    ring_color: &mut Color32,
    opacity: &mut f32,
    shadow_on: &mut bool,
) -> bool {
    let mut changed = false;
    changed |= ui
        .horizontal(|ui| {
            ui.label(if mode == StepMode::Letters {
                t!("draw.opt.letter")
            } else {
                t!("draw.opt.count")
            });
            let mut n = (*count).max(1) as i32;
            let is_lower = *lower;
            let dv = egui::DragValue::new(&mut n).range(1..=999).speed(0.2);
            let dv = if mode == StepMode::Letters {
                dv.custom_formatter(move |v, _| {
                    let base = if is_lower { 'a' } else { 'A' } as u32;
                    char::from_u32(base + (v as u32).saturating_sub(1) % 26)
                        .map(|c| c.to_string())
                        .unwrap_or_default()
                })
            } else {
                dv
            };
            let mut ch = ui.add(dv).changed();
            if ch {
                *count = n.max(1) as u32;
            }
            if mode == StepMode::Letters
                && ui
                    .checkbox(lower, t!("draw.opt.lower").to_string())
                    .changed()
            {
                ch = true;
            }
            ch
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.bg"));
            color_btn(ui, bg).changed()
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.fg"));
            color_btn(ui, fg).changed()
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.style"));
            let cur_style = *style;
            let mut picked_style = cur_style;
            egui::ComboBox::from_id_salt((salt, "style"))
                .width(46.0)
                .selected_text(match cur_style {
                    StepStyle::Circle => "●",
                    StepStyle::Square => "■",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picked_style, StepStyle::Circle, "●");
                    ui.selectable_value(&mut picked_style, StepStyle::Square, "■");
                });
            if picked_style != cur_style {
                *style = picked_style;
            }
            picked_style != cur_style
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.size"));
            let cur_size = *size;
            let mut picked_size = cur_size;
            egui::ComboBox::from_id_salt((salt, "size"))
                .width(46.0)
                .selected_text(match cur_size {
                    StepSize::Small => "S",
                    StepSize::Medium => "M",
                    StepSize::Large => "L",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut picked_size, StepSize::Small, "S");
                    ui.selectable_value(&mut picked_size, StepSize::Medium, "M");
                    ui.selectable_value(&mut picked_size, StepSize::Large, "L");
                });
            if picked_size != cur_size {
                *size = picked_size;
            }
            picked_size != cur_size
        })
        .inner;
    changed |= ui
        .horizontal(|ui| {
            ui.label(t!("draw.opt.outline"));
            outline_combo(ui, salt, ring) | color_btn(ui, ring_color).changed()
        })
        .inner;
    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture::{Capture, MonitorInfo};
    use crate::editor::document::Document;

    /// `emoji_default_side` honouring: a remembered `last` wins but is
    /// still re-bounded to the (scale-scaled) Step-badge range.
    #[test]
    fn emoji_default_side_bounds_and_memory() {
        // 5 % of a 1080-tall capture, sf=1 → 54 px (Step-L footprint).
        assert_eq!(emoji_default_side(1080.0, 1.0, None), 54.0);
        // Small image: 5 % of 300 → 24 px floor.
        assert_eq!(emoji_default_side(300.0, 1.0, None), 24.0);
        // Retina ×2 doubles the bounds, not the fraction: 5 % → 54 but
        // the floor moved to 48 and the ceiling to 192.
        assert_eq!(emoji_default_side(4000.0, 2.0, None), 192.0);
        assert_eq!(emoji_default_side(300.0, 2.0, None), 48.0);
        // Remembered size is reused, re-clamped to bounds.
        assert_eq!(emoji_default_side(1080.0, 1.0, Some(80.0)), 80.0);
        assert_eq!(emoji_default_side(1080.0, 1.0, Some(500.0)), 96.0);
        assert_eq!(emoji_default_side(1080.0, 1.0, Some(4.0)), 24.0);
        // Tiny image clamps the ceiling to the small side itself.
        assert_eq!(emoji_default_side(20.0, 1.0, None), 20.0);
    }

    /// `emoji_square`: side = max delta, direction follows the drag,
    /// capped at `cap`, floored at `min`.
    #[test]
    fn emoji_square_direction_and_cap() {
        let a = pos2(100.0, 100.0);
        // Down-right.
        let r = emoji_square(a, pos2(160.0, 140.0), 8.0, 256.0);
        assert_eq!(
            r,
            PhysicalRect {
                x: 100,
                y: 100,
                width: 60,
                height: 60
            }
        );
        // Up-left (larger |dy| wins) — square grows up-left of anchor.
        let r = emoji_square(a, pos2(70.0, 20.0), 8.0, 256.0);
        assert_eq!(
            r,
            PhysicalRect {
                x: 20,
                y: 20,
                width: 80,
                height: 80
            }
        );
        // Past the cap it just stops growing.
        let r = emoji_square(a, pos2(900.0, 100.0), 8.0, 256.0);
        assert_eq!(r.width, 256);
        // Under min it floors.
        let r = emoji_square(a, pos2(103.0, 101.0), 8.0, 256.0);
        assert_eq!(r.width, 8);
    }

    /// `emoji_centered`: centred on the pointer, shifted back inside
    /// the image near edges.
    #[test]
    fn emoji_centered_stays_inside() {
        let r = emoji_centered(pos2(200.0, 200.0), 64.0, 800, 600);
        assert_eq!(
            r,
            PhysicalRect {
                x: 168,
                y: 168,
                width: 64,
                height: 64
            }
        );
        let r = emoji_centered(pos2(4.0, 596.0), 64.0, 800, 600);
        assert_eq!(
            r,
            PhysicalRect {
                x: 0,
                y: 536,
                width: 64,
                height: 64
            }
        );
        // Side bigger than the image is clamped to it.
        let r = emoji_centered(pos2(50.0, 50.0), 400.0, 200, 150);
        assert_eq!(r.width, 150);
    }

    /// `resize_square`: opposite corner stays fixed, the square follows
    /// the larger axis, clamped — aspect always 1:1.
    #[test]
    fn resize_square_keeps_ratio_and_anchor() {
        let mut r = PhysicalRect {
            x: 10,
            y: 10,
            width: 40,
            height: 40,
        };
        // Corner 4 (bottom-right) drag → top-left stays at (10,10).
        resize_square(&mut r, 4, pos2(80.0, 70.0), 8.0, 256.0);
        assert_eq!(
            r,
            PhysicalRect {
                x: 10,
                y: 10,
                width: 70,
                height: 70
            }
        );
        // Corner 0 (top-left) drag → bottom-right stays at (80,80);
        // the larger delta (|dx|=50) sets the square — y snaps to keep
        // the 1:1 ratio, it does NOT follow the pointer.
        resize_square(&mut r, 0, pos2(30.0, 40.0), 8.0, 256.0);
        assert_eq!(
            r,
            PhysicalRect {
                x: 30,
                y: 30,
                width: 50,
                height: 50
            }
        );
        // Clamped to max.
        resize_square(&mut r, 4, pos2(900.0, 900.0), 8.0, 256.0);
        assert_eq!(r.width, 256);
        assert_eq!(r.width, r.height);
    }

    /// The full click path: placing an emoji bumps the document
    /// revision (thumbnail queues are revision-keyed), selects it with
    /// handles and records exactly ONE undo step.
    #[test]
    fn click_places_emoji_one_undo() {
        let doc = Document::new(Capture {
            image: std::sync::Arc::new(image::RgbaImage::from_pixel(
                800,
                600,
                image::Rgba([30, 30, 30, 255]),
            )),
            scale_factor: 1.0,
            monitor: MonitorInfo {
                id: 0,
                name: String::new(),
                rect: PhysicalRect::default(),
                scale_factor: 1.0,
                is_primary: true,
            },
        });
        let mut s = DrawSession::open(
            doc,
            0,
            "t".to_owned(),
            VecDeque::new(),
            VecDeque::new(),
            &crate::config::Settings::default(),
        );
        s.tool = Tool::Emoji;
        let rev0 = s.doc.revision;
        s.click(pos2(400.0, 300.0), &egui::Context::default());
        assert!(s.doc.revision > rev0, "placement must bump revision");
        assert_eq!(s.undo.len(), 1, "one placement = one undo step");
        let id = s.selected.expect("placed emoji stays selected");
        match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Emoji(o) => {
                assert_eq!(o.id, 0x1F449);
                assert_eq!(o.rect.width, o.rect.height);
                assert_eq!(o.rect.width, 30); // 5 % of 600
            }
            other => panic!("expected Emoji, got {other:?}"),
        }
        // Moving then resizing the emoji bumps the revision again —
        // the thumbnail queue keys off it.
        let rev = s.doc.revision;
        s.doc.annotation_mut(id).unwrap().kind.translate(5.0, -3.0);
        s.doc.revision += 1;
        assert!(s.doc.revision > rev);
        let rev = s.doc.revision;
        let a = s.doc.annotation_mut(id).unwrap();
        let mut r = a.kind.bounds();
        resize_square(
            &mut r,
            4,
            pos2(500.0, 500.0),
            EMOJI_MIN_SIDE,
            EMOJI_MAX_SIDE,
        );
        a.kind.set_bounds(r);
        s.doc.revision += 1;
        assert!(s.doc.revision > rev);
        match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Emoji(o) => assert_eq!(o.rect.width, o.rect.height),
            _ => unreachable!(),
        }
    }
}

#[cfg(test)]
mod live_drag_tests {
    //! The deferred-render drag pipeline: doc + revision stay frozen
    //! for the whole gesture, ONE bump at release/cancel-restores.
    use super::*;
    use crate::capture::{Capture, MonitorInfo};
    use crate::editor::document::{Dash, Document, RectObj};

    fn session() -> DrawSession {
        let doc = Document::new(Capture {
            image: std::sync::Arc::new(image::RgbaImage::from_pixel(
                800,
                600,
                image::Rgba([30, 30, 30, 255]),
            )),
            scale_factor: 1.0,
            monitor: MonitorInfo {
                id: 0,
                name: String::new(),
                rect: PhysicalRect::default(),
                scale_factor: 1.0,
                is_primary: true,
            },
        });
        let mut s = DrawSession::open(
            doc,
            0,
            "t".to_owned(),
            VecDeque::new(),
            VecDeque::new(),
            &crate::config::Settings::default(),
        );
        // The canvas texture/backup the live patch writes into — the
        // test drives the same buffers canvas() would.
        s.rendered_img = Some(crate::editor::render::render_to_image(&s.doc));
        s.rendered_rev = s.doc.revision;
        let id = s.doc.alloc_id();
        s.doc.annotations.push(Annotation {
            id,
            kind: AnnotationKind::Rect(RectObj {
                rect: PhysicalRect {
                    x: 100,
                    y: 100,
                    width: 60,
                    height: 40,
                },
                stroke: Color32::RED,
                stroke_w: 3.0,
                dash: Dash::Solid,
                fill: None,
                radius: 0.0,
                outline: 0.0,
                outline_color: Color32::BLACK,
                opacity: 1.0,
                shadow: 0.0,
            }),
        });
        s.doc.revision += 1;
        s.rendered_rev = s.doc.revision;
        s.selected = Some(id);
        s
    }

    #[test]
    fn moving_freezes_revision_then_bumps_once() {
        let mut s = session();
        let id = s.selected.unwrap();
        let before = s.doc.annotation(id).unwrap().clone();
        let rev0 = s.doc.revision;
        // Press → threshold passed → drag starts (live machinery).
        s.drag = Some(Drag::Moving {
            id,
            live: before.clone(),
            before,
            last: pos2(130.0, 120.0),
        });
        s.begin_live_drag();
        let rev_at_start = s.doc.revision;
        assert_eq!(rev_at_start, rev0, "drag start must not bump revision");
        // Pointer moves: live travels, doc + revision frozen.
        for p in [pos2(140.0, 125.0), pos2(160.0, 135.0), pos2(190.0, 140.0)] {
            s.continue_drag(p);
            assert_eq!(s.doc.revision, rev0, "revision moved mid-drag");
        }
        let doc_x = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(doc_x, 100, "doc still at the old spot mid-drag");
        // Release: one bump, one undo step, object moved.
        let drag = s.drag.take().unwrap();
        s.finish_drag(drag);
        assert_eq!(s.doc.revision, rev0 + 1, "one revision bump on release");
        assert_eq!(s.undo.len(), 1);
        let doc_x = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(doc_x, 160, "released at the live position");
        assert!(s.drag_region.is_none(), "exclusion cleaned up");
    }

    #[test]
    fn esc_cancels_without_touching_revision_or_undo() {
        let mut s = session();
        let id = s.selected.unwrap();
        let before = s.doc.annotation(id).unwrap().clone();
        let rev0 = s.doc.revision;
        s.drag = Some(Drag::Moving {
            id,
            live: before.clone(),
            before,
            last: pos2(130.0, 120.0),
        });
        s.begin_live_drag();
        s.continue_drag(pos2(200.0, 200.0));
        s.cancel_live_drag();
        assert!(s.drag.is_none());
        assert_eq!(s.doc.revision, rev0, "Esc must not bump revision");
        assert_eq!(s.undo.len(), 0, "Esc writes no undo step");
        let doc_x = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(doc_x, 100, "object restored at its original spot");
        assert!(s.drag_region.is_none());
    }

    #[test]
    fn nudge_burst_freezes_then_commits_once() {
        let mut s = session();
        let id = s.selected.unwrap();
        let rev0 = s.doc.revision;
        let a = s.doc.annotation(id).unwrap().clone();
        s.nudge = Some(NudgeBurst {
            id,
            before: a.clone(),
            live: a.clone(),
            at: std::time::Instant::now(),
        });
        s.begin_nudge_live(&a);
        // Several presses: live moves, doc+revision frozen.
        for _ in 0..3 {
            if let Some(n) = &mut s.nudge {
                n.live.kind.translate(1.0, 0.0);
            }
            assert_eq!(s.doc.revision, rev0);
        }
        s.commit_nudge();
        assert_eq!(s.doc.revision, rev0 + 1, "one bump for the burst");
        assert_eq!(s.undo.len(), 1);
        let doc_x = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(doc_x, 103);
    }

    #[test]
    fn release_expands_overlapping_blur_and_matches_full_render() {
        let mut s = session();
        let rect_id = s.selected.unwrap();
        let blur_id = s.doc.alloc_id();
        s.doc.annotations.push(Annotation {
            id: blur_id,
            kind: AnnotationKind::Blur(BlurObj {
                rect: PhysicalRect {
                    x: 200,
                    y: 90,
                    width: 120,
                    height: 80,
                },
                style: BlurStyle::Pixelate,
                shape: AreaShape::Rect,
                strength: 60,
            }),
        });
        s.doc.revision += 1;
        s.rendered_img = Some(crate::editor::render::render_to_image(&s.doc));
        s.rendered_rev = s.doc.revision;

        let before = s.doc.annotation(rect_id).unwrap().clone();
        s.drag = Some(Drag::Moving {
            id: rect_id,
            live: before.clone(),
            before,
            last: pos2(130.0, 120.0),
        });
        s.begin_live_drag();
        s.continue_drag(pos2(220.0, 120.0));
        let drag = s.drag.take().unwrap();
        s.finish_drag(drag);

        let expect = crate::editor::render::render_to_image(&s.doc);
        assert_eq!(s.rendered_img.as_ref().unwrap().as_raw(), expect.as_raw());
    }

    #[test]
    fn fallback_over_half_image_excludes_dragged_object() {
        let mut s = session();
        let rect_id = s.selected.unwrap();
        let blur_id = s.doc.alloc_id();
        // A blur covering >50% of the 800×600 canvas overlapping the rect.
        s.doc.annotations.push(Annotation {
            id: blur_id,
            kind: AnnotationKind::Blur(BlurObj {
                rect: PhysicalRect {
                    x: 50,
                    y: 50,
                    width: 700,
                    height: 500,
                },
                style: BlurStyle::Pixelate,
                shape: AreaShape::Rect,
                strength: 40,
            }),
        });
        s.doc.revision += 1;
        s.rendered_img = Some(crate::editor::render::render_to_image(&s.doc));
        s.rendered_rev = s.doc.revision;

        let before = s.doc.annotation(rect_id).unwrap().clone();
        s.drag = Some(Drag::Moving {
            id: rect_id,
            live: before.clone(),
            before,
            last: pos2(130.0, 120.0),
        });
        s.begin_live_drag();
        assert!(s.drag_region.is_none(), ">50% region must trigger fallback");
        let expect_excluded =
            crate::editor::render::render_to_image_filtered(&s.doc, Some(rect_id));
        assert_eq!(
            s.rendered_img.as_ref().unwrap().as_raw(),
            expect_excluded.as_raw(),
            "fallback render must exclude the dragged annotation"
        );
        s.cancel_live_drag();
        let expect_full = crate::editor::render::render_to_image(&s.doc);
        assert_eq!(
            s.rendered_img.as_ref().unwrap().as_raw(),
            expect_full.as_raw()
        );
    }

    #[test]
    fn delete_and_undo_flush_pending_nudge_first() {
        let mut s = session();
        let id = s.selected.unwrap();
        let a = s.doc.annotation(id).unwrap().clone();
        let mut moved = a.clone();
        moved.kind.translate(15.0, 0.0);
        s.nudge = Some(NudgeBurst {
            id,
            before: a.clone(),
            live: moved,
            at: std::time::Instant::now(),
        });
        s.begin_nudge_live(&a);
        // Delete before the 400 ms timer fires: must commit the nudge
        // first so the first undo restores the nudged position (x = 115).
        s.delete_selected();
        assert_eq!(s.undo.len(), 2);
        s.undo_once();
        let x_after_undo_delete = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(x_after_undo_delete, 115);
        s.undo_once();
        let x_after_undo_nudge = match &s.doc.annotation(id).unwrap().kind {
            AnnotationKind::Rect(o) => o.rect.x,
            _ => unreachable!(),
        };
        assert_eq!(x_after_undo_nudge, 100);
    }
}
