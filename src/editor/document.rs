//! The editor document: a captured image plus non-destructive annotations.
//!
//! Annotations are self-contained objects (geometry + full style) so they
//! can be re-selected, edited, reordered and undone — the draw window's
//! object model. All coordinates are in the capture's physical pixels.

use serde::{Deserialize, Serialize};

use egui::{Color32, Pos2};

use crate::capture::{Capture, PhysicalRect};

/// Unique id of an annotation inside a [`Document`].
pub type AnnotationId = u64;

/// Stroke dashing shared by every outlined object ("Tirets" option).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[allow(clippy::enum_variant_names)] // the variants ARE the dash styles (Dash, Dot, DashDot)
pub enum Dash {
    /// Unbroken stroke.
    #[default]
    Solid,
    /// Dashes.
    Dash,
    /// Dots.
    Dot,
    /// Alternating dash and dot.
    DashDot,
}

/// Line-end glyph for the straight-line tool (the "Fin" dropdown).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnd {
    /// Plain cut end.
    #[default]
    None,
    /// Open arrow head.
    Arrow,
    /// Filled dot.
    Dot,
    /// Ring (hollow circle).
    Ring,
}

/// Outline shape of filled-area objects (the "Forme" icon row).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum AreaShape {
    /// Sharp rectangle.
    #[default]
    Rect,
    /// Rounded rectangle.
    Round,
    /// Ellipse inscribed in the bounds.
    Ellipse,
}

/// The blur tool's two modes ("Style" icons).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlurStyle {
    /// Mosaic / big pixels.
    #[default]
    Pixelate,
    /// Gaussian softening.
    Blur,
}

/// Freehand tip shape (the pencil/eraser "Forme" icons).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TipShape {
    /// Square brush footprint.
    Square,
    /// Round brush footprint.
    #[default]
    Round,
}

/// Step-marker size presets ("Taille": Petit/Moyen/Grand).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepSize {
    /// Small badge.
    Small,
    /// Medium badge.
    #[default]
    Medium,
    /// Large badge.
    Large,
}

/// Step-marker numbering ("Etape 1.2.3" / "Etape A.B.C" tabs).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepMode {
    /// 1, 2, 3, …
    #[default]
    Numbers,
    /// A, B, C, …
    Letters,
}

/// Step badge style (the "Style" dropdown).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStyle {
    /// Filled circle.
    #[default]
    Circle,
    /// Filled rounded square.
    Square,
}

/// Text alignment inside a text box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    /// Left.
    #[default]
    Left,
    /// Center.
    Center,
    /// Right.
    Right,
    /// Justified (word gaps spread to fill the line width).
    Justify,
}

/// Vertical text alignment inside a box.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BoxVAlign {
    /// Vertically centred (default).
    #[default]
    Center,
    /// Aligned to the top of the box.
    Top,
    /// Aligned to the bottom of the box.
    Bottom,
}

/// One icon stamp offered by the Émoticônes menu — rendered as a
/// Lucide glyph recoloured with the object's colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StampId {
    /// Arrow pointer.
    ArrowCursor,
    /// Open hand.
    Hand,
    /// I-beam text caret.
    TextCursor,
    /// Plus sign (crosshair).
    Cross,
    /// Left mouse button.
    ClickLeft,
    /// Right mouse button.
    ClickRight,
    /// Waiting spinner.
    Busy,
    /// Ruler / dimension arrows.
    Dimension,
    /// Information badge.
    Info,
    /// Question badge.
    Question,
    /// Error cross badge.
    Error,
    /// Green check.
    Ok,
    /// Red cross.
    Cancel,
    /// Note / light bulb.
    Note,
    /// Smiley.
    Smile,
}

/// Stroked/filled rectangle (the □ and ■ tools — `fill` distinguishes
/// them).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RectObj {
    /// Bounds (physical px).
    pub rect: PhysicalRect,
    /// Border colour.
    pub stroke: Color32,
    /// Border width px (`0` = no border).
    pub stroke_w: f32,
    /// Border dashing.
    pub dash: Dash,
    /// Fill colour, `None` = transparent.
    pub fill: Option<Color32>,
    /// Corner radius for the rounded look (FS "Contour" corner icon).
    pub radius: f32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px (`0` = no shadow).
    pub shadow: f32,
}

