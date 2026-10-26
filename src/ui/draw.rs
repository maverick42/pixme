//! The "Dessiner" annotation window: a separate deferred viewport with
//! a two-column tool strip (PhotoCraft styling), the composited
//! document canvas, a FastStone-compatible contextual options bar and
//! OK/Annuler. The session edits a clone of the viewer document — OK
//! writes it back (annotations stay re-editable on re-entry), Annuler
//! drops it. Chronological undo/redo uses `AnnoEdit` stacks stored on
//! the viewer `Tab`, so they survive editor close/reopen and die with
//! the tab or on flattening pixel ops.

use std::collections::VecDeque;

use egui::{
    Color32, ColorImage, CornerRadius, Key, Pos2, Rect, Response, ScrollArea, Sense, Stroke,
    StrokeKind, TextureHandle, TextureOptions, Ui, Vec2, pos2, vec2,
};
use rust_i18n::t;

use crate::app::Shared;
use crate::capture::PhysicalRect;
use crate::editor::document::{
    Annotation, AnnotationId, AnnotationKind, AreaObj, AreaShape, BlurObj, BlurStyle, BoxObj, Dash,
    Document, EllipseObj, EraserMode, FancyObj, FillObj, FreeObj, LineEnd, LineObj, PicObj,
    RectObj, StampId, StampObj, StepMode, StepObj, StepSize, StepStyle, TextAlign, TextObj,
    TipShape,
};
use crate::editor::history::{ANNO_HISTORY_CAP, AnnoEdit};
use crate::editor::render;
use crate::ui::icons::{self, Icon};
use crate::ui::{theme, widgets};

/// One drawable tool of the strip (grid order = the user's reference
/// screenshots: two columns, fill/stamps/image on the last row).
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
    FancyArrow,
    Pencil,
    Eraser,
    Rect,
    RectFill,
    Ellipse,
    EllipseFill,
    HighlightLine,
    HighlightArea,
    Fill,
    Blur,
    Stamps,
    InsertImage,
}

impl Tool {
    /// The strip grid in display order (row-major, two columns).
    const ALL: [Tool; 20] = [
        Self::Select,
        Self::SelectArea,
        Self::TextBoxRect,
        Self::TextBoxOval,
        Self::Text,
        Self::Step,
        Self::Arrow,
        Self::FancyArrow,
        Self::Pencil,
        Self::Eraser,
        Self::Rect,
        Self::RectFill,
        Self::Ellipse,
        Self::EllipseFill,
        Self::HighlightLine,
        Self::HighlightArea,
        Self::Fill,
        Self::Blur,
        Self::Stamps,
        Self::InsertImage,
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
            Self::FancyArrow => Icon::ArrowBigUp,
            Self::Pencil => Icon::Pencil,
            Self::Eraser => Icon::Eraser,
            Self::Rect => Icon::Square,
            Self::RectFill => Icon::SquareFill,
            Self::Ellipse => Icon::Circle,
            Self::EllipseFill => Icon::CircleFill,
            Self::HighlightLine => Icon::Highlighter,
            Self::HighlightArea => Icon::SquareFill,
            Self::Fill => Icon::PaintBucket,
            Self::Blur => Icon::Pixelate,
            Self::Stamps => Icon::MousePointerClick,
            Self::InsertImage => Icon::Image,
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
            Self::FancyArrow => t!("draw.tool.fancy"),
            Self::Pencil => t!("draw.tool.pencil"),
            Self::Eraser => t!("draw.tool.eraser"),
            Self::Rect => t!("draw.tool.rect"),
            Self::RectFill => t!("draw.tool.rectfill"),
            Self::Ellipse => t!("draw.tool.ellipse"),
            Self::EllipseFill => t!("draw.tool.ellipsefill"),
            Self::HighlightLine => t!("draw.tool.hline"),
            Self::HighlightArea => t!("draw.tool.harea"),
            Self::Fill => t!("draw.tool.fill"),
            Self::Blur => t!("draw.tool.blur"),
            Self::Stamps => t!("draw.tool.stamps"),
            Self::InsertImage => t!("draw.tool.insert"),
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
    shape: AreaShape,
    soft: bool,
    border_on: bool,
    // Eraser tool.
    eraser_color: Color32,
    eraser_size: f32,
    eraser_mode: EraserMode,
    eraser_pick: bool,
    // Fill tool.
    tolerance: u8,
    transparent_only: bool,
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
    // Stamps.
    stamp: StampId,
    stamp_size: f32,
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
            shape: AreaShape::Rect,
            soft: false,
            border_on: false,
            eraser_color: Color32::WHITE,
            eraser_size: 12.0,
            eraser_mode: EraserMode::Color,
            eraser_pick: false,
            tolerance: 32,
            transparent_only: false,
            blur_style: BlurStyle::Blur,
            blur_strength: 30,
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
            stamp: StampId::Smile,
            stamp_size: 32.0,
        }
    }
}

