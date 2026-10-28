//! The single raster path for documents: every display and export
//! surface (draw canvas, viewer texture, save, copy, print) composites
//! base + annotations through [`render_to_image`], so WYSIWYG holds by
//! construction. Rasterization uses `imageproc` primitives plus small
//! local helpers (thick/dashed polylines, fills, text with outline and
//! faux italics, flood fill, pixelate/blur).

use std::sync::OnceLock;

use egui::{Color32, Pos2};
use image::{Rgba, RgbaImage};
use imageproc::drawing as drw;

use super::document::{
    Annotation, AnnotationKind, AreaShape, BlurStyle, BraceKind, BraceObj, Dash, Document,
    EraserMode, FreeObj, LineEnd, StepMode, StepSize, StepStyle, TextAlign, TipShape,
};
use crate::capture::PhysicalRect;

/// Font bytes embedded for annotation text (same family as the UI).
static FONT_REG_BYTES: &[u8] = include_bytes!("../../assets/fonts/Inter-Regular.ttf");
static FONT_BOLD_BYTES: &[u8] = include_bytes!("../../assets/fonts/Inter-SemiBold.ttf");

fn font(bold: bool) -> &'static ab_glyph::FontArc {
    font_of("Inter", bold)
}

/// Families offered in the text panel — Inter plus the usual document
/// faces, resolved from OS font directories at first use.
pub const FONT_FAMILIES: &[&str] = &[
    "Inter",
    "Arial",
    "Courier New",
    "Times New Roman",
    "Verdana",
    "Georgia",
    "Comic Sans MS",
];

/// Resolves `family`/`bold` to a cached font — Inter when the file is
/// missing (embedded, always present).
pub fn font_of(family: &str, bold: bool) -> &'static ab_glyph::FontArc {
    static CACHE: OnceLock<
        std::sync::Mutex<std::collections::HashMap<(String, bool), ab_glyph::FontArc>>,
    > = OnceLock::new();
    let cache = CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    let key = (family.to_owned(), bold);
    if let Some(f) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        let f = f.clone();
        return Box::leak(Box::new(f));
    }
    let arc = load_family(family, bold).unwrap_or_else(|| {
        let bytes = if bold {
            FONT_BOLD_BYTES
        } else {
            FONT_REG_BYTES
        };
        ab_glyph::FontArc::try_from_slice(bytes)
            .unwrap_or_else(|_| ab_glyph::FontArc::try_from_slice(FONT_REG_BYTES).unwrap())
    });
    let f = {
        let mut g = cache.lock().unwrap_or_else(|e| e.into_inner());
        g.entry(key).or_insert(arc).clone()
    };
    Box::leak(Box::new(f))
}

/// Candidate system paths for a family/bold pair (macOS + Windows).
fn load_family(family: &str, bold: bool) -> Option<ab_glyph::FontArc> {
    let name = match family {
        "Inter" => return None,
        other => other,
    };
    let suffixed = format!("{name} Bold.ttf");
    for dir in [
        "/System/Library/Fonts/Supplemental",
        "/Library/Fonts",
        "C:/Windows/Fonts",
    ] {
        for cand in [
            format!("{dir}/{}{}", name, if bold { " Bold.ttf" } else { ".ttf" }),
            format!("{dir}/{suffixed}"),
            format!("{dir}/{name}.ttf"),
        ] {
            if let Ok(bytes) = std::fs::read(&cand)
                && let Ok(f) = ab_glyph::FontVec::try_from_vec(bytes)
            {
                return Some(ab_glyph::FontArc::from(f));
            }
        }
    }
    None
}

/// Measured (w, h) of a [`super::document::TextObj`]'s lines, padded
/// for its outline — the span the baseline anchors rotate.
pub fn text_extent(o: &super::document::TextObj) -> (f32, f32) {
    let scale = ab_glyph::PxScale::from(o.size);
    let f = font_of(&o.font, o.bold);
    let mut w = 8.0f32;
    let mut n = 0usize;
    for line in o.content.lines() {
        let (lw, _) = drw::text_size(scale, f, line);
        w = w.max(lw as f32);
        n += 1;
    }
    let pad = o.outline.ceil() + 4.0;
    (
        w.max(12.0) + 2.0 * pad,
        (o.size * 1.25 * n.max(1) as f32) + 2.0 * pad,
    )
}

/// Composites `doc` (base + annotations) to a flat RGBA bitmap.
pub fn render_to_image(doc: &Document) -> RgbaImage {
    let mut img = doc.base.image.as_ref().clone();
    // One scratch layer for every layered object: the full-frame
    // alloc happens once per document instead of once per
    // annotation, and each object is rasterized at absolute
    // coordinates — bit-identical to allocating per object.
    let mut scratch = ScratchLayer::new(img.width(), img.height());
    for anno in &doc.annotations {
        render_one(&mut img, &mut scratch, anno);
    }
    img
}

/// A reusable full-frame transparent layer. Only the region the
/// previous object painted needs re-clearing between annotations —
/// a bounded memset instead of a fresh `RgbaImage::new`.
struct ScratchLayer {
    /// The buffer; always full image size.
    buf: RgbaImage,
    /// `(x, y, w, h)` last painted — every other pixel is known-zero.
    dirty: Option<(u32, u32, u32, u32)>,
}

impl ScratchLayer {
    /// A zeroed layer the size of one document image.
    fn new(w: u32, h: u32) -> Self {
        Self {
            buf: RgbaImage::new(w, h),
            dirty: None,
        }
    }

    /// Zeroes the last painted region and lends the layer back.
    fn take(&mut self) -> &mut RgbaImage {
        if let Some((x, y, w, h)) = self.dirty.take() {
            for row in self.buf.rows_mut().skip(y as usize).take(h as usize) {
                for px in row.skip(x as usize).take(w as usize) {
                    *px = Rgba([0, 0, 0, 0]);
                }
            }
        }
        &mut self.buf
    }

    /// Marks `(x, y, w, h)` painted — [`Self::take`] clears it next time.
    fn mark(&mut self, bounds: (u32, u32, u32, u32)) {
        self.dirty = Some(bounds);
    }
}

/// Paints `doc` — kept for API parity with the original module contract;
/// the canvas displays the [`render_to_image`] texture so both paths
/// share pixels by construction.
pub fn paint_document(_ui: &mut egui::Ui, _doc: &Document, _rect: egui::Rect) {}

/// Renders one annotation onto `img` (already a clone of the base).
fn render_one(img: &mut RgbaImage, scratch: &mut ScratchLayer, anno: &Annotation) {
    match &anno.kind {
        // Region effects read + write the base directly.
        AnnotationKind::Blur(o) => apply_blur(img, o),
        AnnotationKind::Freehand(o) if o.eraser_mode != super::document::EraserMode::Color => {
            apply_freehand_effect(img, o);
        }
        _ => {
            // Layered objects rasterize into the shared full-frame
            // scratch layer at absolute coordinates — identical to the
            // old per-object layer — then only their padded bounds get
            // composited (everything outside is transparent by
            // construction). The parity tests pin this to the old
            // path bit for bit.
            let Some((bx, by, bw, bh)) = layer_bounds(anno, img.width(), img.height()) else {
                return;
            };
            let (shadow, opacity) = object_layer(scratch.take(), anno);
            #[cfg(test)]
            {
                // The bounds contract is what makes bounded compositing
                // sound — a paint outside it would silently leak into
                // the NEXT object's layer. Loud in tests only.
                let (w, h) = (img.width(), img.height());
                let (x1, y1) = (bx + bw, by + bh);
                debug_assert!(
                    (0..h * w).all(|i| {
                        let (px, py) = (i % w, i / w);
                        (px >= bx && px < x1 && py >= by && py < y1)
                            || scratch.buf.get_pixel(px, py).0[3] == 0
                    }),
                    "annotation painted outside its layer bounds"
                );
            }
            scratch.mark((bx, by, bw, bh));
            if shadow > 0.0 {
                // The shadow is the object re-tinted dark and shifted —
                // crop just the bounds to tint, composite at +shadow.
                let dark = tint_alpha(
                    &image::imageops::crop_imm(&scratch.buf, bx, by, bw, bh).to_image(),
                    Rgba([0, 0, 0, 90]),
                );
                composite(
                    img,
                    &dark,
                    bx as i64 + shadow as i64,
                    by as i64 + shadow as i64,
                    1.0,
                );
            }
            composite_in(img, &scratch.buf, opacity, (bx, by, bw, bh));
        }
    }
}

/// Anti-aliasing slack every layered bounds gets on top of the
/// stroke/shadow extras — a few transparent pixels of margin cost
/// nothing, a clipped AA fringe would break pixel parity.
const LAYER_AA_SLACK: f32 = 4.0;

/// Image-space `(x, y, w, h)` a layered annotation can touch, clipped
/// to the image — `None` when fully outside. `AnnotationKind::bounds()`
/// grown by the object's stroke/outline/shadow extras plus
/// [`LAYER_AA_SLACK`]; the speech-box tail is unioned in separately
/// since it leaves `BoxObj::rect` entirely.
fn layer_bounds(anno: &Annotation, iw: u32, ih: u32) -> Option<(u32, u32, u32, u32)> {
    let extra = match &anno.kind {
        AnnotationKind::Rect(o) => o.stroke_w * 0.5 + o.outline + o.shadow,
        AnnotationKind::Ellipse(o) => o.stroke_w * 0.5 + o.outline + o.shadow,
        AnnotationKind::Line(o) => o.shadow,
        AnnotationKind::Freehand(o) => o.width + o.outline + o.shadow,
        AnnotationKind::HighlightArea(_) => 0.0,
        AnnotationKind::Text(o) => {
            // Glyphs wider than the baseline spill past `b` (the
            // ribbon in `bounds()` stops at the curve) — pad by the
            // overflow on every side, direction notwithstanding.
            let len = (o.b - o.a).length();
            let overflow = (text_extent(o).0 - len).max(0.0);
            o.outline + o.shadow + overflow
        }
        AnnotationKind::TextBox(o) => o.border_w + o.shadow,
        AnnotationKind::Step(o) => o.shadow,
        AnnotationKind::Picture(o) => o.shadow,
        AnnotationKind::Blur(_) => 0.0, // never layered — a region effect
        AnnotationKind::Brace(o) => o.shadow,
        AnnotationKind::Emoji(_) => 0.0,
    } + LAYER_AA_SLACK;
    let b = anno.kind.bounds();
    let m = extra.ceil() as i32;
    let (mut x0, mut y0) = (b.x - m, b.y - m);
    let (mut x1, mut y1) = (b.x + b.width as i32 + m, b.y + b.height as i32 + m);
    if let AnnotationKind::TextBox(o) = &anno.kind
        && let Some(t) = o.tail
    {
        // Tail triangle: box centre → apex, ~10 px base half-width.
        x0 = x0.min((t.x - 14.0) as i32);
        y0 = y0.min((t.y - 14.0) as i32);
        x1 = x1.max((t.x + 14.0) as i32);
        y1 = y1.max((t.y + 14.0) as i32);
    }
    let (x0, y0) = (x0.clamp(0, iw as i32), y0.clamp(0, ih as i32));
    let (x1, y1) = (x1.clamp(x0, iw as i32), y1.clamp(y0, ih as i32));
    (x1 > x0 && y1 > y0).then(|| (x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32))
}

/// Shadow/opacity accessors shared by the layered objects.
fn shadow_opacity(anno: &Annotation) -> (f32, f32) {
    match &anno.kind {
        AnnotationKind::Rect(o) => (0.0, o.opacity),
        AnnotationKind::Ellipse(o) => (0.0, o.opacity),
        AnnotationKind::Line(o) => (0.0, o.opacity),
        AnnotationKind::Freehand(o) => (0.0, o.opacity),
        AnnotationKind::HighlightArea(o) => (0.0, o.opacity),
        AnnotationKind::Text(o) => (o.shadow, o.opacity),
        AnnotationKind::TextBox(o) => (o.shadow, o.opacity),
        AnnotationKind::Step(o) => (0.0, o.opacity),
        AnnotationKind::Picture(o) => (o.shadow, o.opacity),
        AnnotationKind::Blur(_) => (0.0, 1.0),
        AnnotationKind::Brace(o) => (0.0, o.opacity),
        AnnotationKind::Emoji(_) => (0.0, 1.0),
    }
}

/// Rasterizes a layered object into `layer` — a cleared full-frame
/// scratch buffer — at absolute image coordinates. Returns
/// `(shadow, opacity)`.
fn object_layer(layer: &mut RgbaImage, anno: &Annotation) -> (f32, f32) {
    let (shadow, opacity) = shadow_opacity(anno);
    match &anno.kind {
        AnnotationKind::Rect(o) => draw_shape_sdf(
            layer,
            o.rect,
            false,
            o.radius,
            o.fill,
            o.stroke,
            o.stroke_w,
            o.dash,
            o.outline,
            o.outline_color,
            o.shadow,
        ),
        AnnotationKind::Ellipse(o) => draw_shape_sdf(
            layer,
            o.rect,
            true,
            0.0,
            o.fill,
            o.stroke,
            o.stroke_w,
            o.dash,
            o.outline,
            o.outline_color,
            o.shadow,
        ),
        AnnotationKind::Line(o) => draw_line_obj(layer, o),
        AnnotationKind::Freehand(o) => {
            stroke_freehand(layer, o);
        }
        AnnotationKind::HighlightArea(o) => draw_shape_sdf(
            layer,
            o.rect,
            o.shape != AreaShape::Rect,
            0.0,
            Some(o.color),
            Color32::TRANSPARENT,
            0.0,
            Dash::Solid,
            0.0,
            Color32::TRANSPARENT,
            0.0,
        ),
        AnnotationKind::Text(o) => draw_text_obj(layer, o),
        AnnotationKind::TextBox(o) => draw_textbox(layer, o),
        AnnotationKind::Step(o) => draw_step(layer, o),
        AnnotationKind::Picture(o) => draw_picture(layer, o),
        AnnotationKind::Blur(_) => {}
        AnnotationKind::Brace(o) => draw_brace(layer, o),
        AnnotationKind::Emoji(o) => draw_emoji(layer, o),
    }
    // Per-object alpha (freehand strokes carry theirs on the colour).
    let extra = match &anno.kind {
        AnnotationKind::Freehand(o) => o.opacity,
        _ => 1.0,
    };
    (shadow, opacity * extra)
}