/// Stroked/filled ellipse inside `rect` (○ / ● tools).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EllipseObj {
    /// Bounds (physical px).
    pub rect: PhysicalRect,
    /// Border colour.
    pub stroke: Color32,
    /// Border width px.
    pub stroke_w: f32,
    /// Border dashing.
    pub dash: Dash,
    /// Fill colour, `None` = transparent.
    pub fill: Option<Color32>,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// Straight segment with styled ends (the ↗ arrow tool).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LineObj {
    /// Start point.
    pub a: Pos2,
    /// End point.
    pub b: Pos2,
    /// Stroke colour.
    pub color: Color32,
    /// Stroke width px.
    pub width: f32,
    /// Dashing.
    pub dash: Dash,
    /// Glyph at `a`.
    pub start: LineEnd,
    /// Glyph at `b`.
    pub end: LineEnd,
    /// Outline stroke around the whole line (FS "Contour").
    pub outline: f32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// The ⬆ fancy arrow: a tapered filled shape from `a` to `b`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FancyObj {
    /// Tail point.
    pub a: Pos2,
    /// Head point.
    pub b: Pos2,
    /// Fill colour.
    pub color: Color32,
    /// Shaft width px.
    pub width: f32,
    /// Dashing of the outline (solid by default).
    pub dash: Dash,
    /// Outline stroke around the shape.
    pub outline: f32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// A freehand polyline — pencil, eraser (opaque colour) and line
/// highlight (translucent colour) all share this payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FreeObj {
    /// Polyline points in order.
    pub points: Vec<Pos2>,
    /// Stroke colour.
    pub color: Color32,
    /// Stroke width px.
    pub width: f32,
    /// Brush tip shape.
    pub tip: TipShape,
    /// Soft edge (blurred footprint) vs hard edge.
    pub soft: bool,
    /// Object opacity 0–1 — the highlight tool keeps ~0.5.
    pub opacity: f32,
}

/// Translucent filled area (the zone-highlighter tool).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AreaObj {
    /// Bounds.
    pub rect: PhysicalRect,
    /// Fill colour (rendered translucent through `opacity`).
    pub color: Color32,
    /// Outline shape.
    pub shape: AreaShape,
    /// Optional border colour.
    pub border: Option<Color32>,
    /// Border dashing.
    pub dash: Dash,
    /// Soft edge.
    pub soft: bool,
    /// Object opacity 0–1.
    pub opacity: f32,
}

/// Plain text anchored at `pos` (the "A" tool).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextObj {
    /// Baseline start — the left GREY extremity anchor.
    pub a: Pos2,
    /// Baseline end — the right GREY extremity anchor.
    pub b: Pos2,
    /// Left GREEN knob: a free lever anchor; dragging it pivots the
    /// baseline around `b`. Sits on the line at the text edge at rest.
    pub ka: Pos2,
    /// Right GREEN knob (pivot on `a`).
    pub kb: Pos2,
    /// Text content (multi-line).
    pub content: String,
    /// Font family key resolved by [`super::render::font`] ("Inter",
    /// "Arial", …). Unknown names fall back to Inter.
    pub font: String,
    /// Text colour.
    pub color: Color32,
    /// Font size px.
    pub size: f32,
    /// Bold face.
    pub bold: bool,
    /// Italic (faux-slanted render).
    pub italic: bool,
    /// Per-line horizontal alignment within the baseline span.
    pub align: TextAlign,
    /// Outline width around glyphs (FS "Contour"), `0` = none.
    pub outline: f32,
    /// Outline colour.
    pub outline_color: Color32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// A text box (rect or oval callout) with wrapped text inside.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BoxObj {
    /// The box bounds.
    pub rect: PhysicalRect,
    /// `false` = rectangular box, `true` = oval.
    pub oval: bool,
    /// Text content.
    pub content: String,
    /// Box background colour.
    pub bg: Color32,
    /// Border colour.
    pub border: Color32,
    /// Border width px.
    pub border_w: f32,
    /// Corner radius (rect box only).
    pub radius: f32,
    /// Text colour.
    pub fg: Color32,
    /// Font size px.
    pub size: f32,
    /// Bold face.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
    /// Text alignment.
    pub align: TextAlign,
    /// Vertical text alignment inside the box.
    pub valign: BoxVAlign,
    /// Extra line spacing px ("Espace de ligne").
    pub line_gap: f32,
    /// Callout tail apex — `Some` draws the speech-bubble pointer.
    pub tail: Option<Pos2>,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// A numbered/lettered step badge (the "①" tool).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepObj {
    /// Badge centre.
    pub pos: Pos2,
    /// The displayed counter (number or letter index).
    pub count: u32,
    /// Numbering mode of this marker.
    pub mode: StepMode,
    /// Badge style.
    pub style: StepStyle,
    /// Size preset.
    pub size: StepSize,
    /// Badge background.
    pub bg: Color32,
    /// Counter glyph colour.
    pub fg: Color32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// A Lucide icon stamp (the 😊 / cursor tools).