/// What the pointer is doing on the canvas.
#[derive(Clone, Debug)]
enum Drag {
    /// A shape/line drag: from `anchor` to current pos.
    Shape { anchor: Pos2 },
    /// Pencil/eraser/highlight strokes accumulate points.
    Freehand { points: Vec<Pos2> },
    /// Moving an existing object.
    Moving {
        id: AnnotationId,
        before: Annotation,
        last: Pos2,
    },
    /// Resizing via a corner handle (0..=7).
    Resizing {
        id: AnnotationId,
        before: Annotation,
        handle: usize,
    },
    /// Dragging a line/fancy/text anchor (false = `a`, true = `b`;
    /// `top` marks a top-edge corner of a text quad).
    Endpoint {
        id: AnnotationId,
        before: Annotation,
        which_b: bool,
        top: bool,
    },
    /// Dragging a text-box tail anchor.
    Tail {
        id: AnnotationId,
        before: Annotation,
    },
    /// Framing a marquee region (SelectArea tool).
    Marquee { anchor: Pos2 },
    /// Press inside a live marquee — becomes a pixel move once the drag
    /// travels (a plain click keeps the selection).
    MoveSel { start: Pos2 },
    /// Dragging one of the marquee's 8 resize anchors (0..=7, same
    /// order as object handles).
    SelResize { handle: usize },
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
    drag: Option<Drag>,
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
    /// Last pointer position in image coords (shape-drag end point).
    last_pointer: Option<Pos2>,
    /// The canvas rect of the document image (set every frame).
    img_rect: Rect,
    /// Canvas zoom actually used for painting (screen→image px for grabs).
    canvas_scale: f32,
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
        Self {
            doc,
            tab,
            title,
            undo,
            redo,
            tool: Tool::Select,
            opts: Opts::from_settings(prefs),
            selected: None,
            drag: None,
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
            last_pointer: None,
            img_rect: Rect::NOTHING,
            canvas_scale: 1.0,
        }
    }

    /// The resolved end-of-session, if any.
    pub fn outcome(&self) -> Option<Outcome> {
        self.outcome
    }

    /// Cancels the session — discarding edits is only silent when there
    /// are none; otherwise the user confirms through a warning box.
    /// Called by Annuler, Escape and the window's close control.
    pub fn request_cancel(&mut self) {
        if self.undo.is_empty() {
            self.outcome = Some(Outcome::Cancel);
        } else {
            self.confirm_quit = true;
        }
    }

    /// Applies an edit chronologically (kills redo, bumps revision).
    fn apply(&mut self, edit: AnnoEdit) {
        edit.apply(&mut self.doc);
        self.undo.push_back(edit);
        if self.undo.len() > ANNO_HISTORY_CAP {
            self.undo.pop_front();
        }
        self.redo.clear();
        self.doc.revision += 1;
    }

    pub fn undo_once(&mut self) {
        if let Some(edit) = self.undo.pop_back() {
            edit.revert(&mut self.doc);
            self.redo.push_back(edit);
            self.doc.revision += 1;
            self.selected = None;
        }
    }

    pub fn redo_once(&mut self) {
        if let Some(edit) = self.redo.pop_back() {
            edit.apply(&mut self.doc);
            self.undo.push_back(edit);
            self.doc.revision += 1;
            self.selected = None;
        }
    }

    /// Deletes the selected object (Suppr key and the X button).
    fn delete_selected(&mut self) {
        if let Some(id) = self.selected
            && let Some(idx) = self.doc.index_of(id)
        {
            let anno = self.doc.annotations[idx].clone();
            self.apply(AnnoEdit::Remove { anno, index: idx });
            self.selected = None;
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
        for a in self.doc.annotations.iter().rev() {
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
                AnnotationKind::Fancy(o) => o.width * 2.0 + 8.0,
                AnnotationKind::Freehand(o) | AnnotationKind::HighlightStroke(o) => {
                    o.width / 2.0 + 4.0
                }
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
                AnnotationKind::Fill(o) => {
                    // Hit the mask's actual coverage, not a rect.
                    let (x, y) = (p.x as i64, p.y as i64);
                    if x >= 0
                        && y >= 0
                        && x < o.mask.width() as i64
                        && y < o.mask.height() as i64
                        && o.mask.get_pixel(x as u32, y as u32).0[3] > 0
                    {
                        return Some(a.id);
                    }
                    continue;
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
                    AnnotationKind::Fancy(o) => {
                        if seg_distance(p, o.a, o.b) <= o.width * 2.0 + 10.0 {
                            return Some(a.id);
                        }
                    }
                    AnnotationKind::Text(_) => return Some(a.id),
                    AnnotationKind::Freehand(o) | AnnotationKind::HighlightStroke(o) => {
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
            .exact_size(58.0)
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
        let mut open = true;
        egui::Window::new(t!("draw.quit.title").to_string())
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(t!("draw.quit.body").to_string());
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button(t!("draw.quit.stay").to_string()).clicked() {
                        self.confirm_quit = false;
                    }
                    if ui.button(t!("draw.quit.discard").to_string()).clicked() {
                        self.confirm_quit = false;
                        self.outcome = Some(Outcome::Cancel);
                    }
                });
            });
        if !open {
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
        // Eyedropper mode: pressing `C` copies the hovered `#RRGGBB` and picks the colour.
        if self.tool == Tool::Eraser
            && self.opts.eraser_pick
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
            self.opts.eraser_color = c;
            self.opts.eraser_mode = EraserMode::Color;
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
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.opts.eraser_pick {
                self.opts.eraser_pick = false;
            } else if self.text_panel.is_some() {
                self.close_text_panel();
            } else if self.sel_area.is_some() {
                self.sel_area = None;
            } else {
                self.request_cancel();
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
            // Utility block: delete, undo, redo, zooms, print.
            egui::Grid::new("draw.util")
                .num_columns(2)
                .spacing(vec2(2.0, 2.0))
                .show(ui, |ui| {
                    let del = widgets::icon_button(ui, Icon::X, false)
                        .on_hover_text(t!("draw.del").to_string());
                    if del.clicked() {
                        self.delete_selected();
                    }
                    let undo = widgets::icon_button(ui, Icon::Undo2, false)
                        .on_hover_text(t!("menu.edit.undo").to_string());
                    if undo.clicked() {
                        self.undo_once();
                    }
                    ui.end_row();
                    let redo = widgets::icon_button(ui, Icon::Redo2, false)
                        .on_hover_text(t!("menu.edit.redo").to_string());
                    if redo.clicked() {
                        self.redo_once();
                    }
                    let zin = widgets::icon_button(ui, Icon::ZoomIn, false)
                        .on_hover_text(t!("vbar.zoom_in.tip").to_string());
                    if zin.clicked() {
                        self.bump_zoom(1.25);
                    }
                    ui.end_row();
                    let zout = widgets::icon_button(ui, Icon::ZoomOut, false)
                        .on_hover_text(t!("vbar.zoom_out.tip").to_string());
                    if zout.clicked() {
                        self.bump_zoom(1.0 / 1.25);
                    }
                    let one = widgets::icon_button(ui, Icon::Scaling, false)
                        .on_hover_text(t!("zoom.actual").to_string());
                    if one.clicked() {
                        self.zoom = Some(1.0);
                    }
                    ui.end_row();
                    let print = widgets::icon_button(ui, Icon::Printer, false)
                        .on_hover_text(t!("vbar.print.tip").to_string());
                    if print.clicked() {
                        let _ =
                            crate::export::file::print_image(&render::render_to_image(&self.doc));
                    }
                });
        });
    }

    /// Handles a tool click — Stamps opens the glyph popup instead.
    fn pick_tool(&mut self, tool: Tool, _ctx: &egui::Context) {
        if tool == Tool::Stamps {
            self.tool = tool;
            return;
        }
        if tool == Tool::InsertImage {
            self.insert_image();
            return;
        }
        self.tool = tool;
        self.sel_area = None;
        self.close_text_panel();
    }

    /// Zoom ×factor, starting from the current effective scale.
    fn bump_zoom(&mut self, factor: f32) {
        let cur = self.zoom.unwrap_or(1.0);
        self.zoom = Some((cur * factor).clamp(0.1, 8.0));
    }

    /// File picker for the Insert-image tool.
    fn insert_image(&mut self) {
        let file = rfd::FileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp"])
            .pick_file();
        if let Some(path) = file
            && let Ok(img) = image::open(&path)
        {
            let rgba = img.to_rgba8();
            let (w, h) = (rgba.width(), rgba.height());
            self.add(AnnotationKind::Picture(PicObj {
                rect: PhysicalRect {
                    x: 20,
                    y: 20,
                    width: w,
                    height: h,
                },
                image: rgba,
                opacity: 1.0,
                shadow: 0.0,
            }));
        }
    }

    // ── Marquee selection (SelectArea) ─────────────────────────────

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
                before,
                last: start,
            });
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
        ScrollArea::both()
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
            });
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
        let can_pick_existing =
            !matches!(
                self.tool,
                Tool::Eraser | Tool::Blur | Tool::Fill | Tool::SelectArea
            ) && !matches!(self.drag, Some(Drag::Shape { .. } | Drag::Freehand { .. }));
        let in_sel = self.sel_area.is_some_and(|s| s.rect.contains(img_p));
        let cursor = if on_handle {
            egui::CursorIcon::PointingHand
        } else if matches!(self.drag, Some(Drag::Moving { .. } | Drag::MoveSel { .. }))
            || (can_pick_existing && hit_id.is_some())
            || in_sel
        {
            egui::CursorIcon::Move
        } else if self.tool == Tool::Select {
            egui::CursorIcon::Default
        } else {
            egui::CursorIcon::Crosshair
        };
        ui.ctx().set_cursor_icon(cursor);

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
            self.start_drag(img_p);
        } else if resp.dragged() {
            self.continue_drag(img_p);
        } else if resp.clicked() && self.drag.is_none() {
            self.click(img_p, ui.ctx());
        }

        // Live preview while dragging.
        if let Some(drag) = &self.drag {
            self.preview(painter, drag, img_p, scale);
        }
    }

    /// Press: begin a creation drag or grab an object.
    fn start_drag(&mut self, p: Pos2) {
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
                    before,
                    which_b,
                    top,
                },
                Handle::Tail => Drag::Tail { id, before },
                Handle::Corner(h) => Drag::Resizing {
                    id,
                    before,
                    handle: h,
                },
            });
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
        if !matches!(
            self.tool,
            Tool::Eraser | Tool::Blur | Tool::Fill | Tool::SelectArea
        ) && let Some(id) = self.hit(p, self.canvas_scale)
            && let Some(before) = self.doc.annotation(id).cloned()
        {
            self.selected = Some(id);
            self.drag = Some(Drag::Moving {
                id,
                before,
                last: p,
            });
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
            Tool::Select
            | Tool::Text
            | Tool::Fill
            | Tool::Step
            | Tool::Stamps
            | Tool::InsertImage => {}
            Tool::Eraser => {
                if self.opts.eraser_pick {
                    self.opts.eraser_color = self.pixel_at(p);
                    self.opts.eraser_mode = EraserMode::Color;
                    self.opts.eraser_pick = false;
                } else {
                    self.drag = Some(Drag::Freehand { points: vec![p] });
                }
            }
            Tool::Pencil | Tool::HighlightLine => {
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
        match &mut self.drag {
            Some(Drag::Freehand { points }) => points.push(p),
            Some(Drag::Moving { id, last, .. }) => {
                let (dx, dy) = (p.x - last.x, p.y - last.y);
                *last = p;
                if let Some(a) = self.doc.annotation_mut(*id) {
                    a.kind.translate(dx, dy);
                    self.doc.revision += 1;
                }
            }
            Some(Drag::Resizing { id, handle, .. }) => {
                let h = *handle;
                if let Some(a) = self.doc.annotation_mut(*id) {
                    let mut r = a.kind.bounds();
                    resize_rect(&mut r, h, p);
                    a.kind.set_bounds(r);
                    self.doc.revision += 1;
                }
            }
            Some(Drag::Endpoint {
                id, which_b, top, ..
            }) => {
                let (wb, top) = (*which_b, *top);
                if let Some(a) = self.doc.annotation_mut(*id) {
                    match &mut a.kind {
                        AnnotationKind::Line(o) => {
                            drag_bezier_endpoint(
                                &mut o.a, &mut o.ka, &mut o.kb, &mut o.b, wb, top, p,
                            );
                        }
                        AnnotationKind::Fancy(o) => {
                            if wb {
                                o.b = p;
                            } else {
                                o.a = p;
                            }
                        }
                        AnnotationKind::Text(o) => {
                            drag_bezier_endpoint(
                                &mut o.a, &mut o.ka, &mut o.kb, &mut o.b, wb, top, p,
                            );
                        }
                        _ => {}
                    }
                    self.doc.revision += 1;
                }
            }
            Some(Drag::Tail { id, .. }) => {
                if let Some(a) = self.doc.annotation_mut(*id) {
                    if let AnnotationKind::TextBox(o) = &mut a.kind {
                        o.tail = Some(p);
                    }
                    self.doc.revision += 1;
                }
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
            Some(Drag::MoveSel { .. } | Drag::Shape { .. }) | None => {}
        }
    }

    /// Release: convert the drag into one chronological edit.
    fn finish_drag(&mut self, drag: Drag) {
        match drag {
            Drag::Moving { id, before, .. }
            | Drag::Resizing { id, before, .. }
            | Drag::Endpoint { id, before, .. }
            | Drag::Tail { id, before, .. } => {
                // Keep a minimum baseline span so the object never
                // collapses to a point.
                if let Some(a) = self.doc.annotation_mut(id)
                    && let AnnotationKind::Text(o) = &mut a.kind
                    && (o.b - o.a).length() < 16.0
                {
                    let d = unit_dir(o.a, o.b);
                    o.b = pos2(o.a.x + d.x * 16.0, o.a.y + d.y * 16.0);
                }
                if let Some(after) = self.doc.annotation(id).cloned() {
                    self.apply(AnnoEdit::Replace { before, after });
                    self.selected = Some(id);
                }
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
            Drag::MoveSel { .. } | Drag::SelResize { .. } => {}
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
            Tool::HighlightLine => AnnotationKind::HighlightStroke(FreeObj {
                points,
                color: self.opts.color,
                width: self.opts.width * 4.0,
                tip: TipShape::Round,
                dash: Dash::Solid,
                outline: 0.0,
                outline_color: Color32::WHITE,
                shadow: 0.0,
                eraser_mode: EraserMode::Color,
                soft: self.opts.soft,
                opacity: 0.45,
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
        let kind = match self.tool {
            Tool::Rect => AnnotationKind::Rect(RectObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: None,
                radius: o.radius,
                opacity: o.opacity,
                shadow: o.shadow,
            }),
            Tool::RectFill => AnnotationKind::Rect(RectObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: Some(o.fill_color),
                radius: o.radius,
                opacity: o.opacity,
                shadow: o.shadow,
            }),
            Tool::Ellipse => AnnotationKind::Ellipse(EllipseObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: None,
                opacity: o.opacity,
                shadow: o.shadow,
            }),
            Tool::EllipseFill => AnnotationKind::Ellipse(EllipseObj {
                rect: r,
                stroke: o.color,
                stroke_w: o.width,
                dash: o.dash,
                fill: Some(o.fill_color),
                opacity: o.opacity,
                shadow: o.shadow,
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
            Tool::FancyArrow => AnnotationKind::Fancy(FancyObj {
                a,
                b,
                color: o.color,
                width: o.width,
                dash: o.dash,
                outline: o.outline,
                opacity: o.opacity,
                shadow: o.shadow,
            }),
            Tool::HighlightArea => AnnotationKind::HighlightArea(AreaObj {
                rect: r,
                color: o.color,
                shape: o.shape,
                border: o.border_on.then_some(o.fill_color),
                dash: o.dash,
                soft: o.soft,
                opacity: o.opacity * 0.5,
            }),
            Tool::Blur => AnnotationKind::Blur(BlurObj {
                rect: r,
                style: o.blur_style,
                shape: o.shape,
                strength: o.blur_strength,
            }),
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
        self.add(kind);
    }

    /// A plain click (no drag): clicking an existing object selects it
    /// (any non-pixel tool); clicking empty canvas closes an open text
    /// panel or places a point tool.
    fn click(&mut self, p: Pos2, ctx: &egui::Context) {
        let had_panel = self.text_panel.is_some();
        if had_panel {
            self.close_text_panel();
        }
        if !matches!(
            self.tool,
            Tool::Eraser | Tool::Blur | Tool::Fill | Tool::SelectArea
        ) && let Some(id) = self.hit(p, self.canvas_scale)
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
            Tool::Eraser => {
                if self.opts.eraser_pick {
                    self.opts.eraser_color = self.pixel_at(p);
                    self.opts.eraser_mode = EraserMode::Color;
                    self.opts.eraser_pick = false;
                } else {
                    self.finish_freehand(vec![p]);
                }
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
            Tool::Fill => {
                let mask = render::compute_fill_mask(
                    &render::render_to_image(&self.doc),
                    p,
                    self.opts.tolerance,
                );
                self.add(AnnotationKind::Fill(FillObj {
                    origin: p,
                    color: self.opts.fill_color,
                    tolerance: self.opts.tolerance,
                    transparent: self.opts.transparent_only,
                    mask,
                }));
            }
            Tool::Stamps => {
                self.add(AnnotationKind::Stamp(StampObj {
                    pos: p,
                    stamp: self.opts.stamp,
                    color: self.opts.color,
                    size: self.opts.stamp_size,
                    opacity: self.opts.opacity,
                }));
            }
            _ => {}
        }
        let _ = ctx;
    }

    /// Which handle (if any) of `id` sits under `p`. `scale` converts
    /// the grab radius from screen px into image px so anchors stay
    /// reachable at any zoom.
    fn handle_at(&self, id: AnnotationId, p: Pos2, scale: f32) -> Option<Handle> {
        let a = self.doc.annotation(id)?;
        let grab = 10.0 / scale.max(0.05);
        match &a.kind {
            AnnotationKind::Line(o) => bezier_handle_at(o.a, o.ka, o.kb, o.b, p, grab),
            AnnotationKind::Fancy(o) => {
                if p.distance(o.b) < grab + o.width * 2.0 {
                    Some(Handle::Endpoint(true, false))
                } else if p.distance(o.a) < grab + o.width * 2.0 {
                    Some(Handle::Endpoint(false, false))
                } else {
                    None
                }
            }
            AnnotationKind::Text(o) => bezier_handle_at(o.a, o.ka, o.kb, o.b, p, grab),
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
    fn preview(&self, painter: &egui::Painter, drag: &Drag, cur: Pos2, scale: f32) {
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
                Tool::HighlightLine => (
                    o.color.gamma_multiply(0.45),
                    (o.width * 4.0 * scale).max(2.0),
                    0.0,
                ),
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
                Tool::Rect
                | Tool::RectFill
                | Tool::Ellipse
                | Tool::EllipseFill
                | Tool::HighlightArea
                | Tool::Blur
                | Tool::TextBoxRect
                | Tool::TextBoxOval => {
                    let r = Rect::from_two_pos(ca, cb);
                    match self.tool {
                        Tool::Ellipse | Tool::EllipseFill => {
                            painter.add(egui::epaint::EllipseShape::stroke(
                                r.center(),
                                r.size() / 2.0,
                                Stroke::new(1.5, accent),
                            ));
                        }
                        _ => {
                            painter.rect_stroke(
                                r,
                                CornerRadius::same(
                                    if matches!(
                                        self.tool,
                                        Tool::TextBoxRect | Tool::TextBoxOval | Tool::HighlightArea
                                    ) {
                                        o.radius as u8
                                    } else {
                                        0
                                    },
                                ),
                                Stroke::new(1.5, accent),
                                StrokeKind::Inside,
                            );
                        }
                    }
                }
                Tool::Arrow | Tool::FancyArrow => {
                    painter.line_segment([ca, cb], Stroke::new(o.width.max(1.0), o.color));
                }
                _ => {}
            }
        }
    }

    /// Selection chrome + handles + text edit overlay.
    fn draw_overlay(&mut self, painter: &egui::Painter, img_rect: Rect, scale: f32) {
        self.img_rect = img_rect;
        let accent = theme::tokens(painter.ctx()).accent;
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
        let Some(a) = self.doc.annotation(id).cloned() else {
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
        let is_bezier = matches!(a.kind, AnnotationKind::Text(_) | AnnotationKind::Line(_));
        if !is_bezier {
            painter.rect_stroke(
                cb,
                CornerRadius::ZERO,
                Stroke::new(1.0, accent),
                StrokeKind::Outside,
            );
        }
        // Handles.
        match &a.kind {
            AnnotationKind::Fancy(FancyObj { a, b, .. }) => {
                for p in [*a, *b] {
                    painter.rect_filled(
                        Rect::from_center_size(to_canvas(p), vec2(8.0, 8.0)),
                        CornerRadius::same(2),
                        Color32::WHITE,
                    );
                    painter.rect_stroke(
                        Rect::from_center_size(to_canvas(p), vec2(8.0, 8.0)),
                        CornerRadius::same(2),
                        Stroke::new(1.0, accent),
                        StrokeKind::Inside,
                    );
                }
            }
            AnnotationKind::Line(o) => {
                paint_bezier_chrome(painter, &to_canvas, o.a, o.ka, o.kb, o.b, accent, false);
            }
            AnnotationKind::Text(o) => {
                paint_bezier_chrome(painter, &to_canvas, o.a, o.ka, o.kb, o.b, accent, true);
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
                AnnotationKind::Fancy(_) => t!("draw.tool.fancy"),
                AnnotationKind::Freehand(_) => t!("draw.tool.pencil"),
                AnnotationKind::HighlightStroke(_) => t!("draw.tool.hline"),
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
                AnnotationKind::Stamp(_) => t!("draw.tool.stamps"),
                AnnotationKind::Picture(_) => t!("draw.tab.image"),
                AnnotationKind::Fill(_) => t!("draw.tool.fill"),
                AnnotationKind::Blur(_) => t!("draw.tool.blur"),
            };
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
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // Tab header chip (two clickable mode tabs for the step marker).
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
            });
            // Tab body: reserve the right-hand buttons first, then fill
            // the remaining strip with a vertically-centred row so every
            // label, dropdown, swatch, slider and button shares one axis.
            let row_h = 26.0;
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), row_h),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    ui.spacing_mut().interact_size.y = 22.0;
                    ui.add_space(6.0);
                    if widgets::secondary_button(ui, &t!("draw.cancel")).clicked() {
                        self.request_cancel();
                    }
                    if widgets::primary_button(ui, &t!("draw.ok")).clicked() {
                        self.outcome = Some(Outcome::Ok);
                    }
                    let rest = ui.available_size();
                    ui.allocate_ui_with_layout(
                        rest,
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.spacing_mut().interact_size.y = 22.0;
                            ui.add_space(8.0);
                            self.bottom_content(ui, shared);
                        },
                    );
                },
            );
        });
    }

    /// Content of the bottom tab: a live marquee shows its pixel ops,
    /// a selected object its properties (regardless of the armed tool);
    /// otherwise the armed tool's defaults.
    fn bottom_content(&mut self, ui: &mut Ui, shared: &Shared) {
        if self.sel_area.is_some() {
            self.sel_options(ui);
        } else if self.selected.is_some() {
            self.selected_options(ui, shared);
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
        color_btn(ui, &mut self.opts.fill_color);
        if ui.button(t!("draw.opt.fill_c")).clicked() {
            self.sel_fill(self.opts.fill_color);
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
            Tool::FancyArrow => {
                w_edit(ui, t!("draw.opt.width"), &mut o.width, 1.0..=20.0);
                dash_combo(ui, &mut o.dash);
                color_btn(ui, &mut o.color);
                slider(ui, t!("draw.opt.outline"), &mut o.outline, 0.0..=6.0);
                shadow_opacity(ui, &mut o.shadow, &mut o.opacity);
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
            Tool::Eraser => {
                ui.label(t!("draw.opt.color"));
                if color_btn(ui, &mut o.eraser_color).changed() {
                    o.eraser_mode = EraserMode::Color;
                    o.eraser_pick = false;
                }
                if widgets::icon_button(ui, Icon::Pipette, o.eraser_pick)
                    .on_hover_text(t!("draw.opt.pick").to_string())
                    .clicked()
                {
                    o.eraser_pick = !o.eraser_pick;
                }
                w_edit(ui, t!("draw.opt.size"), &mut o.eraser_size, 1.0..=80.0);

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
            }
            Tool::Rect | Tool::RectFill | Tool::Ellipse | Tool::EllipseFill => {
                w_edit(ui, t!("draw.opt.width"), &mut o.width, 0.0..=20.0);
                ui.label(t!("draw.opt.border"));
                color_btn(ui, &mut o.color);
                if matches!(self.tool, Tool::RectFill | Tool::EllipseFill) {
                    ui.label(t!("draw.opt.fill_c"));
                    color_btn(ui, &mut o.fill_color);
                }
                if self.tool == Tool::Rect || self.tool == Tool::RectFill {
                    slider(ui, t!("draw.opt.radius"), &mut o.radius, 0.0..=40.0);
                }
                dash_combo(ui, &mut o.dash);
                shadow_opacity(ui, &mut o.shadow, &mut o.opacity);
            }
            Tool::HighlightLine => {
                w_edit(ui, t!("draw.opt.width"), &mut o.width, 1.0..=30.0);
                color_btn(ui, &mut o.color);
                ui.checkbox(&mut o.soft, t!("draw.opt.soft").to_string());
            }
            Tool::HighlightArea => {
                color_btn(ui, &mut o.color);
                shape_combo(ui, &mut o.shape);
                ui.checkbox(&mut o.border_on, t!("draw.opt.border").to_string());
                if o.border_on {
                    color_btn(ui, &mut o.fill_color);
                    dash_combo(ui, &mut o.dash);
                }
                ui.checkbox(&mut o.soft, t!("draw.opt.soft").to_string());
                slider(ui, t!("draw.opt.opacity"), &mut o.opacity, 0.1..=1.0);
            }
            Tool::Fill => {
                color_btn(ui, &mut o.fill_color);
                let mut tol = f32::from(o.tolerance);
                if slider(ui, t!("draw.opt.tolerance"), &mut tol, 0.0..=255.0).changed() {
                    o.tolerance = tol as u8;
                }
                ui.checkbox(
                    &mut o.transparent_only,
                    t!("draw.opt.transparent").to_string(),
                );
            }
            Tool::Blur => {
                enum_combo(
                    ui,
                    "draw.opt.style",
                    o.blur_style,
                    &[
                        (BlurStyle::Blur, t!("draw.opt.blur")),
                        (BlurStyle::Pixelate, t!("draw.opt.pixelate")),
                    ],
                    |v| o.blur_style = v,
                );
                let mut bs = f32::from(o.blur_strength);
                if slider(ui, t!("draw.opt.strength"), &mut bs, 1.0..=100.0).changed() {
                    o.blur_strength = bs as u8;
                }
                shape_combo(ui, &mut o.shape);
            }
            Tool::Text => {
                let mut ch = false;
                ui.label(t!("draw.opt.color"));
                ch |= color_btn(ui, &mut o.color).changed();
                ui.label(t!("draw.opt.size"));
                ch |= font_size_combo(ui, "draw.bar.tsize", &mut o.font_size);
                ch |= ui
                    .toggle_value(&mut o.bold, "B")
                    .on_hover_text(t!("draw.opt.bold"))
                    .changed();
                ch |= ui
                    .toggle_value(&mut o.italic, "I")
                    .on_hover_text(t!("draw.opt.italic"))
                    .changed();
                ui.label(t!("draw.opt.outline"));
                ch |= outline_combo(ui, "draw.bar.toutline", &mut o.outline_w);
                ch |= color_btn(ui, &mut o.outline_color).changed();
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
                ui.label(t!("draw.opt.fg"));
                color_btn(ui, &mut o.outline_color);
                ui.label(t!("draw.opt.bg"));
                color_btn(ui, &mut o.fill_color);
                // Outline: width (1..10) + colour.
                ui.label(t!("draw.opt.outline"));
                outline_combo(ui, "draw.bar.boxw", &mut o.width);
                color_btn(ui, &mut o.color);
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
            Tool::Stamps => {
                // The stamp glyph picker.
                for s in StampId::ALL {
                    let resp = widgets::icon_button(ui, icons::stamp_icon(s), self.opts.stamp == s)
                        .on_hover_text(s.label());
                    if resp.clicked() {
                        self.opts.stamp = s;
                    }
                }
                ui.separator();
                color_btn(ui, &mut self.opts.color);
                slider(
                    ui,
                    t!("draw.opt.size"),
                    &mut self.opts.stamp_size,
                    8.0..=96.0,
                );
            }
            Tool::InsertImage => {
                ui.label(t!("draw.opt.insert_hint"));
            }
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
                    changed |=
                        w_edit(ui, t!("draw.opt.width"), &mut o.stroke_w, 0.0..=20.0).changed();
                    changed |= color_btn(ui, &mut o.stroke).changed();
                    if o.fill.is_some() {
                        ui.label(t!("draw.opt.fill_c"));
                        let mut c = o.fill.unwrap_or(Color32::TRANSPARENT);
                        if color_btn(ui, &mut c).changed() {
                            o.fill = Some(c);
                            changed = true;
                        }
                    }
                    changed |= dash_combo(ui, &mut o.dash);
                    changed |= shadow_opacity(ui, &mut o.shadow, &mut o.opacity);
                }
                AnnotationKind::Ellipse(o) => {
                    changed |=
                        w_edit(ui, t!("draw.opt.width"), &mut o.stroke_w, 0.0..=20.0).changed();
                    changed |= color_btn(ui, &mut o.stroke).changed();
                    if o.fill.is_some() {
                        ui.label(t!("draw.opt.fill_c"));
                        let mut c = o.fill.unwrap_or(Color32::TRANSPARENT);
                        if color_btn(ui, &mut c).changed() {
                            o.fill = Some(c);
                            changed = true;
                        }
                    }
                    changed |= dash_combo(ui, &mut o.dash);
                    changed |= shadow_opacity(ui, &mut o.shadow, &mut o.opacity);
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
                AnnotationKind::Fancy(o) => {
                    changed |= w_edit(ui, t!("draw.opt.width"), &mut o.width, 1.0..=30.0).changed();
                    changed |= color_btn(ui, &mut o.color).changed();
                    changed |= shadow_opacity(ui, &mut o.shadow, &mut o.opacity);
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
                AnnotationKind::HighlightStroke(o) => {
                    changed |= w_edit(ui, t!("draw.opt.width"), &mut o.width, 1.0..=30.0).changed();
                    changed |= color_btn(ui, &mut o.color).changed();
                    changed |= opacity_pct_slider(ui, &mut o.opacity);
                }
                AnnotationKind::HighlightArea(o) => {
                    changed |= color_btn(ui, &mut o.color).changed();
                    changed |=
                        slider(ui, t!("draw.opt.opacity"), &mut o.opacity, 0.1..=1.0).changed();
                }
                AnnotationKind::Text(o) => {
                    // 1) Couleurs · 2) Taille (8..72) · 3) Bold · 4) Italic ·
                    // 5) Contour (0..10) · 6) Couleur contour · 7) Opacité ·
                    // 8) Case Ombre (9=OK, 10=Annuler are on the bar's right).
                    let mut style_ch = false;
                    ui.label(t!("draw.opt.color"));
                    if color_btn(ui, &mut o.color).changed() {
                        changed = true;
                        style_ch = true;
                    }
                    ui.label(t!("draw.opt.size"));
                    if font_size_combo(ui, "draw.sel.tsize", &mut o.size) {
                        changed = true;
                        style_ch = true;
                    }
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
                    ui.label(t!("draw.opt.outline"));
                    if outline_combo(ui, "draw.sel.toutline", &mut o.outline) {
                        changed = true;
                        style_ch = true;
                    }
                    if color_btn(ui, &mut o.outline_color).changed() {
                        changed = true;
                        style_ch = true;
                    }
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
                    ui.label(t!("draw.opt.fg"));
                    changed |= color_btn(ui, &mut o.fg).changed();
                    ui.label(t!("draw.opt.bg"));
                    changed |= color_btn(ui, &mut o.bg).changed();
                    ui.label(t!("draw.opt.outline"));
                    changed |= outline_combo(ui, "draw.sel.boxw", &mut o.border_w);
                    changed |= color_btn(ui, &mut o.border).changed();
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
                AnnotationKind::Stamp(o) => {
                    changed |= color_btn(ui, &mut o.color).changed();
                    changed |= slider(ui, t!("draw.opt.size"), &mut o.size, 8.0..=96.0).changed();
                }
                AnnotationKind::Picture(o) => {
                    changed |= opacity_pct_slider(ui, &mut o.opacity);
                    let mut sh_on = o.shadow > 0.0;
                    if ui.checkbox(&mut sh_on, t!("draw.opt.shadow")).changed() {
                        o.shadow = if sh_on { 3.0 } else { 0.0 };
                        changed = true;
                    }
                }
                AnnotationKind::Fill(o) => {
                    changed |= color_btn(ui, &mut o.color).changed();
                }
                AnnotationKind::Blur(o) => {
                    let mut s = f32::from(o.strength);
                    if slider(ui, t!("draw.opt.strength"), &mut s, 1.0..=100.0).changed() {
                        o.strength = s as u8;
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
    ui.label(label.into());
    ui.add(
        egui::DragValue::new(v)
            .range(range)
            .speed(0.2)
            .max_decimals(1),
    )
}

/// Label + slider.
fn slider(
    ui: &mut Ui,
    label: std::borrow::Cow<'static, str>,
    v: &mut f32,
    range: std::ops::RangeInclusive<f32>,
) -> Response {
    ui.label(label);
    ui.add(
        egui::Slider::new(v, range)
            .show_value(false)
            .trailing_fill(true),
    )
}

/// Colour swatch button with an explicit border so light swatches
/// (e.g. white outline) remain visible on the light theme.
fn color_btn(ui: &mut Ui, c: &mut Color32) -> Response {
    let border = theme::tokens(ui.ctx()).field_border;
    let id = ui.next_auto_id();
    let mut hsva: egui::epaint::Hsva = ui
        .ctx()
        .data_mut(|d| d.get_temp(id))
        .filter(|cached: &egui::epaint::Hsva| Color32::from(*cached) == *c)
        .unwrap_or_else(|| egui::epaint::Hsva::from(*c));
    let old_h = hsva.h;
    let r = egui::widgets::color_picker::color_edit_button_hsva(
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
}

/// Shadow + opacity sliders — the recurring FS pair.
fn shadow_opacity(ui: &mut Ui, shadow: &mut f32, opacity: &mut f32) -> bool {
    let mut changed = slider(ui, t!("draw.opt.shadow"), shadow, 0.0..=8.0).changed();
    changed |= slider(ui, t!("draw.opt.opacity"), opacity, 0.1..=1.0).changed();
    changed
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
    ui.label(t!("draw.opt.color"));
    changed |= color_btn(ui, color).changed();
    changed |= w_edit(ui, t!("draw.opt.width"), width, 1.0..=20.0).changed();

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
        changed = true;
    }

    changed |= dash_combo(ui, dash);

    ui.label(t!("draw.opt.outline"));
    changed |= outline_combo(ui, salt, outline);
    changed |= color_btn(ui, outline_color).changed();

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
    ui.label(t!("draw.opt.color"));
    changed |= color_btn(ui, color).changed();
    changed |= w_edit(ui, t!("draw.opt.width"), width, 1.0..=30.0).changed();
    ui.label(t!("draw.opt.style"));
    changed |= tip_combo(ui, tip);
    changed |= dash_combo(ui, dash);
    ui.label(t!("draw.opt.outline"));
    changed |= outline_combo(ui, salt, outline);
    changed |= color_btn(ui, outline_color).changed();
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
    if ui.add(dv).changed() {
        *count = n.max(1) as u32;
        changed = true;
    }
    if mode == StepMode::Letters
        && ui
            .checkbox(lower, t!("draw.opt.lower").to_string())
            .changed()
    {
        changed = true;
    }

    ui.label(t!("draw.opt.bg"));
    changed |= color_btn(ui, bg).changed();
    ui.label(t!("draw.opt.fg"));
    changed |= color_btn(ui, fg).changed();

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
        changed = true;
    }

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
        changed = true;
    }

    ui.label(t!("draw.opt.outline"));
    changed |= outline_combo(ui, salt, ring);
    changed |= color_btn(ui, ring_color).changed();

    changed |= opacity_pct_slider(ui, opacity);
    changed |= ui
        .checkbox(shadow_on, t!("draw.opt.shadow").to_string())
        .changed();
    changed
}