/// Signed distance to a rounded box of half-extents `(hx, hy)` and corner radius `r`.
fn rounded_box_sdf(dx: f32, dy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = dx.abs() - (hx - r);
    let qy = dy.abs() - (hy - r);
    let out = if qx > 0.0 && qy > 0.0 {
        qx.hypot(qy)
    } else {
        qx.max(0.0) + qy.max(0.0)
    };
    out + qx.min(0.0).max(qy.min(0.0)) - r
}

/// Signed distance to an ellipse of semi-axes `(hx, hy)`.
fn ellipse_sdf(dx: f32, dy: f32, hx: f32, hy: f32) -> f32 {
    let a = hx.max(0.5);
    let b = hy.max(0.5);
    let px = dx.abs();
    let py = dy.abs();
    if (a - b).abs() < 1e-2 {
        return px.hypot(py) - a;
    }
    let u = px / a;
    let v = py / b;
    let r = u.hypot(v);
    if r < 1e-5 {
        return -a.min(b);
    }
    let approx = (r - 1.0) * a.min(b);
    if approx.abs() > a.max(b) * 0.5 + 18.0 {
        return approx;
    }
    let c2 = u.min(1.0);
    let s2 = (1.0 - c2 * c2).max(0.0).sqrt();
    let s3 = v.min(1.0);
    let c3 = (1.0 - s3 * s3).max(0.0).sqrt();
    let (mut c, mut s) = (u / r, v / r);
    let mut best_d2 = (px - a * c).powi(2) + (py - b * s).powi(2);
    for (cand_c, cand_s) in [(c2, s2), (c3, s3)] {
        let d2 = (px - a * cand_c).powi(2) + (py - b * cand_s).powi(2);
        if d2 < best_d2 {
            best_d2 = d2;
            c = cand_c;
            s = cand_s;
        }
    }
    for _ in 0..3 {
        let (ex, ey) = (a * c, b * s);
        let (rx, ry) = (px - ex, py - ey);
        let (tx, ty) = (-a * s, b * c);
        let num = rx * tx + ry * ty;
        let den = (tx * tx + ty * ty + rx * ex + ry * ey).max(1e-3);
        let dth = (num / den).clamp(-0.6, 0.6);
        let nc = (c - s * dth).max(0.0);
        let ns = (s + c * dth).max(0.0);
        let inv = 1.0 / nc.hypot(ns).max(1e-6);
        let (nc, ns) = (nc * inv, ns * inv);
        let d2 = (px - a * nc).powi(2) + (py - b * ns).powi(2);
        if d2 <= best_d2 {
            best_d2 = d2;
            c = nc;
            s = ns;
        }
    }
    let dist = best_d2.sqrt();
    if r >= 1.0 { dist } else { -dist }
}

/// Renders a rectangle or ellipse (stroked or filled) with analytical SDF
/// anti-aliasing, rounded corners, dashing, unified outline and soft drop shadow.
#[allow(clippy::too_many_arguments)]
fn draw_shape_sdf(
    img: &mut RgbaImage,
    rect: PhysicalRect,
    is_ellipse: bool,
    radius: f32,
    fill: Option<Color32>,
    stroke: Color32,
    stroke_w: f32,
    dash: Dash,
    outline: f32,
    outline_color: Color32,
    shadow: f32,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let (w_rect, h_rect) = (rect.width as f32, rect.height as f32);
    let (cx, cy) = (rect.x as f32 + w_rect * 0.5, rect.y as f32 + h_rect * 0.5);
    let (hx, hy) = ((w_rect * 0.5).max(0.5), (h_rect * 0.5).max(0.5));
    let r = if is_ellipse {
        0.0
    } else {
        radius.clamp(0.0, hx.min(hy))
    };
    let has_stroke = stroke.a() > 0 && (stroke_w > 0.0 || fill.is_none());
    let w = if has_stroke {
        if fill.is_none() {
            stroke_w.max(1.0)
        } else {
            stroke_w.max(0.0)
        }
    } else {
        0.0
    };
    let hw = w * 0.5;
    let out_px = outline.max(0.0) * 0.65;

    let d_bound = |p: Pos2| -> f32 {
        if is_ellipse {
            ellipse_sdf(p.x - cx, p.y - cy, hx, hy)
        } else {
            rounded_box_sdf(p.x - cx, p.y - cy, hx, hy, r)
        }
    };

    // Precompute dashed perimeter segments when `dash != Dash::Solid`.
    let mut segs: Vec<(Pos2, Pos2, f32, f32, f32, f32)> = Vec::new();
    if has_stroke && dash != Dash::Solid {
        let mut poly = shape_path(
            rect,
            if is_ellipse {
                AreaShape::Ellipse
            } else if r > 0.5 {
                AreaShape::Round
            } else {
                AreaShape::Rect
            },
            r,
        );
        if let Some(first) = poly.first().copied() {
            poly.push(first);
        }
        let mut arc = Vec::with_capacity(poly.len());
        let mut total = 0.0_f32;
        if let Some(&p0) = poly.first() {
            arc.push((0.0_f32, p0));
            for pair in poly.windows(2) {
                let d = (pair[1] - pair[0]).length();
                if d > 1e-4 {
                    total += d;
                    arc.push((total, pair[1]));
                }
            }
        }
        if total >= 0.5 {
            let sample_poly = |s: f32| -> Pos2 {
                let s = s.clamp(0.0, total);
                let idx = arc
                    .partition_point(|e| e.0 < s)
                    .min(arc.len().saturating_sub(1));
                if idx == 0 {
                    return arc[0].1;
                }
                let (s0, p0) = arc[idx - 1];
                let (s1, p1) = arc[idx];
                let t = if (s1 - s0).abs() < 1e-4 {
                    0.0
                } else {
                    (s - s0) / (s1 - s0)
                };
                lerp(p0, p1, t)
            };
            for (u0, u1) in dash_runs(total, w.max(1.0), dash) {
                let sa = u0 * total;
                let sb = u1 * total;
                let i0 = arc.partition_point(|e| e.0 <= sa);
                let i1 = arc.partition_point(|e| e.0 < sb);
                let mut prev = sample_poly(sa);
                for &(s_mid, p_mid) in &arc[i0..i1] {
                    if s_mid > sa && s_mid < sb {
                        segs.push((
                            prev,
                            p_mid,
                            prev.x.min(p_mid.x),
                            prev.y.min(p_mid.y),
                            prev.x.max(p_mid.x),
                            prev.y.max(p_mid.y),
                        ));
                        prev = p_mid;
                    }
                }
                let end_p = sample_poly(sb);
                segs.push((
                    prev,
                    end_p,
                    prev.x.min(end_p.x),
                    prev.y.min(end_p.y),
                    prev.x.max(end_p.x),
                    prev.y.max(end_p.y),
                ));
            }
        }
    }

    let reach = hw + out_px + 10.0;
    let stroke_sdf_with_bound = |p: Pos2, db: f32| -> f32 {
        if !has_stroke {
            return f32::INFINITY;
        }
        if dash == Dash::Solid {
            db.abs() - hw
        } else {
            if db.abs() > reach {
                return db.abs() - hw;
            }
            let mut d = f32::INFINITY;
            for &(a, b, x0, y0, x1, y1) in &segs {
                if p.x < x0 - reach || p.x > x1 + reach || p.y < y0 - reach || p.y > y1 + reach {
                    continue;
                }
                d = d.min(pt_seg_dist(p, a, b) - hw);
            }
            d
        }
    };

    let pad = (hw + out_px + if shadow > 0.0 { 10.0 } else { 3.0 }).ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let x0 = (rect.x - pad).max(0);
    let y0 = (rect.y - pad).max(0);
    let x1 = (rect.x + rect.width as i32 + pad).min(iw - 1);
    let y1 = (rect.y + rect.height as i32 + pad).min(ih - 1);

    let rc = outline_color;
    for py in y0..=y1 {
        for px in x0..=x1 {
            let p = Pos2::new(px as f32 + 0.5, py as f32 + 0.5);
            let db = d_bound(p);

            if let Some(fill_col) = fill {
                if db > hw + out_px + 9.0 {
                    continue;
                }
                let d_stroke = stroke_sdf_with_bound(p, db);
                let d_body = if !has_stroke {
                    db
                } else if dash == Dash::Solid {
                    db - hw
                } else {
                    db.min(d_stroke)
                };
                let dout = d_body - out_px;
                let cov_out = (0.5 - dout).clamp(0.0, 1.0);

                let sh_a = if shadow > 0.0 && cov_out < 1.0 {
                    let ps = Pos2::new(p.x - 2.0, p.y - 2.5);
                    let dbs = d_bound(ps);
                    let d_body_s = if !has_stroke {
                        dbs
                    } else if dash == Dash::Solid {
                        dbs - hw
                    } else {
                        dbs.min(stroke_sdf_with_bound(ps, dbs))
                    };
                    let dsh = d_body_s - out_px;
                    let t = (1.0 - (dsh + 0.8) / 6.5).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t) * 0.45
                } else {
                    0.0
                };

                let total_a = cov_out + sh_a * (1.0 - cov_out);
                if total_a <= 1.0 / 255.0 {
                    continue;
                }

                let (mut br, mut bg_c, mut bb) = (0.0_f32, 0.0_f32, 0.0_f32);
                if cov_out > 0.0 {
                    br = f32::from(fill_col.r());
                    bg_c = f32::from(fill_col.g());
                    bb = f32::from(fill_col.b());
                    if has_stroke {
                        let cov_s = if dash == Dash::Solid {
                            (0.5 - (-hw - db)).clamp(0.0, 1.0)
                        } else {
                            (0.5 - d_stroke).clamp(0.0, 1.0)
                        };
                        if cov_s > 0.0 {
                            br = br * (1.0 - cov_s) + f32::from(stroke.r()) * cov_s;
                            bg_c = bg_c * (1.0 - cov_s) + f32::from(stroke.g()) * cov_s;
                            bb = bb * (1.0 - cov_s) + f32::from(stroke.b()) * cov_s;
                        }
                    }
                    if out_px > 0.0 {
                        let cov_in = (0.5 - d_body).clamp(0.0, 1.0);
                        br = f32::from(rc.r()) * (1.0 - cov_in) + br * cov_in;
                        bg_c = f32::from(rc.g()) * (1.0 - cov_in) + bg_c * cov_in;
                        bb = f32::from(rc.b()) * (1.0 - cov_in) + bb * cov_in;
                    }
                }

                let w_body = cov_out / total_a;
                let out_r = (br * w_body).round().clamp(0.0, 255.0) as u8;
                let out_g = (bg_c * w_body).round().clamp(0.0, 255.0) as u8;
                let out_b = (bb * w_body).round().clamp(0.0, 255.0) as u8;
                let out_a = (total_a * 255.0).round().clamp(0.0, 255.0) as u8;
                img.put_pixel(px as u32, py as u32, Rgba([out_r, out_g, out_b, out_a]));
            } else {
                if db.abs() > hw + out_px + 9.0 {
                    continue;
                }
                let din = stroke_sdf_with_bound(p, db);
                if din > out_px + 9.0 {
                    continue;
                }
                let dout = din - out_px;
                let cov_out = (0.5 - dout).clamp(0.0, 1.0);

                let sh_a = if shadow > 0.0 && cov_out < 1.0 {
                    let ps = Pos2::new(p.x - 2.0, p.y - 2.5);
                    let dbs = d_bound(ps);
                    let dsh = stroke_sdf_with_bound(ps, dbs) - out_px;
                    let t = (1.0 - (dsh + 0.8) / 6.5).clamp(0.0, 1.0);
                    t * t * (3.0 - 2.0 * t) * 0.45
                } else {
                    0.0
                };

                let total_a = cov_out + sh_a * (1.0 - cov_out);
                if total_a <= 1.0 / 255.0 {
                    continue;
                }

                let (mut br, mut bg_c, mut bb) = (0.0_f32, 0.0_f32, 0.0_f32);
                if cov_out > 0.0 {
                    let cov_in = if out_px > 0.0 {
                        (0.5 - din).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    br = f32::from(rc.r()) * (1.0 - cov_in) + f32::from(stroke.r()) * cov_in;
                    bg_c = f32::from(rc.g()) * (1.0 - cov_in) + f32::from(stroke.g()) * cov_in;
                    bb = f32::from(rc.b()) * (1.0 - cov_in) + f32::from(stroke.b()) * cov_in;
                }

                let w_body = cov_out / total_a;
                let out_r = (br * w_body).round().clamp(0.0, 255.0) as u8;
                let out_g = (bg_c * w_body).round().clamp(0.0, 255.0) as u8;
                let out_b = (bb * w_body).round().clamp(0.0, 255.0) as u8;
                let out_a = (total_a * 255.0).round().clamp(0.0, 255.0) as u8;
                img.put_pixel(px as u32, py as u32, Rgba([out_r, out_g, out_b, out_a]));
            }
        }
    }
}

/// Filled + stroked shape (rect/round/ellipse) onto a layer.
#[allow(clippy::too_many_arguments)]
fn draw_shape(
    img: &mut RgbaImage,
    rect: PhysicalRect,
    shape: AreaShape,
    radius: f32,
    fill: Option<Color32>,
    stroke: Color32,
    stroke_w: f32,
    dash: Dash,
    fill_strength: f32,
) {
    let path = shape_path(rect, shape, radius);
    if let Some(f) = fill {
        let c = with_alpha(f, (f.a() as f32 * fill_strength) as u8);
        drw::draw_polygon_mut(img, &to_points(&path), c);
    }
    if stroke_w > 0.0 && stroke.a() > 0 {
        let mut closed = path.clone();
        if let Some(first) = path.first().copied() {
            closed.push(first);
        }
        stroke_polyline(img, &closed, stroke_w, stroke, TipShape::Square, dash);
    }
}