impl StampId {
    /// Every stamp, in popup order (cursors, badges, note, emoji).
    pub const ALL: [StampId; 15] = [
        StampId::ArrowCursor,
        StampId::Hand,
        StampId::TextCursor,
        StampId::Cross,
        StampId::ClickLeft,
        StampId::ClickRight,
        StampId::Busy,
        StampId::Dimension,
        StampId::Info,
        StampId::Question,
        StampId::Error,
        StampId::Ok,
        StampId::Cancel,
        StampId::Note,
        StampId::Smile,
    ];

    /// Translated label for tooltips and the stamp menu.
    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::ArrowCursor => rust_i18n::t!("stamp.arrow_cursor"),
            Self::Hand => rust_i18n::t!("stamp.hand"),
            Self::TextCursor => rust_i18n::t!("stamp.text_cursor"),
            Self::Cross => rust_i18n::t!("stamp.cross"),
            Self::ClickLeft => rust_i18n::t!("stamp.click_left"),
            Self::ClickRight => rust_i18n::t!("stamp.click_right"),
            Self::Busy => rust_i18n::t!("stamp.busy"),
            Self::Dimension => rust_i18n::t!("stamp.dimension"),
            Self::Info => rust_i18n::t!("stamp.info"),
            Self::Question => rust_i18n::t!("stamp.question"),
            Self::Error => rust_i18n::t!("stamp.error"),
            Self::Ok => rust_i18n::t!("stamp.ok"),
            Self::Cancel => rust_i18n::t!("stamp.cancel"),
            Self::Note => rust_i18n::t!("stamp.note"),
            Self::Smile => rust_i18n::t!("stamp.smile"),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StampObj {
    /// Top-left anchor.
    pub pos: Pos2,
    /// Which stamp.
    pub stamp: StampId,
    /// Stamp colour (recolours the glyph).
    pub color: Color32,
    /// Stamp size px.
    pub size: f32,
    /// Object opacity 0–1.
    pub opacity: f32,
}

/// An inserted bitmap (the 🖼 tool) — pixels stored on the object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PicObj {
    /// Placement bounds (the image is stretched to fit).
    pub rect: PhysicalRect,
    /// The inserted pixels — persisted as a PNG blob.
    #[serde(with = "crate::store::ser::rgba_png")]
    pub image: image::RgbaImage,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Drop-shadow offset px.
    pub shadow: f32,
}

/// A flood fill (the bucket tool) — `mask` is precomputed at creation:
/// the connected region under `origin` within `tolerance`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FillObj {
    /// Seed point (physical px).
    pub origin: Pos2,
    /// Fill colour.
    pub color: Color32,
    /// Colour tolerance 0–100.
    pub tolerance: u8,
    /// `true` erases to transparent instead of painting.
    pub transparent: bool,
    /// Coverage mask, full-document size (alpha = coverage). Computed
    /// once when the object is placed, stored for rendering — persisted
    /// as a PNG blob.
    #[serde(with = "crate::store::ser::rgba_png")]
    pub mask: image::RgbaImage,
}

/// A blur/pixelate region (parametric — rendered live from the base).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlurObj {
    /// Region bounds.
    pub rect: PhysicalRect,
    /// Pixelate vs gaussian.
    pub style: BlurStyle,
    /// Clip shape.
    pub shape: AreaShape,
    /// Effect strength 0–100.
    pub strength: u8,
}

/// Geometry/content of one annotation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum AnnotationKind {
    /// □ / ■ rectangle.
    Rect(RectObj),
    /// ○ / ● ellipse.
    Ellipse(EllipseObj),
    /// ↗ straight line with end glyphs.
    Line(LineObj),
    /// ⬆ tapered arrow.
    Fancy(FancyObj),
    /// Pencil / eraser freehand stroke.
    Freehand(FreeObj),
    /// Translucent line highlight.
    HighlightStroke(FreeObj),
    /// Translucent zone highlight.
    HighlightArea(AreaObj),
    /// "A" plain text.
    Text(TextObj),
    /// Speech-bubble text boxes.
    TextBox(BoxObj),
    /// ① step badge.
    Step(StepObj),
    /// Icon stamp.
    Stamp(StampObj),
    /// Inserted bitmap.
    Picture(PicObj),
    /// Bucket fill region.
    Fill(FillObj),
    /// Blur/pixelate region.
    Blur(BlurObj),
}

/// One annotation object.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Annotation {
    /// Identity for undo/redo, hit-testing and selection.
    pub id: AnnotationId,
    /// Shape and payload.
    pub kind: AnnotationKind,
}