/// Perimeter path of a rect/round-rect/ellipse (polyline approximation).
fn shape_path(rect: PhysicalRect, shape: AreaShape, radius: f32) -> Vec<Pos2> {
    let (x, y) = (rect.x as f32, rect.y as f32);
    let (w, h) = (rect.width as f32, rect.height as f32);
    match shape {
        AreaShape::Rect => vec![
            Pos2::new(x, y),
            Pos2::new(x + w, y),
            Pos2::new(x + w, y + h),
            Pos2::new(x, y + h),
        ],
        AreaShape::Round => {
            let r = radius.min(w / 2.0).min(h / 2.0).max(0.0);
            let mut pts = Vec::with_capacity(36);
            for &(cx, cy, a0) in &[
                (x + w - r, y + r, -90.0f32),
                (x + w - r, y + h - r, 0.0),
                (x + r, y + h - r, 90.0),
                (x + r, y + r, 180.0),
            ] {
                for i in 0..=8 {
                    let a = (a0 + i as f32 * 90.0 / 8.0).to_radians();
                    pts.push(Pos2::new(cx + r * a.cos(), cy + r * a.sin()));
                }
            }
            pts
        }
        AreaShape::Ellipse => {
            let (cx, cy, rx, ry) = (x + w / 2.0, y + h / 2.0, w / 2.0, h / 2.0);
            (0..=48)
                .map(|i| {
                    let a = i as f32 * std::f32::consts::TAU / 48.0;
                    Pos2::new(cx + rx * a.cos(), cy + ry * a.sin())
                })
                .collect()
        }
    }
}

/// egui `Pos2` list → imageproc `Point<i32>` list (filled polygons).
fn to_points(path: &[Pos2]) -> Vec<imageproc::point::Point<i32>> {
    path.iter()
        .map(|p| imageproc::point::Point::new(p.x as i32, p.y as i32))
        .collect()
}

/// Converts an inline list of `Point<f32>` literals to `Point<i32>`.
fn points_i32(pts: &[imageproc::point::Point<f32>]) -> Vec<imageproc::point::Point<i32>> {
    pts.iter()
        .map(|p| imageproc::point::Point::new(p.x as i32, p.y as i32))
        .collect()
}

/// Strokes a polyline with width — quad per segment + round joints.
fn stroke_polyline(
    img: &mut RgbaImage,
    points: &[Pos2],
    width: f32,
    color: Color32,
    tip: TipShape,
    dash: Dash,
) {
    if points.len() < 2 || width <= 0.0 {
        return;
    }
    let c = c32(color);
    let half = width / 2.0;
    for seg in points.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        for (t0, t1) in dash_runs((a.x - b.x).hypot(a.y - b.y), width, dash) {
            let (p, q) = (lerp(a, b, t0), lerp(a, b, t1));
            let (dx, dy) = (q.x - p.x, q.y - p.y);
            let len = dx.hypot(dy).max(0.001);
            let (nx, ny) = (-dy / len * half, dx / len * half);
            drw::draw_polygon_mut(
                img,
                &points_i32(&[
                    imageproc::point::Point::new(p.x + nx, p.y + ny),
                    imageproc::point::Point::new(q.x + nx, q.y + ny),
                    imageproc::point::Point::new(q.x - nx, q.y - ny),
                    imageproc::point::Point::new(p.x - nx, p.y - ny),
                ]),
                c,
            );
        }
        if tip == TipShape::Round {
            drw::draw_filled_circle_mut(img, (a.x as i32, a.y as i32), half as i32, c);
        }
    }
    if tip == TipShape::Round
        && let Some(last) = points.last()
    {
        drw::draw_filled_circle_mut(img, (last.x as i32, last.y as i32), half as i32, c);
    }
}

/// Sub-runs (fractions 0–1) of a segment visible under `dash`.
fn dash_runs(len: f32, w: f32, dash: Dash) -> Vec<(f32, f32)> {
    let pat: &[f32] = match dash {
        Dash::Solid => return vec![(0.0, 1.0)],
        Dash::Dash => &[4.0, 2.5],
        Dash::Dot => &[0.8, 2.0],
        Dash::DashDot => &[4.0, 2.0, 0.8, 2.0],
    };
    let period: f32 = pat.iter().sum::<f32>() * w.max(1.0);
    let mut out = Vec::new();
    let mut d = 0.0;
    let mut on = true;
    let mut i = 0;
    while d < len {
        let step = pat[i % pat.len()] * w.max(1.0);
        if on {
            out.push((d / len, ((d + step).min(len)) / len));
        }
        d += step;
        on = !on;
        i += 1;
        let _ = period;
    }
    out
}

/// Linear interpolation between two positions.
fn lerp(a: Pos2, b: Pos2, t: f32) -> Pos2 {
    Pos2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
}

/// Signed distance from `p` to a closed 2D polygon (`verts` in order).
fn sd_polygon(p: Pos2, verts: &[Pos2]) -> f32 {
    let n = verts.len();
    if n < 3 {
        return f32::INFINITY;
    }
    let mut d2 = (p - verts[0]).length_sq();
    let mut s = 1.0_f32;
    let mut j = n - 1;
    for i in 0..n {
        let vi = verts[i];
        let vj = verts[j];
        let e = vj - vi;
        let w = p - vi;
        let proj = (w.dot(e) / e.length_sq().max(1e-6)).clamp(0.0, 1.0);
        let b = w - e * proj;
        d2 = d2.min(b.length_sq());
        let c1 = p.y >= vi.y;
        let c2 = p.y < vj.y;
        let c3 = e.x * w.y > e.y * w.x;
        if (c1 && c2 && c3) || (!c1 && !c2 && !c3) {
            s = -s;
        }
        j = i;
    }
    s * d2.sqrt()
}