impl AnnotationKind {
    /// Bounding box in document physical px — hit-test + handle frame.
    pub fn bounds(&self) -> PhysicalRect {
        match self {
            Self::Rect(o) => o.rect,
            Self::Ellipse(o) => o.rect,
            Self::Line(o) => rect_of_points(o.a, o.b),
            Self::Fancy(o) => rect_of_points(o.a, o.b),
            Self::Freehand(o) | Self::HighlightStroke(o) => points_bounds(&o.points),
            Self::HighlightArea(o) => o.rect,
            Self::Text(o) => {
                // Ribbon above the cubic Bézier baseline (`a`, `ka`, `kb`, `b`).
                let (_, h) = crate::editor::render::text_extent(o);
                let mut x1 = f32::INFINITY;
                let mut y1 = f32::INFINITY;
                let mut x2 = f32::NEG_INFINITY;
                let mut y2 = f32::NEG_INFINITY;
                for (_, p, d) in crate::editor::render::bezier_table(o, 24) {
                    for q in [p, egui::pos2(p.x + d.y * h, p.y - d.x * h)] {
                        x1 = x1.min(q.x);
                        y1 = y1.min(q.y);
                        x2 = x2.max(q.x);
                        y2 = y2.max(q.y);
                    }
                }
                let xs = [x1, x2, x1, x2];
                let ys = [y1, y2, y1, y2];
                let (x1, x2) = (
                    xs.iter().cloned().fold(f32::INFINITY, f32::min),
                    xs.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                );
                let (y1, y2) = (
                    ys.iter().cloned().fold(f32::INFINITY, f32::min),
                    ys.iter().cloned().fold(f32::NEG_INFINITY, f32::max),
                );
                PhysicalRect {
                    x: x1 as i32,
                    y: y1 as i32,
                    width: (x2 - x1).max(1.0) as u32,
                    height: (y2 - y1).max(1.0) as u32,
                }
            }
            Self::TextBox(o) => o.rect,
            Self::Step(o) => {
                let r = step_radius(o.size) as i32;
                PhysicalRect {
                    x: o.pos.x as i32 - r,
                    y: o.pos.y as i32 - r,
                    width: (r * 2) as u32,
                    height: (r * 2) as u32,
                }
            }
            Self::Stamp(o) => PhysicalRect {
                x: o.pos.x as i32,
                y: o.pos.y as i32,
                width: o.size as u32,
                height: o.size as u32,
            },
            Self::Picture(o) => o.rect,
            Self::Fill(o) => PhysicalRect {
                x: 0,
                y: 0,
                width: o.mask.width(),
                height: o.mask.height(),
            },
            Self::Blur(o) => o.rect,
        }
    }

    /// Moves the object by `(dx, dy)` physical px.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        let shift = |p: &mut Pos2| {
            p.x += dx;
            p.y += dy;
        };
        let shift_rect = |r: &mut PhysicalRect| {
            r.x += dx as i32;
            r.y += dy as i32;
        };
        match self {
            Self::Rect(o) => shift_rect(&mut o.rect),
            Self::Ellipse(o) => shift_rect(&mut o.rect),
            Self::Line(o) => {
                shift(&mut o.a);
                shift(&mut o.b);
            }
            Self::Fancy(o) => {
                shift(&mut o.a);
                shift(&mut o.b);
            }
            Self::Freehand(o) | Self::HighlightStroke(o) => {
                for p in &mut o.points {
                    shift(p);
                }
            }
            Self::HighlightArea(o) => shift_rect(&mut o.rect),
            Self::Text(o) => {
                shift(&mut o.a);
                shift(&mut o.b);
                shift(&mut o.ka);
                shift(&mut o.kb);
            }
            Self::TextBox(o) => {
                shift_rect(&mut o.rect);
                if let Some(t) = &mut o.tail {
                    shift(t);
                }
            }
            Self::Step(o) => shift(&mut o.pos),
            Self::Stamp(o) => shift(&mut o.pos),
            Self::Picture(o) => shift_rect(&mut o.rect),
            Self::Fill(o) => shift(&mut o.origin),
            Self::Blur(o) => shift_rect(&mut o.rect),
        }
    }

    /// Resizes the object into `rect` (handle drag). Point-based shapes
    /// scale; text/labels move their top-left.
    pub fn set_bounds(&mut self, rect: PhysicalRect) {
        let b = self.bounds();
        let (dx, dy, sx, sy) = (
            (rect.x - b.x) as f32,
            (rect.y - b.y) as f32,
            if b.width > 0 {
                rect.width as f32 / b.width as f32
            } else {
                1.0
            },
            if b.height > 0 {
                rect.height as f32 / b.height as f32
            } else {
                1.0
            },
        );
        let scale_pt = |p: &mut Pos2, b: &PhysicalRect| {
            p.x = rect.x as f32 + (p.x - b.x as f32) * sx;
            p.y = rect.y as f32 + (p.y - b.y as f32) * sy;
        };
        match self {
            Self::Rect(o) => o.rect = rect,
            Self::Ellipse(o) => o.rect = rect,
            Self::Line(o) => {
                scale_pt(&mut o.a, &b);
                scale_pt(&mut o.b, &b);
            }
            Self::Fancy(o) => {
                scale_pt(&mut o.a, &b);
                scale_pt(&mut o.b, &b);
            }
            Self::Freehand(o) | Self::HighlightStroke(o) => {
                for p in &mut o.points {
                    scale_pt(p, &b);
                }
            }
            Self::HighlightArea(o) => o.rect = rect,
            Self::Text(o) => {
                scale_pt(&mut o.a, &b);
                scale_pt(&mut o.b, &b);
                scale_pt(&mut o.ka, &b);
                scale_pt(&mut o.kb, &b);
            }
            Self::TextBox(o) => o.rect = rect,
            Self::Step(o) => {
                o.pos.x += dx;
                o.pos.y += dy;
            }
            Self::Stamp(o) => {
                o.pos.x += dx;
                o.pos.y += dy;
                o.size = (o.size * sx.max(sy)).max(8.0);
            }
            Self::Picture(o) => o.rect = rect,
            Self::Fill(o) => {
                o.origin.x += dx;
                o.origin.y += dy;
            }
            Self::Blur(o) => o.rect = rect,
        }
    }
}

/// The badge radius for a step size preset, physical px.
fn step_radius(size: StepSize) -> f32 {
    match size {
        StepSize::Small => 12.0,
        StepSize::Medium => 18.0,
        StepSize::Large => 26.0,
    }
}

/// `PhysicalRect` spanning two points.
fn rect_of_points(a: Pos2, b: Pos2) -> PhysicalRect {
    let (x1, y1) = (a.x.min(b.x), a.y.min(b.y));
    let (x2, y2) = (a.x.max(b.x), a.y.max(b.y));
    PhysicalRect {
        x: x1 as i32,
        y: y1 as i32,
        width: (x2 - x1).max(1.0) as u32,
        height: (y2 - y1).max(1.0) as u32,
    }
}

/// `PhysicalRect` spanning a point list.
fn points_bounds(points: &[Pos2]) -> PhysicalRect {
    let (mut x1, mut y1, mut x2, mut y2) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for p in points {
        x1 = x1.min(p.x);
        y1 = y1.min(p.y);
        x2 = x2.max(p.x);
        y2 = y2.max(p.y);
    }
    if points.is_empty() {
        (x1, y1, x2, y2) = (0.0, 0.0, 1.0, 1.0);
    }
    PhysicalRect {
        x: x1 as i32,
        y: y1 as i32,
        width: (x2 - x1).max(1.0) as u32,
        height: (y2 - y1).max(1.0) as u32,
    }
}

/// The open document: base capture + annotations in z-order.
#[derive(Clone, Debug)]
pub struct Document {
    /// Internal id, provenance and optional password — the record a
    /// future database round-trip restores the capture from.
    pub meta: super::meta::CaptureMeta,
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
            meta: super::meta::CaptureMeta::fresh(),
            base,
            annotations: Vec::new(),
            revision: 0,
            next_id: 0,
        }
    }

    /// The next id [`Self::alloc_id`] will hand out — persisted so a
    /// restored document never re-issues an id its annotations use.
    pub fn next_id(&self) -> AnnotationId {
        self.next_id
    }

    /// Sets the id counter (session restore only).
    pub fn set_next_id(&mut self, id: AnnotationId) {
        self.next_id = id;
    }

    /// Allocates a fresh annotation id.
    pub fn alloc_id(&mut self) -> AnnotationId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Looks up an annotation by id.
    pub fn annotation(&self, id: AnnotationId) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.id == id)
    }

    /// Looks up an annotation mutably.
    pub fn annotation_mut(&mut self, id: AnnotationId) -> Option<&mut Annotation> {
        self.annotations.iter_mut().find(|a| a.id == id)
    }

    /// Index of `id` in the z-order.
    pub fn index_of(&self, id: AnnotationId) -> Option<usize> {
        self.annotations.iter().position(|a| a.id == id)
    }
}