/// Signed distance from `p` to segment `a..b`.
fn pt_seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 < 1e-6 {
        return (p - a).length();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

/// Cubic-Bézier line/arrow rendered with analytical SDF anti-aliasing,
/// barbed arrowheads, unified outline (0–10) and soft drop shadow.
fn draw_line_obj(img: &mut RgbaImage, o: &super::document::LineObj) {
    let tab = cubic_table(o.a, o.ka, o.kb, o.b, 96);
    let total = tab.last().map_or(0.0, |e| e.0);
    if total < 0.5 {
        return;
    }
    let w = o.width.max(1.0);
    let hw = w * 0.5;
    let out_px = o.outline.max(0.0) * 0.65;

    // End geometry: outward unit direction at `a` and `b`.
    let (_, pa, da0) = tab[0];
    let (_, pb, db0) = tab[tab.len() - 1];
    // Average tangent over a few pixels near the tip so curved arrows aim cleanly.
    let da_in = sample_at(&tab, (w * 2.5 + 6.0).min(total * 0.35)).1;
    let db_in = sample_at(&tab, (total - (w * 2.5 + 6.0).min(total * 0.35)).max(0.0)).1;
    let dir_a = {
        let v = -(da0 + da_in);
        if v.length() > 1e-3 {
            v / v.length()
        } else {
            -da0
        }
    };
    let dir_b = {
        let v = db0 + db_in;
        if v.length() > 1e-3 {
            v / v.length()
        } else {
            db0
        }
    };

    let arrow_dims = |width: f32| -> (f32, f32, f32) {
        let hl = (width * 3.4 + 11.0).min(total * 0.55);
        let ahw = width * 1.45 + 4.5;
        let neck = hl * 0.76;
        (hl, ahw, neck)
    };
    let (hl, ahw, neck) = arrow_dims(w);
    let dot_r = w * 1.35 + 2.5;
    let ring_r = w * 1.45 + 3.0;
    let ring_th = (w * 0.45).max(1.25);

    let trim_for = |end: LineEnd| -> f32 {
        match end {
            LineEnd::None | LineEnd::Dot => 0.0,
            LineEnd::Arrow => neck * 0.85,
            LineEnd::Ring => ring_r,
        }
    };
    let s0 = trim_for(o.start).min(total * 0.48);
    let s1 = (total - trim_for(o.end)).max(s0 + 0.1);

    // Build active polyline sub-segments along [s0, s1] according to `o.dash`.
    let span = (s1 - s0).max(0.1);
    let runs = dash_runs(span, w, o.dash);
    let mut segs: Vec<(Pos2, Pos2)> = Vec::new();
    for (u0, u1) in runs {
        let sa = s0 + u0 * span;
        let sb = s0 + u1 * span;
        let steps = (((sb - sa) / 3.0).ceil() as usize).clamp(1, 64);
        let mut prev = sample_at(&tab, sa).0;
        for k in 1..=steps {
            let s = sa + (sb - sa) * (k as f32 / steps as f32);
            let cur = sample_at(&tab, s).0;
            segs.push((prev, cur));
            prev = cur;
        }
    }

    let arrow_verts = |tip: Pos2, dir: egui::Vec2| -> [Pos2; 4] {
        let n = egui::vec2(-dir.y, dir.x);
        [
            tip,
            tip - dir * hl + n * ahw,
            tip - dir * neck,
            tip - dir * hl - n * ahw,
        ]
    };
    let va = (o.start == LineEnd::Arrow).then(|| arrow_verts(pa, dir_a));
    let vb = (o.end == LineEnd::Arrow).then(|| arrow_verts(pb, dir_b));

    let end_sdf = |p: Pos2, tip: Pos2, end: LineEnd, verts: Option<&[Pos2; 4]>| -> f32 {
        match end {
            LineEnd::None => f32::INFINITY,
            LineEnd::Arrow => verts.map_or(f32::INFINITY, |v| sd_polygon(p, v)),
            LineEnd::Dot => (p - tip).length() - dot_r,
            LineEnd::Ring => ((p - tip).length() - ring_r).abs() - ring_th,
        }
    };

    let shape_sdf = |p: Pos2| -> f32 {
        let mut d = f32::INFINITY;
        for &(a, b) in &segs {
            d = d.min(pt_seg_dist(p, a, b) - hw);
        }
        d = d.min(end_sdf(p, pa, o.start, va.as_ref()));
        d = d.min(end_sdf(p, pb, o.end, vb.as_ref()));
        d
    };

    // Bounding box of the curve + heads + outline + shadow.
    let mut x0f = pa.x.min(pb.x);
    let mut y0f = pa.y.min(pb.y);
    let mut x1f = pa.x.max(pb.x);
    let mut y1f = pa.y.max(pb.y);
    for (_, p, _) in &tab {
        x0f = x0f.min(p.x);
        y0f = y0f.min(p.y);
        x1f = x1f.max(p.x);
        y1f = y1f.max(p.y);
    }
    let pad = (hl.max(ring_r + ring_th).max(hw) + out_px + if o.shadow > 0.0 { 8.0 } else { 3.0 })
        .ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let x0 = (x0f.floor() as i32 - pad).max(0);
    let y0 = (y0f.floor() as i32 - pad).max(0);
    let x1 = (x1f.ceil() as i32 + pad).min(iw - 1);
    let y1 = (y1f.ceil() as i32 + pad).min(ih - 1);

    let (fc, rc) = (o.color, o.outline_color);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let p = Pos2::new(px as f32, py as f32);
            let din = shape_sdf(p);
            let dout = din - out_px;
            let cov_out = (0.5 - dout).clamp(0.0, 1.0);

            let sh_a = if o.shadow > 0.0 && cov_out < 1.0 {
                let dsh = shape_sdf(Pos2::new(p.x - 1.2, p.y - 2.0)) - out_px;
                let t = (1.0 - (dsh + 0.8) / 5.2).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t) * 0.42
            } else {
                0.0
            };

            let total_a = cov_out + sh_a * (1.0 - cov_out);
            if total_a <= 1.0 / 255.0 {
                continue;
            }

            let (mut br, mut bg_c, mut bb) = (0.0_f32, 0.0_f32, 0.0_f32);
            if cov_out > 0.0 {
                let cov_in = if out_px > 0.0 {
                    (0.5 - din).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                br = f32::from(rc.r()) * (1.0 - cov_in) + f32::from(fc.r()) * cov_in;
                bg_c = f32::from(rc.g()) * (1.0 - cov_in) + f32::from(fc.g()) * cov_in;
                bb = f32::from(rc.b()) * (1.0 - cov_in) + f32::from(fc.b()) * cov_in;
            }

            let w_body = cov_out / total_a;
            let out_r = (br * w_body).round().clamp(0.0, 255.0) as u8;
            let out_g = (bg_c * w_body).round().clamp(0.0, 255.0) as u8;
            let out_b = (bb * w_body).round().clamp(0.0, 255.0) as u8;
            let out_a = (total_a * 255.0).round().clamp(0.0, 255.0) as u8;
            img.put_pixel(px as u32, py as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// Polyline vertices of a [`BraceObj`] from endpoint `a` to endpoint `b`.
pub fn brace_polyline(o: &BraceObj) -> Vec<Pos2> {
    let d_vec = o.b - o.a;
    let len = d_vec.length();
    if len < 1.0 {
        return vec![o.a, o.b];
    }
    // Canonicalize endpoint order so `{`, `[`, `(` always bulge left (or up
    // when horizontal) and `}`, `]`, `)` always bulge right (or down).
    let (p0, p1) = if d_vec.y < -1e-3 || (d_vec.y.abs() <= 1e-3 && d_vec.x < 0.0) {
        (o.b, o.a)
    } else {
        (o.a, o.b)
    };
    let u = (p1 - p0) / len;
    let n_left = egui::vec2(-u.y, u.x);
    let n = if o.kind.is_left() { n_left } else { -n_left };
    let pt = |s: f32, off: f32| -> Pos2 { p0 + u * s + n * off };

    match o.kind {
        BraceKind::CurlyLeft
        | BraceKind::CurlyRight
        | BraceKind::CurlyWideLeft
        | BraceKind::CurlyWideRight => {
            let wide = matches!(o.kind, BraceKind::CurlyWideLeft | BraceKind::CurlyWideRight);
            let d = if wide {
                ((len * 0.18).clamp(9.0, 22.0) + o.width * 0.45).min(len * 0.36)
            } else {
                ((len * 0.10).clamp(5.0, 11.0) + o.width * 0.35).min(len * 0.25)
            };
            let r_end = (d * 0.95).min(len * 0.22);
            let d_tip = (d * 0.85).min(len * 0.22);
            let r_mid = d_tip.min((len * 0.5 - r_end).max(0.5));
            let half = len * 0.5;
            let frac_pi_2 = std::f32::consts::FRAC_PI_2;
            let mut pts = Vec::with_capacity(52);
            for k in 0..=12 {
                let th = (k as f32 / 12.0) * frac_pi_2;
                pts.push(pt(r_end * (1.0 - th.cos()), d * th.sin()));
            }
            for k in 0..=12 {
                let th = (k as f32 / 12.0) * frac_pi_2;
                pts.push(pt(
                    (half - r_mid) + r_mid * th.sin(),
                    (d + d_tip) - d_tip * th.cos(),
                ));
            }
            for k in 1..=12 {
                let th = (k as f32 / 12.0) * frac_pi_2;
                pts.push(pt(
                    (half + r_mid) - r_mid * th.cos(),
                    (d + d_tip) - d_tip * th.sin(),
                ));
            }
            for k in 0..=12 {
                let th = (k as f32 / 12.0) * frac_pi_2;
                pts.push(pt((len - r_end) + r_end * th.sin(), d * th.cos()));
            }
            pts
        }
        BraceKind::SquareLeft | BraceKind::SquareRight => {
            let d = ((len * 0.16).clamp(5.0, 15.0) + o.width * 0.4).min(len * 0.45);
            vec![pt(0.0, 0.0), pt(0.0, d), pt(len, d), pt(len, 0.0)]
        }
        BraceKind::RoundLeft | BraceKind::RoundRight => {
            let d = ((len * 0.18).clamp(4.0, 18.0) + o.width * 0.3).min(len * 0.45);
            let r = (d * d + (len * 0.5).powi(2)) / (2.0 * d.max(0.5));
            let alpha = (len * 0.5 / r).clamp(-1.0, 1.0).asin();
            (0..=32)
                .map(|k| {
                    let ang = -alpha + (2.0 * alpha) * (k as f32 / 32.0);
                    pt(len * 0.5 + r * ang.sin(), d - r * (1.0 - ang.cos()))
                })
                .collect()
        }
    }
}

/// Renders a bracket / curly brace onto `img` using SDF polyline stroking.
fn draw_brace(img: &mut RgbaImage, o: &BraceObj) {
    let points = brace_polyline(o);
    let free = FreeObj {
        points,
        color: o.color,
        width: o.width,
        tip: TipShape::Round,
        dash: o.dash,
        outline: o.outline,
        outline_color: o.outline_color,
        shadow: o.shadow,
        eraser_mode: EraserMode::Color,
        soft: false,
        opacity: 1.0,
    };
    stroke_freehand(img, &free);
}

/// Pencil/eraser freehand stroke with SDF anti-aliasing,
/// dashing, unified outline (0–10), soft edge feathering and drop shadow.
fn stroke_freehand(img: &mut RgbaImage, o: &super::document::FreeObj) {
    if o.points.is_empty() {
        return;
    }

    let w = o.width.max(1.0);
    let hw = w * 0.5;
    let out_px = o.outline.max(0.0) * 0.65;
    let feather = if o.soft { (hw * 0.65).max(2.5) } else { 1.0 };

    // Build arc-length table along `o.points` and clip to active dash runs.
    let mut arc = Vec::with_capacity(o.points.len());
    let mut total = 0.0_f32;
    arc.push((0.0_f32, o.points[0]));
    for pair in o.points.windows(2) {
        let d = (pair[1] - pair[0]).length();
        if d > 1e-4 {
            total += d;
            arc.push((total, pair[1]));
        }
    }

    let sample_poly = |s: f32| -> Pos2 {
        let s = s.clamp(0.0, total);
        let idx = arc
            .partition_point(|e| e.0 < s)
            .min(arc.len().saturating_sub(1));
        if idx == 0 {
            return arc[0].1;
        }
        let (s0, p0) = arc[idx - 1];
        let (s1, p1) = arc[idx];
        let t = if (s1 - s0).abs() < 1e-4 {
            0.0
        } else {
            (s - s0) / (s1 - s0)
        };
        lerp(p0, p1, t)
    };

    // Active sub-segments `(a, b, min_x, min_y, max_x, max_y)` for fast AABB culling.
    let mut segs: Vec<(Pos2, Pos2, f32, f32, f32, f32)> = Vec::new();
    if total < 0.5 {
        let p = o.points[0];
        segs.push((p, p, p.x, p.y, p.x, p.y));
    } else {
        for (u0, u1) in dash_runs(total, w, o.dash) {
            let sa = u0 * total;
            let sb = u1 * total;
            let i0 = arc.partition_point(|e| e.0 <= sa);
            let i1 = arc.partition_point(|e| e.0 < sb);
            let mut prev = sample_poly(sa);
            for &(s_mid, p_mid) in &arc[i0..i1] {
                if s_mid > sa && s_mid < sb {
                    segs.push((
                        prev,
                        p_mid,
                        prev.x.min(p_mid.x),
                        prev.y.min(p_mid.y),
                        prev.x.max(p_mid.x),
                        prev.y.max(p_mid.y),
                    ));
                    prev = p_mid;
                }
            }
            let end_p = sample_poly(sb);
            segs.push((
                prev,
                end_p,
                prev.x.min(end_p.x),
                prev.y.min(end_p.y),
                prev.x.max(end_p.x),
                prev.y.max(end_p.y),
            ));
        }
    }

    let reach = hw + out_px + 8.0;
    let sdf_at = |p: Pos2| -> f32 {
        let mut d = f32::INFINITY;
        for &(a, b, x0, y0, x1, y1) in &segs {
            if p.x < x0 - reach || p.x > x1 + reach || p.y < y0 - reach || p.y > y1 + reach {
                continue;
            }
            let seg_d = match o.tip {
                TipShape::Round => pt_seg_dist(p, a, b) - hw,
                TipShape::Square => {
                    let ab = b - a;
                    let l2 = ab.length_sq();
                    let q = if l2 < 1e-6 {
                        a
                    } else {
                        a + ab * ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
                    };
                    (p.x - q.x).abs().max((p.y - q.y).abs()) - hw
                }
            };
            d = d.min(seg_d);
        }
        d
    };

    let mut x0f = f32::INFINITY;
    let mut y0f = f32::INFINITY;
    let mut x1f = f32::NEG_INFINITY;
    let mut y1f = f32::NEG_INFINITY;
    for p in &o.points {
        x0f = x0f.min(p.x);
        y0f = y0f.min(p.y);
        x1f = x1f.max(p.x);
        y1f = y1f.max(p.y);
    }
    let pad = (hw + out_px + if o.shadow > 0.0 { 8.0 } else { 3.0 }).ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let x0 = (x0f.floor() as i32 - pad).max(0);
    let y0 = (y0f.floor() as i32 - pad).max(0);
    let x1 = (x1f.ceil() as i32 + pad).min(iw - 1);
    let y1 = (y1f.ceil() as i32 + pad).min(ih - 1);

    let (fc, rc) = (o.color, o.outline_color);
    for py in y0..=y1 {
        for px in x0..=x1 {
            let p = Pos2::new(px as f32, py as f32);
            let din = sdf_at(p);
            if din > out_px + feather + 6.0 {
                continue;
            }
            let dout = din - out_px;
            let cov_out = ((0.5 * feather - dout) / feather).clamp(0.0, 1.0);

            let sh_a = if o.shadow > 0.0 && cov_out < 1.0 {
                let dsh = sdf_at(Pos2::new(p.x - 1.2, p.y - 2.0)) - out_px;
                let t = (1.0 - (dsh + 0.8) / 5.2).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t) * 0.42
            } else {
                0.0
            };

            let total_a = cov_out + sh_a * (1.0 - cov_out);
            if total_a <= 1.0 / 255.0 {
                continue;
            }

            let (mut br, mut bg_c, mut bb) = (0.0_f32, 0.0_f32, 0.0_f32);
            if cov_out > 0.0 {
                let cov_in = if out_px > 0.0 {
                    ((0.5 * feather - din) / feather).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                br = f32::from(rc.r()) * (1.0 - cov_in) + f32::from(fc.r()) * cov_in;
                bg_c = f32::from(rc.g()) * (1.0 - cov_in) + f32::from(fc.g()) * cov_in;
                bb = f32::from(rc.b()) * (1.0 - cov_in) + f32::from(fc.b()) * cov_in;
            }

            let w_body = cov_out / total_a;
            let out_r = (br * w_body).round().clamp(0.0, 255.0) as u8;
            let out_g = (bg_c * w_body).round().clamp(0.0, 255.0) as u8;
            let out_b = (bb * w_body).round().clamp(0.0, 255.0) as u8;
            let out_a = (total_a * 255.0).round().clamp(0.0, 255.0) as u8;
            img.put_pixel(px as u32, py as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// Applies a freehand brush blur or pixelation along `o.points` directly onto `img`.
fn apply_freehand_effect(img: &mut RgbaImage, o: &super::document::FreeObj) {
    if o.points.is_empty() {
        return;
    }
    let w = o.width.max(1.0);
    let hw = w * 0.5;
    let feather = if o.soft { (hw * 0.65).max(2.5) } else { 1.0 };

    let mut x0f = f32::INFINITY;
    let mut y0f = f32::INFINITY;
    let mut x1f = f32::NEG_INFINITY;
    let mut y1f = f32::NEG_INFINITY;
    for p in &o.points {
        x0f = x0f.min(p.x);
        y0f = y0f.min(p.y);
        x1f = x1f.max(p.x);
        y1f = y1f.max(p.y);
    }
    let pad = (hw + feather + 4.0).ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let x0 = (x0f.floor() as i32 - pad).max(0);
    let y0 = (y0f.floor() as i32 - pad).max(0);
    let x1 = (x1f.ceil() as i32 + pad).min(iw - 1);
    let y1 = (y1f.ceil() as i32 + pad).min(ih - 1);
    if x1 < x0 || y1 < y0 {
        return;
    }
    let (rw, rh) = ((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32);
    let patch = image::imageops::crop_imm(img, x0 as u32, y0 as u32, rw, rh).to_image();
    let fx = match o.eraser_mode {
        super::document::EraserMode::Blur => {
            imageproc::filter::gaussian_blur_f32(&patch, (w * 0.28).clamp(2.2, 10.0))
        }
        super::document::EraserMode::Pixelate => {
            let block = ((w as u32 / 3) + 4).clamp(4, 18);
            pixelate(&patch, block)
        }
        super::document::EraserMode::Color => return,
    };

    let mut segs: Vec<(Pos2, Pos2, f32, f32, f32, f32)> = Vec::new();
    if o.points.len() == 1 {
        let p = o.points[0];
        segs.push((p, p, p.x, p.y, p.x, p.y));
    } else {
        for pair in o.points.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            segs.push((a, b, a.x.min(b.x), a.y.min(b.y), a.x.max(b.x), a.y.max(b.y)));
        }
    }
    let reach = hw + feather + 2.0;

    for py in y0..=y1 {
        for px in x0..=x1 {
            let p = Pos2::new(px as f32, py as f32);
            let mut d = f32::INFINITY;
            for &(a, b, sx0, sy0, sx1, sy1) in &segs {
                if p.x < sx0 - reach || p.x > sx1 + reach || p.y < sy0 - reach || p.y > sy1 + reach
                {
                    continue;
                }
                let seg_d = match o.tip {
                    TipShape::Round => pt_seg_dist(p, a, b) - hw,
                    TipShape::Square => {
                        let ab = b - a;
                        let l2 = ab.length_sq();
                        let q = if l2 < 1e-6 {
                            a
                        } else {
                            a + ab * ((p - a).dot(ab) / l2).clamp(0.0, 1.0)
                        };
                        (p.x - q.x).abs().max((p.y - q.y).abs()) - hw
                    }
                };
                d = d.min(seg_d);
            }
            let cov = (((0.5 * feather - d) / feather).clamp(0.0, 1.0) * o.opacity).clamp(0.0, 1.0);
            if cov <= 1.0 / 255.0 {
                continue;
            }
            let dst = img.get_pixel(px as u32, py as u32).0;
            let src = fx.get_pixel((px - x0) as u32, (py - y0) as u32).0;
            let mix = |a: u8, b: u8| -> u8 {
                (f32::from(a) * (1.0 - cov) + f32::from(b) * cov)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            img.put_pixel(
                px as u32,
                py as u32,
                Rgba([
                    mix(dst[0], src[0]),
                    mix(dst[1], src[1]),
                    mix(dst[2], src[2]),
                    dst[3],
                ]),
            );
        }
    }
}

/// Plain text object: lines laid out horizontally in a scratch layer,
/// then rotated around baseline anchor `a` towards `b`.
/// Shared text geometry: `a`/`b` are the guide-line extremities (the
/// grey anchors). The text baseline is the centred, text-width segment
/// between them (green anchors) and the glyph box sits ABOVE it.
/// Returns (top-left origin of the rotated text quad, unit dir, w, h).
pub fn text_layout(o: &super::document::TextObj) -> (egui::Pos2, egui::Vec2, f32, f32) {
    let (w, h) = text_extent(o);
    let ab = egui::pos2(o.b.x, o.b.y) - egui::pos2(o.a.x, o.a.y);
    let d = if ab.length() < 1e-3 {
        egui::vec2(1.0, 0.0)
    } else {
        ab / ab.length()
    };
    let len = ab.length().max(w + 16.0);
    let ext = ((len - w) / 2.0).max(0.0);
    let gl = egui::pos2(o.a.x + d.x * ext, o.a.y + d.y * ext);
    // Quad top-left: `h` above the baseline along the perpendicular.
    (egui::pos2(gl.x + d.y * h, gl.y - d.x * h), d, w, h)
}

/// Point + unit tangent on the cubic Bézier (`a`, `ka`, `kb`, `b`).
pub fn cubic_eval(a: Pos2, ka: Pos2, kb: Pos2, b: Pos2, t: f32) -> (egui::Pos2, egui::Vec2) {
    let u = 1.0 - t;
    let p = egui::pos2(
        u * u * u * a.x + 3.0 * u * u * t * ka.x + 3.0 * u * t * t * kb.x + t * t * t * b.x,
        u * u * u * a.y + 3.0 * u * u * t * ka.y + 3.0 * u * t * t * kb.y + t * t * t * b.y,
    );
    let dx = 3.0 * u * u * (ka.x - a.x) + 6.0 * u * t * (kb.x - ka.x) + 3.0 * t * t * (b.x - kb.x);
    let dy = 3.0 * u * u * (ka.y - a.y) + 6.0 * u * t * (kb.y - ka.y) + 3.0 * t * t * (b.y - kb.y);
    let v = egui::vec2(dx, dy);
    let d = if v.length() < 1e-3 {
        let ab = b - a;
        if ab.length() < 1e-3 {
            egui::vec2(1.0, 0.0)
        } else {
            ab / ab.length()
        }
    } else {
        v / v.length()
    };
    (p, d)
}

/// Samples a cubic Bézier (`a`, `ka`, `kb`, `b`) into `(arc_s, point, unit_tangent)`.
pub fn cubic_table(
    a: Pos2,
    ka: Pos2,
    kb: Pos2,
    b: Pos2,
    n: usize,
) -> Vec<(f32, egui::Pos2, egui::Vec2)> {
    let mut out = Vec::with_capacity(n + 1);
    let mut s = 0.0f32;
    let (mut prev, d0) = cubic_eval(a, ka, kb, b, 0.0);
    out.push((0.0, prev, d0));
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let (p, d) = cubic_eval(a, ka, kb, b, t);
        s += (p - prev).length();
        out.push((s, p, d));
        prev = p;
    }
    out
}

/// Point + unit tangent on the cubic Bézier baseline (`a`, `ka`, `kb`, `b`).
pub fn bezier_eval(o: &super::document::TextObj, t: f32) -> (egui::Pos2, egui::Vec2) {
    cubic_eval(o.a, o.ka, o.kb, o.b, t)
}

/// Samples the cubic Bézier into `(arc_s, point, unit_tangent)`.
pub fn bezier_table(o: &super::document::TextObj, n: usize) -> Vec<(f32, egui::Pos2, egui::Vec2)> {
    cubic_table(o.a, o.ka, o.kb, o.b, n)
}

fn sample_at(tab: &[(f32, egui::Pos2, egui::Vec2)], s: f32) -> (egui::Pos2, egui::Vec2) {
    let s = s.clamp(0.0, tab.last().map_or(0.0, |e| e.0));
    let idx = tab
        .partition_point(|e| e.0 < s)
        .min(tab.len().saturating_sub(1));
    if idx == 0 {
        return (tab[0].1, tab[0].2);
    }
    let (s0, p0, d0) = tab[idx - 1];
    let (s1, p1, d1) = tab[idx];
    let f = if (s1 - s0).abs() < 1e-4 {
        0.0
    } else {
        (s - s0) / (s1 - s0)
    };
    let d = d0 + (d1 - d0) * f;
    let dn = if d.length() < 1e-3 {
        d0
    } else {
        d / d.length()
    };
    (p0 + (p1 - p0) * f, dn)
}

/// Plain text on a cubic Bézier baseline — rendered like the reference:
/// EACH CHARACTER stays rigid and is rotated at its own tangent along
/// the curve, rasterized at 2× supersampling with a smooth circular
/// outline dilation, soft drop shadow, and bilinear composite.
fn draw_text_obj(img: &mut RgbaImage, o: &super::document::TextObj) {
    if o.content.is_empty() {
        return;
    }
    let f = font_of(&o.font, o.bold);
    let scale = ab_glyph::PxScale::from(o.size);
    let tab = bezier_table(o, 160);
    let total = tab.last().map_or(0.0, |e| e.0);
    let lh = o.size * 1.2;
    let lines: Vec<&str> = o.content.lines().collect();
    let n_lines = lines.len().max(1);

    // Collect every character's rigid 2× stamp + placement on the curve
    // so we can paint all shadows first, then all outlines+fills (clean
    // overlap between neighbouring outlines, like the reference).
    struct Placed {
        stamp: RgbaImage,
        anchor: egui::Pos2,
        ang: f32,
    }
    let mut placed = Vec::new();
    for (li, line) in lines.iter().enumerate() {
        // Measure per-char advances via prefix widths (keeps kerning).
        let chars: Vec<char> = line.chars().collect();
        let mut xs = Vec::with_capacity(chars.len() + 1);
        xs.push(0.0f32);
        let mut prefix = String::new();
        for &ch in &chars {
            prefix.push(ch);
            let (pw, _) = drw::text_size(scale, f, &prefix);
            xs.push(pw as f32);
        }
        let line_w = *xs.last().unwrap_or(&0.0);
        let s0 = match o.align {
            TextAlign::Left | TextAlign::Justify => 12.0,
            TextAlign::Center => ((total - line_w) / 2.0).max(0.0),
            TextAlign::Right => (total - line_w - 12.0).max(0.0),
        };
        // Older lines stack higher above the baseline.
        let v_off = (n_lines - 1 - li) as f32 * lh;
        for (ci, &ch) in chars.iter().enumerate() {
            if ch.is_whitespace() {
                continue;
            }
            let cw = (xs[ci + 1] - xs[ci]).max(o.size * 0.25);
            let s_mid = s0 + xs[ci] + cw * 0.5;
            let (bp, d) = sample_at(&tab, s_mid);
            let anchor = egui::pos2(bp.x + d.y * v_off, bp.y - d.x * v_off);
            let stamp = render_glyph_2x(ch, f, o);
            placed.push(Placed {
                stamp,
                anchor,
                ang: d.y.atan2(d.x),
            });
        }
    }

    // Soft drop shadow pass (reference always casts a subtle shadow on
    // outlined curved text).
    if o.shadow > 0.0 || o.outline > 0.0 {
        for g in &placed {
            let sh = shadow_stamp_2x(&g.stamp);
            blit_stamp_2x(
                img,
                &sh,
                egui::pos2(g.anchor.x + 2.5, g.anchor.y + 3.0),
                g.ang,
                o.opacity * 0.45,
            );
        }
    }
    for g in &placed {
        blit_stamp_2x(img, &g.stamp, g.anchor, g.ang, o.opacity);
    }
}

/// Renders one character at 2× resolution with a smooth circular
/// outline (Euclidean distance dilation) and the glyph fill on top.
/// The stamp's anchor sits at `(width/2, height - pad)` (bottom-centre
/// on the baseline).
fn render_glyph_2x(ch: char, f: &ab_glyph::FontArc, o: &super::document::TextObj) -> RgbaImage {
    let s = ch.to_string();
    let sc2 = ab_glyph::PxScale::from(o.size * 2.0);
    let (gw, gh) = drw::text_size(sc2, f, &s);
    let r = (o.outline * 2.0).max(0.0);
    let pad = (r.ceil() as u32) + 8;
    let w = gw.max(4) + pad * 2;
    let h = (gh.max((o.size * 2.1) as u32)) + pad * 2;
    let mut glyph = RgbaImage::new(w, h);
    drw::draw_text_mut(
        &mut glyph,
        Rgba([255, 255, 255, 255]),
        pad as i32,
        pad as i32,
        sc2,
        f,
        &s,
    );
    if o.italic {
        glyph = slant(&glyph, 0.22);
    }
    let (w, h) = (glyph.width(), glyph.height());
    let mut out = RgbaImage::new(w, h);

    // Circular dilation for a smooth, gap-free outline with a 1-px AA
    // feather at radius `r`.
    if r > 0.2 {
        let ri = r.ceil() as i32 + 1;
        let oc = o.outline_color;
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut cov = 0.0f32;
                let y0 = (y - ri).max(0);
                let y1 = (y + ri).min(h as i32 - 1);
                let x0 = (x - ri).max(0);
                let x1 = (x + ri).min(w as i32 - 1);
                for ny in y0..=y1 {
                    let dy = (ny - y) as f32;
                    for nx in x0..=x1 {
                        let a = glyph.get_pixel(nx as u32, ny as u32).0[3];
                        if a == 0 {
                            continue;
                        }
                        let dist = ((nx - x) as f32).hypot(dy);
                        if dist <= r + 1.0 {
                            let edge = (r + 0.5 - dist).clamp(0.0, 1.0);
                            cov = cov.max((a as f32 / 255.0) * edge);
                            if cov >= 0.99 {
                                break;
                            }
                        }
                    }
                }
                if cov > 0.0 {
                    out.put_pixel(
                        x as u32,
                        y as u32,
                        Rgba([oc.r(), oc.g(), oc.b(), (cov * 255.0) as u8]),
                    );
                }
            }
        }
    }

    // Composite the glyph fill over the outline with AA alpha.
    let fc = o.color;
    for y in 0..h {
        for x in 0..w {
            let ga = glyph.get_pixel(x, y).0[3] as f32 / 255.0;
            if ga <= 0.0 {
                continue;
            }
            let p = out.get_pixel_mut(x, y);
            let da = p.0[3] as f32 / 255.0;
            let oa = ga + da * (1.0 - ga);
            if oa > 0.0 {
                p.0[0] = ((fc.r() as f32 * ga + p.0[0] as f32 * da * (1.0 - ga)) / oa) as u8;
                p.0[1] = ((fc.g() as f32 * ga + p.0[1] as f32 * da * (1.0 - ga)) / oa) as u8;
                p.0[2] = ((fc.b() as f32 * ga + p.0[2] as f32 * da * (1.0 - ga)) / oa) as u8;
                p.0[3] = (oa * 255.0) as u8;
            }
        }
    }
    out
}

/// Soft black silhouette of a 2× glyph stamp for the drop shadow.
fn shadow_stamp_2x(src: &RgbaImage) -> RgbaImage {
    let (w, h) = (src.width(), src.height());
    let mut mask = RgbaImage::new(w, h);
    for (x, y, p) in src.enumerate_pixels() {
        mask.put_pixel(x, y, Rgba([0, 0, 0, p.0[3]]));
    }
    imageproc::filter::gaussian_blur_f32(&mask, 2.2)
}

/// Composites a 2× supersampled glyph stamp rotated by `ang` around its
/// bottom-centre baseline anchor onto `dst` using a 2×2 sub-pixel grid +
/// bilinear sampling — crisp, anti-aliased edges at any angle.
fn blit_stamp_2x(dst: &mut RgbaImage, src: &RgbaImage, anchor: egui::Pos2, ang: f32, opacity: f32) {
    let (sin, cos) = ang.sin_cos();
    // Source coordinates are at 2× scale; the anchor sits near bottom-centre.
    let (sw, sh) = (src.width() as f32, src.height() as f32);
    let (ax, ay) = (sw * 0.5, sh - 10.0);
    // Rotated corners in 1× dst space.
    let mut x1 = f32::INFINITY;
    let mut y1 = f32::INFINITY;
    let mut x2 = f32::NEG_INFINITY;
    let mut y2 = f32::NEG_INFINITY;
    for (sx, sy) in [(0.0f32, 0.0f32), (sw, 0.0), (sw, sh), (0.0, sh)] {
        let (lx, ly) = ((sx - ax) * 0.5, (sy - ay) * 0.5);
        let rx = anchor.x + lx * cos - ly * sin;
        let ry = anchor.y + lx * sin + ly * cos;
        x1 = x1.min(rx);
        y1 = y1.min(ry);
        x2 = x2.max(rx);
        y2 = y2.max(ry);
    }
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    for dy in (y1.floor() as i64 - 1)..=(y2.ceil() as i64 + 1) {
        if dy < 0 || dy >= dh {
            continue;
        }
        for dx in (x1.floor() as i64 - 1)..=(x2.ceil() as i64 + 1) {
            if dx < 0 || dx >= dw {
                continue;
            }
            // 2×2 RGSS sub-pixel average + bilinear lookup in the 2× stamp.
            let mut acc = [0.0f32; 4];
            for (ox, oy) in [
                (-0.25f32, -0.25f32),
                (0.25, -0.25),
                (-0.25, 0.25),
                (0.25, 0.25),
            ] {
                let (qx, qy) = (
                    dx as f32 + 0.5 + ox - anchor.x,
                    dy as f32 + 0.5 + oy - anchor.y,
                );
                let sx = (qx * cos + qy * sin) * 2.0 + ax;
                let sy = (-qx * sin + qy * cos) * 2.0 + ay;
                let sp = sample_bilinear(src, sx, sy);
                let sa = sp[3];
                acc[0] += sp[0] * sa;
                acc[1] += sp[1] * sa;
                acc[2] += sp[2] * sa;
                acc[3] += sa;
            }
            let sa = (acc[3] * 0.25) * opacity;
            if sa <= 1.0 / 255.0 {
                continue;
            }
            let inv = 1.0 / acc[3].max(1e-5);
            let (sr, sg, sb) = (acc[0] * inv, acc[1] * inv, acc[2] * inv);
            let dp = dst.get_pixel_mut(dx as u32, dy as u32);
            let da = dp.0[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            dp.0[0] = ((sr * sa + dp.0[0] as f32 * da * (1.0 - sa)) / oa) as u8;
            dp.0[1] = ((sg * sa + dp.0[1] as f32 * da * (1.0 - sa)) / oa) as u8;
            dp.0[2] = ((sb * sa + dp.0[2] as f32 * da * (1.0 - sa)) / oa) as u8;
            dp.0[3] = (oa * 255.0).min(255.0) as u8;
        }
    }
}

/// Bilinear RGBA fetch in [0..255] rgb, [0..1] alpha; transparent outside.
fn sample_bilinear(img: &RgbaImage, x: f32, y: f32) -> [f32; 4] {
    let (w, h) = (img.width() as i32, img.height() as i32);
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    if x0 < -1 || y0 < -1 || x0 >= w || y0 >= h {
        return [0.0; 4];
    }
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let fetch = |px: i32, py: i32| -> [f32; 4] {
        if px < 0 || py < 0 || px >= w || py >= h {
            [0.0; 4]
        } else {
            let p = img.get_pixel(px as u32, py as u32).0;
            [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32 / 255.0]
        }
    };
    let (p00, p10, p01, p11) = (
        fetch(x0, y0),
        fetch(x0 + 1, y0),
        fetch(x0, y0 + 1),
        fetch(x0 + 1, y0 + 1),
    );
    let mut out = [0.0; 4];
    for i in 0..4 {
        let a = p00[i] * (1.0 - fx) + p10[i] * fx;
        let b = p01[i] * (1.0 - fx) + p11[i] * fx;
        out[i] = a * (1.0 - fy) + b * fy;
    }
    out
}

/// Faux-italic: shears the layer right-towards-top by `factor`.
fn slant(src: &RgbaImage, factor: f32) -> RgbaImage {
    let (w, h) = (src.width(), src.height());
    let mut out = RgbaImage::new(w, h);
    for y in 0..h {
        let off = ((h - 1 - y) as f32 * factor) as i32;
        for x in 0..w {
            let sx = x as i32 - off;
            if (0..w as i32).contains(&sx) {
                out.put_pixel(x, y, *src.get_pixel(sx as u32, y));
            }
        }
    }
    out
}

/// Speech-bubble box (rect/oval) + wrapped text.
fn draw_textbox(img: &mut RgbaImage, o: &super::document::BoxObj) {
    // Tail first (under the box rim).
    if let Some(t) = o.tail {
        let (cx, cy) = (
            o.rect.x as f32 + o.rect.width as f32 / 2.0,
            o.rect.y as f32 + o.rect.height as f32 / 2.0,
        );
        let (dx, dy) = (t.x - cx, t.y - cy);
        let len = dx.hypot(dy).max(0.001);
        let (nx, ny) = (-dy / len, dx / len);
        let w = 10.0f32;
        drw::draw_polygon_mut(
            img,
            &points_i32(&[
                imageproc::point::Point::new(cx + nx * w, cy + ny * w),
                imageproc::point::Point::new(cx - nx * w, cy - ny * w),
                imageproc::point::Point::new(t.x, t.y),
            ]),
            c32(o.bg),
        );
    }
    draw_shape(
        img,
        o.rect,
        if o.oval {
            AreaShape::Ellipse
        } else {
            AreaShape::Round
        },
        o.radius,
        Some(o.bg),
        o.border,
        o.border_w,
        Dash::Solid,
        1.0,
    );
    // Wrapped text inside the box, padded.
    let pad = 8.0f32;
    let inner_w = (o.rect.width as f32 - 2.0 * pad).max(8.0);
    let lines = wrap_text(&o.content, o.size, o.bold, inner_w);
    let lh = o.size * 1.25 + o.line_gap;
    let text_h = lines.len() as f32 * lh;
    let free = (o.rect.height as f32 - 2.0 * pad - text_h).max(0.0);
    let y0 = match o.valign {
        super::document::BoxVAlign::Top => o.rect.y as f32 + pad,
        super::document::BoxVAlign::Center => o.rect.y as f32 + pad + free / 2.0,
        super::document::BoxVAlign::Bottom => o.rect.y as f32 + pad + free,
    };
    // Draw into a scratch layer so the fake-italic shear stays inside
    // the box bounds.
    let (sw, sh) = (
        inner_w.ceil().max(8.0) as u32,
        (text_h.ceil() + 4.0).max(8.0) as u32,
    );
    let mut layer = RgbaImage::new(sw, sh);
    let mut y = 0.0f32;
    for line in &lines {
        let (tw, _) = drw::text_size(ab_glyph::PxScale::from(o.size), font(o.bold), line);
        let tx = match o.align {
            TextAlign::Left | TextAlign::Justify => 0.0,
            TextAlign::Center => (inner_w - tw as f32) / 2.0,
            TextAlign::Right => inner_w - tw as f32,
        };
        if o.align == TextAlign::Justify {
            // Spread word gaps to fill the inner width (last line and
            // single-word lines fall back to left).
            let words: Vec<&str> = line.split_whitespace().collect();
            let last = lines.last().is_some_and(|l| l == line);
            if words.len() > 1 && !last {
                let words_w: f32 = words
                    .iter()
                    .map(|wd| {
                        drw::text_size(ab_glyph::PxScale::from(o.size), font(o.bold), wd).0 as f32
                    })
                    .sum();
                let gap = (inner_w - words_w).max(0.0) / (words.len() - 1) as f32;
                let mut wx = 0.0f32;
                for wd in &words {
                    drw::draw_text_mut(
                        &mut layer,
                        c32(o.fg),
                        wx as i32,
                        y as i32,
                        ab_glyph::PxScale::from(o.size),
                        font(o.bold),
                        wd,
                    );
                    wx += drw::text_size(ab_glyph::PxScale::from(o.size), font(o.bold), wd).0
                        as f32
                        + gap;
                }
                y += lh;
                continue;
            }
        }
        drw::draw_text_mut(
            &mut layer,
            c32(o.fg),
            tx as i32,
            y as i32,
            ab_glyph::PxScale::from(o.size),
            font(o.bold),
            line,
        );
        y += lh;
    }
    if o.italic {
        layer = slant(&layer, 0.22);
    }
    // Alpha-composite the text layer into the box at (x+pad, y0).
    for (sx, sy, p) in layer.enumerate_pixels() {
        let a = p.0[3] as f32 / 255.0 * o.opacity;
        if a <= 0.0 {
            continue;
        }
        let (dx, dy) = (
            (o.rect.x as f32 + pad + sx as f32) as i64,
            (y0 + sy as f32) as i64,
        );
        if dx < 0 || dy < 0 || dx >= img.width() as i64 || dy >= img.height() as i64 {
            continue;
        }
        let dp = img.get_pixel_mut(dx as u32, dy as u32);
        let da = dp.0[3] as f32 / 255.0;
        let oa = a + da * (1.0 - a);
        if oa > 0.0 {
            dp.0[0] = ((p.0[0] as f32 * a + dp.0[0] as f32 * da * (1.0 - a)) / oa) as u8;
            dp.0[1] = ((p.0[1] as f32 * a + dp.0[1] as f32 * da * (1.0 - a)) / oa) as u8;
            dp.0[2] = ((p.0[2] as f32 * a + dp.0[2] as f32 * da * (1.0 - a)) / oa) as u8;
            dp.0[3] = (oa * 255.0) as u8;
        }
    }
}

/// Wraps `content` to lines fitting `max_w` px at `size`.
fn wrap_text(content: &str, size: f32, bold: bool, max_w: f32) -> Vec<String> {
    let mut out = Vec::new();
    let scale = ab_glyph::PxScale::from(size);
    for para in content.lines() {
        let mut cur = String::new();
        for word in para.split(' ') {
            let cand = if cur.is_empty() {
                word.to_owned()
            } else {
                format!("{cur} {word}")
            };
            let (w, _) = drw::text_size(scale, font(bold), &cand);
            if w as f32 > max_w && !cur.is_empty() {
                out.push(cur);
                cur = word.to_owned();
            } else {
                cur = cand;
            }
        }
        out.push(cur);
    }
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// A numbered/lettered step badge rendered with analytical signed-distance
/// anti-aliasing (concentric inner fill + ring + soft drop shadow) and a
/// 4× supersampled, ink-centred counter glyph.
fn draw_step(img: &mut RgbaImage, o: &super::document::StepObj) {
    let r_in = match o.size {
        StepSize::Small => 11.0_f32,
        StepSize::Medium => 15.0_f32,
        StepSize::Large => 21.0_f32,
    };
    let ring_px = o.ring.max(0.0) * 0.65;
    let r_out = r_in + ring_px;
    let cr_in = (r_in * 0.26).max(2.5);
    let core = (r_in - cr_in).max(0.0);
    let d_inner = |dx: f32, dy: f32| -> f32 {
        match o.style {
            StepStyle::Circle => dx.hypot(dy) - r_in,
            StepStyle::Square => {
                let (qx, qy) = (dx.abs() - core, dy.abs() - core);
                qx.max(0.0).hypot(qy.max(0.0)) + qx.min(0.0).max(qy.min(0.0)) - cr_in
            }
        }
    };

    // Rasterize the counter glyph at 4× and find its true ink bounding box
    // so digits and letters sit at the exact geometric centre of the badge.
    const SS: i32 = 4;
    let label = step_label(o.mode, o.count, o.lower);
    let fs4 = r_in * 1.18 * SS as f32;
    let (tw, th) = drw::text_size(ab_glyph::PxScale::from(fs4), font(true), &label);
    let gw = (tw + 16).max(16);
    let gh = (th + 16).max(16);
    let mut gbuf = RgbaImage::new(gw, gh);
    drw::draw_text_mut(
        &mut gbuf,
        Rgba([255, 255, 255, 255]),
        8,
        8,
        ab_glyph::PxScale::from(fs4),
        font(true),
        &label,
    );
    let (mut gx0, mut gy0, mut gx1, mut gy1) = (gw as i32, gh as i32, 0i32, 0i32);
    for (x, y, p) in gbuf.enumerate_pixels() {
        if p.0[3] > 16 {
            gx0 = gx0.min(x as i32);
            gy0 = gy0.min(y as i32);
            gx1 = gx1.max(x as i32);
            gy1 = gy1.max(y as i32);
        }
    }
    let (gcx, gcy) = if gx0 <= gx1 && gy0 <= gy1 {
        ((gx0 + gx1) as f32 * 0.5, (gy0 + gy1) as f32 * 0.5)
    } else {
        (gw as f32 * 0.5, gh as f32 * 0.5)
    };

    let sample_glyph = |dx: f32, dy: f32| -> f32 {
        let bx = (gcx + dx * SS as f32 - 1.5).round() as i32;
        let by = (gcy + dy * SS as f32 - 1.5).round() as i32;
        if bx + SS < 0 || by + SS < 0 || bx >= gw as i32 || by >= gh as i32 {
            return 0.0;
        }
        let mut sum = 0u32;
        for sy in 0..SS {
            let py = by + sy;
            if py < 0 || py >= gh as i32 {
                continue;
            }
            for sx in 0..SS {
                let px = bx + sx;
                if px >= 0 && px < gw as i32 {
                    sum += u32::from(gbuf.get_pixel(px as u32, py as u32).0[3]);
                }
            }
        }
        sum as f32 / (255.0 * (SS * SS) as f32)
    };

    let (bg, fg, rc) = (o.bg, o.fg, o.ring_color);
    let pad = (r_out + if o.shadow > 0.0 { 8.0 } else { 2.0 }).ceil() as i32;
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    let x0 = (o.pos.x.floor() as i32 - pad).max(0);
    let y0 = (o.pos.y.floor() as i32 - pad).max(0);
    let x1 = (o.pos.x.ceil() as i32 + pad).min(iw - 1);
    let y1 = (o.pos.y.ceil() as i32 + pad).min(ih - 1);

    for py in y0..=y1 {
        let dy = py as f32 - o.pos.y;
        for px in x0..=x1 {
            let dx = px as f32 - o.pos.x;
            let din = d_inner(dx, dy);
            let dout = din - ring_px;
            let cov_out = (0.5 - dout).clamp(0.0, 1.0);

            let sh_a = if o.shadow > 0.0 && cov_out < 1.0 {
                let dsh = d_inner(dx - 1.0, dy - 1.8) - ring_px;
                let t = (1.0 - (dsh + 0.8) / 5.2).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t) * 0.40
            } else {
                0.0
            };

            let total_a = cov_out + sh_a * (1.0 - cov_out);
            if total_a <= 1.0 / 255.0 {
                continue;
            }

            let (mut br, mut bg_c, mut bb) = (0.0_f32, 0.0_f32, 0.0_f32);
            if cov_out > 0.0 {
                let cov_in = if ring_px > 0.0 {
                    (0.5 - din).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                br = f32::from(rc.r()) * (1.0 - cov_in) + f32::from(bg.r()) * cov_in;
                bg_c = f32::from(rc.g()) * (1.0 - cov_in) + f32::from(bg.g()) * cov_in;
                bb = f32::from(rc.b()) * (1.0 - cov_in) + f32::from(bg.b()) * cov_in;

                if cov_in > 0.0 {
                    let ga = (sample_glyph(dx, dy) * cov_in).clamp(0.0, 1.0);
                    if ga > 0.0 {
                        br = br * (1.0 - ga) + f32::from(fg.r()) * ga;
                        bg_c = bg_c * (1.0 - ga) + f32::from(fg.g()) * ga;
                        bb = bb * (1.0 - ga) + f32::from(fg.b()) * ga;
                    }
                }
            }

            let w_badge = cov_out / total_a;
            let out_r = (br * w_badge).round().clamp(0.0, 255.0) as u8;
            let out_g = (bg_c * w_badge).round().clamp(0.0, 255.0) as u8;
            let out_b = (bb * w_badge).round().clamp(0.0, 255.0) as u8;
            let out_a = (total_a * 255.0).round().clamp(0.0, 255.0) as u8;
            img.put_pixel(px as u32, py as u32, Rgba([out_r, out_g, out_b, out_a]));
        }
    }
}

/// The counter glyph for a step (`1`/`2`/`3` or `A`/`B`/`C`).
fn step_label(mode: StepMode, count: u32, lower: bool) -> String {
    match mode {
        StepMode::Numbers => count.to_string(),
        StepMode::Letters => {
            let base = if lower { 'a' } else { 'A' } as u32;
            String::from(
                char::from_u32(base + count.saturating_sub(1) % 26).unwrap_or(base as u8 as char),
            )
        }
    }
}

/// An inserted bitmap, resized into its bounds and composited.
fn draw_picture(img: &mut RgbaImage, o: &super::document::PicObj) {
    if o.rect.width == 0 || o.rect.height == 0 {
        return;
    }
    let scaled = image::imageops::resize(
        &o.image,
        o.rect.width,
        o.rect.height,
        image::imageops::FilterType::Lanczos3,
    );
    composite(img, &scaled, o.rect.x as i64, o.rect.y as i64, 1.0);
}

/// An embedded emoji sticker — the same Lanczos3 + composite path as
/// [`draw_picture`], but the pixels come from the embedded set via the
/// stored id. An unknown/undecodable id renders the neutral
/// placeholder — never an error, never a panic.
fn draw_emoji(img: &mut RgbaImage, o: &super::document::EmojiObj) {
    if o.rect.width == 0 || o.rect.height == 0 {
        return;
    }
    match super::emoji::image(o.id) {
        Some(src) => {
            let scaled = image::imageops::resize(
                src,
                o.rect.width,
                o.rect.height,
                image::imageops::FilterType::Lanczos3,
            );
            composite(img, &scaled, o.rect.x as i64, o.rect.y as i64, 1.0);
        }
        None => draw_emoji_placeholder(img, o.rect),
    }
}

/// Neutral marker standing in for an emoji id without an embedded
/// image: dashed grey square + diagonal cross (theme-independent, like
/// every document pixel).
fn draw_emoji_placeholder(img: &mut RgbaImage, rect: PhysicalRect) {
    let grey = Color32::from_rgb(150, 150, 150);
    draw_shape_sdf(
        img,
        rect,
        false,
        0.0,
        Some(Color32::from_rgba_unmultiplied(150, 150, 150, 48)),
        grey,
        2.0,
        super::document::Dash::Dash,
        0.0,
        Color32::TRANSPARENT,
        0.0,
    );
    // Diagonal cross inside the box, inset ~20 %, 2 px thick.
    let (x0, y0) = (rect.x as i64, rect.y as i64);
    let (w, h) = (rect.width as i64, rect.height as i64);
    let (ix, iy) = (x0 + w / 5, y0 + h / 5);
    let (iw, ih) = (w * 3 / 5, h * 3 / 5);
    let n = iw.min(ih).max(1);
    let (dw, dh) = (img.width() as i64, img.height() as i64);
    let px = image::Rgba([150, 150, 150, 255]);
    for t in 0..=n {
        for (cx, cy) in [
            (ix + t * iw / n, iy + t * ih / n),
            (ix + t * iw / n, iy + ih - t * ih / n),
        ] {
            for dx in 0..2i64 {
                for dy in 0..2i64 {
                    let (x, y) = (cx + dx, cy + dy);
                    if (0..dw).contains(&x) && (0..dh).contains(&y) {
                        img.put_pixel(x as u32, y as u32, px);
                    }
                }
            }
        }
    }
}

/// Blur/pixelate applied to a document region.
fn apply_blur(img: &mut RgbaImage, o: &super::document::BlurObj) {
    let (x, y, w, h) = clip_rect(o.rect, img.width(), img.height());
    if w == 0 || h == 0 {
        return;
    }
    let mut patch = image::imageops::crop_imm(img, x, y, w, h).to_image();
    patch = match o.style {
        BlurStyle::Pixelate => {
            let block = ((o.strength as u32 / 12) + 3).max(2);
            pixelate(&patch, block)
        }
        BlurStyle::Blur => {
            let sigma = o.strength as f32 / 12.0 + 0.8;
            imageproc::filter::gaussian_blur_f32(&patch, sigma)
        }
    };
    if o.shape != AreaShape::Rect {
        mask_shape(&mut patch, o.shape);
    }
    composite(img, &patch, x as i64, y as i64, 1.0);
}

/// Block-average mosaic for the pixelate blur style.
fn pixelate(src: &RgbaImage, block: u32) -> RgbaImage {
    let (w, h) = (src.width(), src.height());
    let mut out = src.clone();
    for by in (0..h).step_by(block as usize) {
        for bx in (0..w).step_by(block as usize) {
            let (mut r, mut g, mut b, mut a, mut n) = (0u64, 0u64, 0u64, 0u64, 0u64);
            for yy in by..(by + block).min(h) {
                for xx in bx..(bx + block).min(w) {
                    let p = src.get_pixel(xx, yy).0;
                    r += p[0] as u64;
                    g += p[1] as u64;
                    b += p[2] as u64;
                    a += p[3] as u64;
                    n += 1;
                }
            }
            let avg = Rgba([(r / n) as u8, (g / n) as u8, (b / n) as u8, (a / n) as u8]);
            for yy in by..(by + block).min(h) {
                for xx in bx..(bx + block).min(w) {
                    out.put_pixel(xx, yy, avg);
                }
            }
        }
    }
    out
}

/// Zeros the alpha outside the given shape (ellipse/round clip).
fn mask_shape(patch: &mut RgbaImage, shape: AreaShape) {
    let (w, h) = (patch.width() as f32, patch.height() as f32);
    let (cx, cy, rx, ry) = (w / 2.0, h / 2.0, w / 2.0, h / 2.0);
    for y in 0..patch.height() {
        for x in 0..patch.width() {
            let inside = match shape {
                AreaShape::Ellipse => {
                    let dx = (x as f32 + 0.5 - cx) / rx.max(1.0);
                    let dy = (y as f32 + 0.5 - cy) / ry.max(1.0);
                    dx * dx + dy * dy <= 1.0
                }
                AreaShape::Round => {
                    let r = 12.0f32.min(w / 2.0).min(h / 2.0);
                    let qx = (x as f32 + 0.5).clamp(r, w - r);
                    let qy = (y as f32 + 0.5).clamp(r, h - r);
                    let (dx, dy) = (x as f32 + 0.5 - qx, y as f32 + 0.5 - qy);
                    dx * dx + dy * dy <= r * r
                }
                AreaShape::Rect => true,
            };
            if !inside {
                patch.get_pixel_mut(x, y).0[3] = 0;
            }
        }
    }
}

/// Retints a layer: every pixel becomes `color` at its own alpha.
fn tint_alpha(src: &RgbaImage, color: Rgba<u8>) -> RgbaImage {
    let mut out = src.clone();
    for p in out.pixels_mut() {
        let a = p.0[3] as f32 / 255.0 * color.0[3] as f32 / 255.0;
        *p = Rgba([color.0[0], color.0[1], color.0[2], (a * 255.0) as u8]);
    }
    out
}

/// Alpha-composites `src` over `dst` at offset `(ox, oy)` with a
/// global alpha multiplier.
fn composite(dst: &mut RgbaImage, src: &RgbaImage, ox: i64, oy: i64, alpha_mul: f32) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    for y in 0..src.height() as i64 {
        let dy = y + oy;
        if dy < 0 || dy >= dh {
            continue;
        }
        for x in 0..src.width() as i64 {
            let dx = x + ox;
            if dx < 0 || dx >= dw {
                continue;
            }
            let mut s = *src.get_pixel(x as u32, y as u32);
            if alpha_mul < 1.0 {
                s.0[3] = (s.0[3] as f32 * alpha_mul) as u8;
            }
            if s.0[3] == 0 {
                continue;
            }
            let d = dst.get_pixel_mut(dx as u32, dy as u32);
            let sa = s.0[3] as f32 / 255.0;
            for ch in 0..3 {
                d.0[ch] = (s.0[ch] as f32 * sa + d.0[ch] as f32 * (1.0 - sa)) as u8;
            }
            d.0[3] = (sa + d.0[3] as f32 / 255.0 * (1.0 - sa))
                .min(1.0)
                .mul_add(255.0, 0.5) as u8;
        }
    }
}

/// `composite` restricted to `(x, y, w, h)` of `src` at the same
/// position — the bounded variant used for the shared scratch layer:
/// pixels outside the object's bounds are transparent by construction.
fn composite_in(
    dst: &mut RgbaImage,
    src: &RgbaImage,
    alpha_mul: f32,
    region: (u32, u32, u32, u32),
) {
    let (x0, y0, w, h) = region;
    let (x1, y1) = ((x0 + w).min(src.width()), (y0 + h).min(src.height()));
    for y in y0..y1 {
        for x in x0..x1 {
            let mut s = *src.get_pixel(x, y);
            if alpha_mul < 1.0 {
                s.0[3] = (s.0[3] as f32 * alpha_mul) as u8;
            }
            if s.0[3] == 0 {
                continue;
            }
            let d = dst.get_pixel_mut(x, y);
            let sa = s.0[3] as f32 / 255.0;
            for ch in 0..3 {
                d.0[ch] = (s.0[ch] as f32 * sa + d.0[ch] as f32 * (1.0 - sa)) as u8;
            }
            d.0[3] = (sa + d.0[3] as f32 / 255.0 * (1.0 - sa))
                .min(1.0)
                .mul_add(255.0, 0.5) as u8;
        }
    }
}

/// Clips a `PhysicalRect` to image bounds → (x, y, w, h).
fn clip_rect(r: PhysicalRect, w: u32, h: u32) -> (u32, u32, u32, u32) {
    let x = r.x.clamp(0, w as i32) as u32;
    let y = r.y.clamp(0, h as i32) as u32;
    let w2 = (r.width as i64 + r.x as i64 - x as i64).clamp(0, w as i64 - x as i64) as u32;
    let h2 = (r.height as i64 + r.y as i64 - y as i64).clamp(0, h as i64 - y as i64) as u32;
    (x, y, w2.min(w - x), h2.min(h - y))
}

/// `Color32` → `Rgba<u8>`.
fn c32(c: Color32) -> Rgba<u8> {
    Rgba([c.r(), c.g(), c.b(), c.a()])
}

/// `Color32` with a forced alpha.
fn with_alpha(c: Color32, a: u8) -> Rgba<u8> {
    Rgba([c.r(), c.g(), c.b(), a])
}

#[cfg(test)]
mod tests {
    use super::super::document::{
        AreaObj, BlurObj, BoxObj, BoxVAlign, EllipseObj, EmojiObj, LineObj, PicObj, RectObj,
        StepObj, TextObj,
    };
    use super::*;
    use crate::capture::{Capture, MonitorInfo};
    use egui::pos2;

    /// A deterministic patterned base — compositing against a flat
    /// colour would hide half the blend bugs.
    pub(super) fn base() -> Capture {
        let mut img = RgbaImage::new(340, 260);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = Rgba([
                ((x % 64) * 4) as u8,
                ((y % 64) * 4) as u8,
                (((x + y) % 128) * 2) as u8,
                255,
            ]);
        }
        Capture {
            image: std::sync::Arc::new(img),
            scale_factor: 1.0,
            monitor: MonitorInfo {
                id: 0,
                name: String::new(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: 340,
                    height: 260,
                },
                scale_factor: 1.0,
                is_primary: true,
            },
        }
    }

    pub(super) fn anno(id: u64, kind: AnnotationKind) -> Annotation {
        Annotation { id, kind }
    }

    pub(super) fn doc_with(annos: Vec<Annotation>) -> Document {
        let mut doc = Document::new(base());
        doc.annotations = annos;
        doc
    }

    /// Pixel-exact comparison with a first-mismatch report — `assert_eq!`
    /// on the raw buffers would dump a megabyte on failure.
    fn assert_same_pixels(a: &RgbaImage, b: &RgbaImage) {
        assert_eq!(a.dimensions(), b.dimensions());
        let mut diffs = 0usize;
        let mut max_delta = 0i32;
        let mut first = None;
        for (i, (x, y)) in a.as_raw().iter().zip(b.as_raw().iter()).enumerate() {
            if x != y {
                diffs += 1;
                max_delta = max_delta.max((*x as i32 - *y as i32).abs());
                if first.is_none() {
                    first = Some(i);
                }
            }
        }
        if let Some(i) = first {
            let (x, y) = ((i / 4) % a.width() as usize, (i / 4) / a.width() as usize);
            panic!(
                "{diffs} channel diffs (max delta {max_delta}); first at ({x},{y}) ch {}: {} vs {}",
                i % 4,
                a.as_raw()[i],
                b.as_raw()[i]
            );
        }
    }

    /// The bounded-layer path must reproduce the old full-frame
    /// rasterization EXACTLY — zero tolerance: the same code path
    /// serves canvas, thumbnails and export, so any drift is a
    /// rendering bug, not a quality trade-off.
    fn assert_render_parity(annos: Vec<Annotation>) {
        let doc = doc_with(annos);
        assert_same_pixels(&render_to_image(&doc), &render_to_image_ref(&doc));
    }

    /// Reference implementation of `render_one`: the pre-bounded-layer
    /// path, kept ONLY as the test oracle. It rasterizes every layered
    /// object into a full-frame transparent layer like the code did
    /// before [`layer_bounds`].
    fn render_one_ref(img: &mut RgbaImage, anno: &Annotation) {
        match &anno.kind {
            AnnotationKind::Blur(o) => apply_blur(img, o),
            AnnotationKind::Freehand(o)
                if o.eraser_mode != super::super::document::EraserMode::Color =>
            {
                apply_freehand_effect(img, o);
            }
            _ => {
                let mut layer = RgbaImage::new(img.width(), img.height());
                let (shadow, opacity) = object_layer(&mut layer, anno);
                if shadow > 0.0 {
                    let dark = tint_alpha(&layer, Rgba([0, 0, 0, 90]));
                    composite(img, &dark, shadow as i64, shadow as i64, 1.0);
                }
                composite(img, &layer, 0, 0, opacity);
            }
        }
    }

    /// Reference [`render_to_image`], oracle for the parity tests.
    pub(super) fn render_to_image_ref(doc: &Document) -> RgbaImage {
        let mut img = doc.base.image.as_ref().clone();
        for anno in &doc.annotations {
            render_one_ref(&mut img, anno);
        }
        img
    }

    /// Stroked + filled rects: radius, outline, shadow, dash and a
    /// transparent-fill variant — plus one clipped by the image edge.
    #[test]
    fn parity_rects() {
        let mk = |x, y| RectObj {
            rect: PhysicalRect {
                x,
                y,
                width: 120,
                height: 80,
            },
            stroke: Color32::from_rgb(230, 70, 60),
            stroke_w: 6.0,
            dash: Dash::Dash,
            fill: Some(Color32::from_rgba_premultiplied(60, 120, 220, 90)),
            radius: 10.0,
            outline: 3.0,
            outline_color: Color32::WHITE,
            opacity: 0.9,
            shadow: 5.0,
        };
        let mut border_only = mk(180, 140);
        border_only.fill = None;
        border_only.stroke_w = 12.0;
        border_only.dash = Dash::Dot;
        assert_render_parity(vec![
            anno(1, AnnotationKind::Rect(mk(20, 30))),
            anno(2, AnnotationKind::Rect(border_only)),
            anno(3, AnnotationKind::Rect(mk(290, 210))), // clipped at edge
        ]);
    }

    /// Stroked and filled ellipses, dashed and outlined.
    #[test]
    fn parity_ellipses() {
        let mk = |x, y, fill| EllipseObj {
            rect: PhysicalRect {
                x,
                y,
                width: 150,
                height: 100,
            },
            stroke: Color32::from_rgb(55, 142, 240),
            stroke_w: 8.0,
            dash: Dash::DashDot,
            fill,
            outline: 2.0,
            outline_color: Color32::BLACK,
            opacity: 1.0,
            shadow: 4.0,
        };
        assert_render_parity(vec![
            anno(1, AnnotationKind::Ellipse(mk(30, 30, None))),
            anno(
                2,
                AnnotationKind::Ellipse(mk(150, 120, Some(Color32::from_rgb(40, 200, 120)))),
            ),
        ]);
    }

    /// Bézier lines with arrow/dot/ring ends, thick and dashed.
    #[test]
    fn parity_lines() {
        let mk = |start, end| LineObj {
            a: pos2(20.0, 200.0),
            b: pos2(320.0, 60.0),
            ka: pos2(120.0, 260.0),
            kb: pos2(240.0, 20.0),
            color: Color32::from_rgb(255, 170, 30),
            width: 10.0,
            dash: Dash::DashDot,
            start,
            end,
            outline: 3.0,
            outline_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 4.0,
        };
        assert_render_parity(vec![
            anno(1, AnnotationKind::Line(mk(LineEnd::Arrow, LineEnd::Arrow))),
            anno(2, AnnotationKind::Line(mk(LineEnd::Dot, LineEnd::Ring))),
            anno(3, AnnotationKind::Line(mk(LineEnd::None, LineEnd::None))),
        ]);
    }

    /// Freehand colour strokes: soft feather, outline, shadow, dash.
    #[test]
    fn parity_freehand() {
        let points: Vec<Pos2> = (0..40)
            .map(|i| pos2(20.0 + i as f32 * 8.0, 130.0 + (i as f32 * 0.7).sin() * 60.0))
            .collect();
        assert_render_parity(vec![anno(
            1,
            AnnotationKind::Freehand(FreeObj {
                points,
                color: Color32::from_rgba_premultiplied(200, 40, 60, 200),
                width: 12.0,
                tip: TipShape::Round,
                dash: Dash::Solid,
                outline: 2.0,
                outline_color: Color32::WHITE,
                shadow: 3.0,
                eraser_mode: EraserMode::Color,
                soft: true,
                opacity: 0.7,
            }),
        )]);
    }

    /// Zone highlighter in all three shapes.
    #[test]
    fn parity_highlight() {
        let mk = |x, y, shape| {
            AnnotationKind::HighlightArea(AreaObj {
                rect: PhysicalRect {
                    x,
                    y,
                    width: 200,
                    height: 50,
                },
                color: Color32::from_rgb(255, 235, 60),
                shape,
                border: Some(Color32::from_rgb(180, 140, 0)),
                dash: Dash::Solid,
                soft: true,
                opacity: 0.35,
            })
        };
        assert_render_parity(vec![
            anno(1, mk(20, 30, AreaShape::Rect)),
            anno(2, mk(60, 100, AreaShape::Round)),
            anno(3, mk(100, 170, AreaShape::Ellipse)),
        ]);
    }

    /// Text on an angled Bézier baseline with outline and shadow —
    /// Inter is embedded so glyphs rasterize deterministically.
    #[test]
    fn parity_text() {
        assert_render_parity(vec![anno(
            1,
            AnnotationKind::Text(TextObj {
                a: pos2(30.0, 200.0),
                b: pos2(310.0, 120.0),
                ka: pos2(60.0, 200.0),
                kb: pos2(280.0, 120.0),
                content: "Héllo wörld — ¿Qué?".to_owned(),
                font: "Inter".to_owned(),
                color: Color32::WHITE,
                size: 40.0,
                bold: true,
                italic: false,
                align: TextAlign::Left,
                outline: 2.0,
                outline_color: Color32::BLACK,
                opacity: 1.0,
                shadow: 3.0,
            }),
        )]);
    }

    /// Speech boxes, one with the tail apex far outside the rect.
    #[test]
    fn parity_textbox() {
        let mk = |rect, oval, tail| BoxObj {
            rect,
            oval,
            content: "note".to_owned(),
            bg: Color32::from_rgb(250, 250, 250),
            border: Color32::from_rgb(30, 30, 30),
            border_w: 2.0,
            radius: 8.0,
            fg: Color32::BLACK,
            size: 18.0,
            bold: false,
            italic: false,
            align: TextAlign::Center,
            valign: BoxVAlign::Center,
            line_gap: 4.0,
            tail,
            opacity: 1.0,
            shadow: 5.0,
        };
        assert_render_parity(vec![
            anno(
                1,
                AnnotationKind::TextBox(mk(
                    PhysicalRect {
                        x: 40,
                        y: 30,
                        width: 140,
                        height: 60,
                    },
                    false,
                    Some(pos2(300.0, 240.0)),
                )),
            ),
            anno(
                2,
                AnnotationKind::TextBox(mk(
                    PhysicalRect {
                        x: 120,
                        y: 120,
                        width: 120,
                        height: 80,
                    },
                    true,
                    Some(pos2(60.0, 20.0)),
                )),
            ),
        ]);
    }

    /// Step badges in every size and style.
    #[test]
    fn parity_steps() {
        let mk = |x, y, size, style, count| {
            AnnotationKind::Step(StepObj {
                pos: pos2(x, y),
                count,
                mode: StepMode::Numbers,
                style,
                size,
                bg: Color32::from_rgb(55, 142, 240),
                fg: Color32::WHITE,
                ring: 4.0,
                ring_color: Color32::WHITE,
                opacity: 1.0,
                shadow: 5.0,
                lower: false,
            })
        };
        assert_render_parity(vec![
            anno(1, mk(50.0, 50.0, StepSize::Small, StepStyle::Circle, 1)),
            anno(2, mk(170.0, 130.0, StepSize::Medium, StepStyle::Square, 12)),
            anno(3, mk(300.0, 220.0, StepSize::Large, StepStyle::Circle, 7)),
        ]);
    }

    /// An inserted bitmap with shadow and partial transparency.
    #[test]
    fn parity_picture() {
        let mut pic = RgbaImage::new(40, 30);
        for (x, y, p) in pic.enumerate_pixels_mut() {
            *p = if (x + y) % 2 == 0 {
                Rgba([255, 0, 0, 255])
            } else {
                Rgba([0, 0, 255, 180])
            };
        }
        assert_render_parity(vec![anno(
            1,
            AnnotationKind::Picture(PicObj {
                rect: PhysicalRect {
                    x: 60,
                    y: 40,
                    width: 110,
                    height: 70,
                },
                image: pic,
                opacity: 0.9,
                shadow: 6.0,
            }),
        )]);
    }

    /// Emoji stickers: downscale, 2× upscale past the 128 px source,
    /// edge clip — the same bounded/ref contract as every layered
    /// object (emojis are icons: no opacity/shadow to vary).
    #[test]
    fn parity_emoji() {
        let mk = |x, y, side, id| {
            AnnotationKind::Emoji(EmojiObj {
                rect: PhysicalRect {
                    x,
                    y,
                    width: side,
                    height: side,
                },
                id,
            })
        };
        assert_render_parity(vec![
            // 👉 downscaled to 48 px.
            anno(1, mk(20, 20, 48, 0x1F449)),
            // 🔥 upscaled past the 128 px source (2× = max side).
            anno(2, mk(140, 60, 256, 0x1F525)),
            // ⚠️ clipped by the bottom-right edge.
            anno(3, mk(300, 220, 64, 0x26A0)),
        ]);
    }

    /// An emoji id absent from the embedded set renders the neutral
    /// placeholder — deterministic pixels, no error, no panic — and
    /// the bounded/ref paths still agree bit for bit.
    #[test]
    fn emoji_unknown_id_renders_placeholder() {
        let doc = doc_with(vec![anno(
            1,
            AnnotationKind::Emoji(EmojiObj {
                rect: PhysicalRect {
                    x: 40,
                    y: 40,
                    width: 64,
                    height: 64,
                },
                id: 0x10FFFE,
            }),
        )]);
        let bounded = render_to_image(&doc);
        let reference = render_to_image_ref(&doc);
        assert_same_pixels(&bounded, &reference);
        // The placeholder actually painted inside the rect (the base
        // pattern has no pure mid-grey run like the marker's).
        let painted = (40..104).any(|y| {
            (40..104).any(|x| {
                let p = bounded.get_pixel(x, y).0;
                p[0] == 150 && p[1] == 150 && p[2] == 150 && p[3] == 255
            })
        });
        assert!(painted, "placeholder drew no mid-grey pixels");
    }

    /// Emoji on a fully transparent base (e.g. a clipboard-imported
    /// RGBA document): parity holds and the sticker's own pixels land
    /// uncomposited-by-mistake — alpha survives in the output.
    #[test]
    fn emoji_on_transparent_base() {
        let mut doc = doc_with(vec![anno(
            1,
            AnnotationKind::Emoji(EmojiObj {
                rect: PhysicalRect {
                    x: 10,
                    y: 10,
                    width: 96,
                    height: 96,
                },
                id: 0x2764, // ❤️
            }),
        )]);
        doc.base.image = std::sync::Arc::new(RgbaImage::from_pixel(340, 260, Rgba([0, 0, 0, 0])));
        let bounded = render_to_image(&doc);
        assert_same_pixels(&bounded, &render_to_image_ref(&doc));
        // Some opaque emoji pixels exist and transparent corners stay.
        assert!(
            (10..106).any(|y| (10..106).any(|x| bounded.get_pixel(x, y).0[3] == 255)),
            "no opaque emoji pixels on transparent base"
        );
        assert_eq!(bounded.get_pixel(0, 0).0[3], 0);
    }

    /// Braces between anchors, several shapes.
    #[test]
    fn parity_braces() {
        let mk = |kind| {
            AnnotationKind::Brace(BraceObj {
                a: pos2(60.0, 30.0),
                b: pos2(60.0, 230.0),
                kind,
                color: Color32::WHITE,
                width: 4.0,
                dash: Dash::Solid,
                outline: 1.0,
                outline_color: Color32::BLACK,
                opacity: 1.0,
                shadow: 2.0,
            })
        };
        assert_render_parity(vec![
            anno(1, mk(BraceKind::CurlyLeft)),
            anno(2, mk(BraceKind::SquareRight)),
        ]);
    }

    /// Region effects bypass the layer path entirely — regression net
    /// only: pixelate, gaussian, clipped and fully outside.
    #[test]
    fn parity_blur() {
        let mk = |x, y, w, h, style| {
            AnnotationKind::Blur(BlurObj {
                rect: PhysicalRect {
                    x,
                    y,
                    width: w,
                    height: h,
                },
                style,
                shape: AreaShape::Ellipse,
                strength: 60,
            })
        };
        assert_render_parity(vec![
            anno(1, mk(50, 50, 120, 90, BlurStyle::Pixelate)),
            anno(2, mk(200, 100, 100, 80, BlurStyle::Blur)),
            anno(3, mk(-30, 200, 80, 80, BlurStyle::Blur)), // clipped
            anno(4, mk(-500, -500, 40, 40, BlurStyle::Pixelate)), // outside
        ]);
    }

    /// Eraser effects are the other direct-to-base path.
    #[test]
    fn parity_eraser_modes() {
        let points: Vec<Pos2> = (0..20)
            .map(|i| pos2(40.0 + i as f32 * 12.0, 130.0))
            .collect();
        let mk = |mode| {
            AnnotationKind::Freehand(FreeObj {
                points: points.clone(),
                color: Color32::WHITE,
                width: 20.0,
                tip: TipShape::Round,
                dash: Dash::Solid,
                outline: 0.0,
                outline_color: Color32::WHITE,
                shadow: 0.0,
                eraser_mode: mode,
                soft: false,
                opacity: 1.0,
            })
        };
        assert_render_parity(vec![
            anno(1, mk(EraserMode::Pixelate)),
            anno(2, mk(EraserMode::Blur)),
        ]);
    }

    /// Stacked semi-transparent objects — z-order and alpha
    /// accumulation must survive the split into per-object layers.
    #[test]
    fn parity_overlapping() {
        assert_render_parity(vec![
            anno(
                1,
                AnnotationKind::HighlightArea(AreaObj {
                    rect: PhysicalRect {
                        x: 60,
                        y: 60,
                        width: 220,
                        height: 140,
                    },
                    color: Color32::from_rgb(255, 235, 60),
                    shape: AreaShape::Round,
                    border: None,
                    dash: Dash::Solid,
                    soft: false,
                    opacity: 0.4,
                }),
            ),
            anno(
                2,
                AnnotationKind::Rect(RectObj {
                    rect: PhysicalRect {
                        x: 90,
                        y: 90,
                        width: 160,
                        height: 90,
                    },
                    stroke: Color32::from_rgb(230, 70, 60),
                    stroke_w: 14.0,
                    dash: Dash::Solid,
                    fill: Some(Color32::from_rgba_premultiplied(60, 120, 220, 120)),
                    radius: 6.0,
                    outline: 4.0,
                    outline_color: Color32::WHITE,
                    opacity: 0.8,
                    shadow: 8.0,
                }),
            ),
            anno(3, mk_step(200.0, 130.0, 5)),
        ]);
    }

    fn mk_step(x: f32, y: f32, count: u32) -> AnnotationKind {
        AnnotationKind::Step(StepObj {
            pos: pos2(x, y),
            count,
            mode: StepMode::Numbers,
            style: StepStyle::Circle,
            size: StepSize::Large,
            bg: Color32::from_rgb(230, 70, 60),
            fg: Color32::WHITE,
            ring: 3.0,
            ring_color: Color32::WHITE,
            opacity: 0.9,
            shadow: 4.0,
            lower: false,
        })
    }

    /// Objects fully or partially outside the image: the bounded layer
    /// must clip or skip without losing a pixel of the in-bounds part.
    #[test]
    fn parity_clipped_and_outside() {
        let mut r = RectObj {
            rect: PhysicalRect {
                x: -40,
                y: -20,
                width: 100,
                height: 60,
            },
            stroke: Color32::RED,
            stroke_w: 10.0,
            dash: Dash::Solid,
            fill: Some(Color32::BLUE),
            radius: 0.0,
            outline: 0.0,
            outline_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 12.0,
        };
        let clipped = anno(1, AnnotationKind::Rect(r.clone()));
        r.rect = PhysicalRect {
            x: 500,
            y: 400,
            width: 60,
            height: 60,
        };
        let outside = anno(2, AnnotationKind::Rect(r));
        let line_through = anno(
            3,
            AnnotationKind::Line(LineObj {
                a: pos2(-30.0, 130.0),
                b: pos2(200.0, 130.0),
                ka: pos2(50.0, 130.0),
                kb: pos2(150.0, 130.0),
                color: Color32::GREEN,
                width: 22.0,
                dash: Dash::Solid,
                start: LineEnd::Dot,
                end: LineEnd::Arrow,
                outline: 2.0,
                outline_color: Color32::WHITE,
                opacity: 1.0,
                shadow: 6.0,
            }),
        );
        assert_render_parity(vec![clipped, outside, line_through]);
    }

    /// An empty document renders its base verbatim.
    #[test]
    fn parity_empty_document() {
        assert_render_parity(Vec::new());
    }
}
