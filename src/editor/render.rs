//! The single raster path for documents: every display and export
//! surface (draw canvas, viewer texture, save, copy, print) composites
//! base + annotations through [`render_to_image`], so WYSIWYG holds by
//! construction. Rasterization uses `imageproc` primitives plus small
//! local helpers (thick/dashed polylines, fills, text with outline and
//! faux italics, flood fill, pixelate/blur).

use std::collections::VecDeque;
use std::sync::OnceLock;

use egui::{Color32, Pos2};
use image::{Rgba, RgbaImage};
use imageproc::drawing as drw;

use super::document::{
    Annotation, AnnotationKind, AreaShape, BlurStyle, Dash, Document, LineEnd, StepMode, StepSize,
    StepStyle, TextAlign, TipShape,
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
    let mut img = doc.base.image.clone();
    for anno in &doc.annotations {
        render_one(&mut img, anno);
    }
    img
}

/// Paints `doc` — kept for API parity with the original module contract;
/// the canvas displays the [`render_to_image`] texture so both paths
/// share pixels by construction.
pub fn paint_document(_ui: &mut egui::Ui, _doc: &Document, _rect: egui::Rect) {}

/// Renders one annotation onto `img` (already a clone of the base).
fn render_one(img: &mut RgbaImage, anno: &Annotation) {
    match &anno.kind {
        // Region effects read + write the base directly.
        AnnotationKind::Blur(o) => apply_blur(img, o),
        AnnotationKind::Fill(o) => apply_fill(img, o),
        _ => {
            // Layered objects: rasterize to a transparent layer, then
            // composite shadow + body — one path for every drawable.
            let (layer, shadow, opacity) = object_layer(anno, img.width(), img.height());
            if layer.width() == 0 {
                return;
            }
            if shadow > 0.0 {
                let dark = tint_alpha(&layer, Rgba([0, 0, 0, 90]));
                composite(img, &dark, shadow as i64, shadow as i64, 1.0);
            }
            composite(img, &layer, 0, 0, opacity);
        }
    }
}

/// Shadow/opacity accessors shared by the layered objects.
fn shadow_opacity(anno: &Annotation) -> (f32, f32) {
    match &anno.kind {
        AnnotationKind::Rect(o) => (o.shadow, o.opacity),
        AnnotationKind::Ellipse(o) => (o.shadow, o.opacity),
        AnnotationKind::Line(o) => (0.0, o.opacity),
        AnnotationKind::Fancy(o) => (o.shadow, o.opacity),
        AnnotationKind::Freehand(_) | AnnotationKind::HighlightStroke(_) => (0.0, 1.0),
        AnnotationKind::HighlightArea(o) => (0.0, o.opacity),
        AnnotationKind::Text(o) => (o.shadow, o.opacity),
        AnnotationKind::TextBox(o) => (o.shadow, o.opacity),
        AnnotationKind::Step(o) => (0.0, o.opacity),
        AnnotationKind::Stamp(o) => (0.0, o.opacity),
        AnnotationKind::Picture(o) => (o.shadow, o.opacity),
        AnnotationKind::Fill(_) | AnnotationKind::Blur(_) => (0.0, 1.0),
    }
}

/// Rasterizes a layered object onto a fresh transparent layer.
fn object_layer(anno: &Annotation, w: u32, h: u32) -> (RgbaImage, f32, f32) {
    let (shadow, opacity) = shadow_opacity(anno);
    let mut layer = RgbaImage::new(w, h);
    match &anno.kind {
        AnnotationKind::Rect(o) => draw_shape(
            &mut layer,
            o.rect,
            AreaShape::Rect,
            o.radius,
            o.fill,
            o.stroke,
            o.stroke_w,
            o.dash,
            1.0,
        ),
        AnnotationKind::Ellipse(o) => draw_shape(
            &mut layer,
            o.rect,
            AreaShape::Ellipse,
            0.0,
            o.fill,
            o.stroke,
            o.stroke_w,
            o.dash,
            1.0,
        ),
        AnnotationKind::Line(o) => draw_line_obj(&mut layer, o),
        AnnotationKind::Fancy(o) => draw_fancy(&mut layer, o),
        AnnotationKind::Freehand(o) => {
            stroke_freehand(&mut layer, o);
        }
        AnnotationKind::HighlightStroke(o) => {
            stroke_freehand(&mut layer, o);
        }
        AnnotationKind::HighlightArea(o) => draw_shape(
            &mut layer,
            o.rect,
            o.shape,
            if o.shape == AreaShape::Round {
                12.0
            } else {
                0.0
            },
            Some(o.color),
            o.border.unwrap_or(Color32::TRANSPARENT),
            if o.border.is_some() { 1.5 } else { 0.0 },
            o.dash,
            if o.soft { 0.5 } else { 1.0 },
        ),
        AnnotationKind::Text(o) => draw_text_obj(&mut layer, o),
        AnnotationKind::TextBox(o) => draw_textbox(&mut layer, o),
        AnnotationKind::Step(o) => draw_step(&mut layer, o),
        AnnotationKind::Stamp(o) => draw_stamp(&mut layer, o),
        AnnotationKind::Picture(o) => draw_picture(&mut layer, o),
        AnnotationKind::Fill(_) | AnnotationKind::Blur(_) => {}
    }
    // Per-object alpha (freehand strokes carry theirs on the colour).
    let extra = match &anno.kind {
        AnnotationKind::Freehand(o) | AnnotationKind::HighlightStroke(o) => o.opacity,
        _ => 1.0,
    };
    (layer, shadow, opacity * extra)
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

/// The tapered fancy arrow: shaft quad + oversized head triangle.
fn draw_fancy(img: &mut RgbaImage, o: &super::document::FancyObj) {
    let (dx, dy) = (o.b.x - o.a.x, o.b.y - o.a.y);
    let len = dx.hypot(dy).max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (-uy, ux);
    let c = c32(o.color);
    let head_len = (o.width * 4.0 + 14.0).min(len * 0.6);
    let head_w = o.width * 1.8 + 7.0;
    let base = Pos2::new(o.b.x - ux * head_len, o.b.y - uy * head_len);
    // Shaft: quad from tail to head base.
    let hw = o.width / 2.0;
    drw::draw_polygon_mut(
        img,
        &points_i32(&[
            imageproc::point::Point::new(o.a.x + nx * hw, o.a.y + ny * hw),
            imageproc::point::Point::new(base.x + nx * hw, base.y + ny * hw),
            imageproc::point::Point::new(base.x - nx * hw, base.y - ny * hw),
            imageproc::point::Point::new(o.a.x - nx * hw, o.a.y - ny * hw),
        ]),
        c,
    );
    // Tail cap + head.
    drw::draw_filled_circle_mut(img, (o.a.x as i32, o.a.y as i32), hw as i32, c);
    drw::draw_polygon_mut(
        img,
        &points_i32(&[
            imageproc::point::Point::new(o.b.x, o.b.y),
            imageproc::point::Point::new(base.x + nx * head_w, base.y + ny * head_w),
            imageproc::point::Point::new(base.x - nx * head_w, base.y - ny * head_w),
        ]),
        c,
    );
}

/// Pencil/eraser/highlight freehand stroke.
fn stroke_freehand(img: &mut RgbaImage, o: &super::document::FreeObj) {
    if o.soft {
        // Soft edge: draw to a scratch layer, blur it, composite.
        let mut tmp = RgbaImage::new(img.width(), img.height());
        stroke_polyline(&mut tmp, &o.points, o.width, o.color, o.tip, Dash::Solid);
        let blurred = imageproc::filter::gaussian_blur_f32(&tmp, (o.width / 6.0).max(0.8));
        composite(img, &blurred, 0, 0, 1.0);
    } else {
        stroke_polyline(img, &o.points, o.width, o.color, o.tip, Dash::Solid);
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

/// A Lucide-glyph stamp, recoloured and rasterized once per draw.
fn draw_stamp(img: &mut RgbaImage, o: &super::document::StampObj) {
    let icon = crate::ui::icons::stamp_icon(o.stamp);
    let svg = String::from_utf8_lossy(crate::ui::icon_data::raw_svg(icon))
        .replace("currentColor", &hex(o.color));
    let size = o.size.max(4.0) as u32;
    let Ok(bmp) = egui_extras::image::load_svg_bytes_with_size(
        svg.as_bytes(),
        egui::load::SizeHint::Size {
            width: size,
            height: size,
            maintain_aspect_ratio: true,
        },
        &Default::default(),
    ) else {
        return;
    };
    let rgba = RgbaImage::from_raw(
        bmp.width() as u32,
        bmp.height() as u32,
        bmp.as_raw().to_vec(),
    )
    .unwrap_or_default();
    composite(img, &rgba, o.pos.x as i64, o.pos.y as i64, 1.0);
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

/// Fill object: composite `color` through the precomputed mask (or
/// erase to transparent when `transparent`).
fn apply_fill(img: &mut RgbaImage, o: &super::document::FillObj) {
    for y in 0..img.height() {
        for x in 0..img.width() {
            let cov = o.mask.get_pixel(x, y).0[3];
            if cov == 0 {
                continue;
            }
            let t = cov as f32 / 255.0;
            let dst = img.get_pixel_mut(x, y);
            if o.transparent {
                dst.0[3] = (dst.0[3] as f32 * (1.0 - t)) as u8;
            } else {
                blend_channel(dst, Rgba([o.color.r(), o.color.g(), o.color.b(), 255]), t);
            }
        }
    }
}

/// Source-over blends `src` (coverage `t`) into `dst`.
fn blend_channel(dst: &mut Rgba<u8>, src: Rgba<u8>, t: f32) {
    for ch in 0..3 {
        dst.0[ch] = (src.0[ch] as f32 * t + dst.0[ch] as f32 * (1.0 - t)) as u8;
    }
    dst.0[3] = dst.0[3].max((src.0[3] as f32 * t) as u8);
}

/// Computes the fill mask at placement time — BFS flood fill from
/// `origin` over pixels within `tolerance` of the seed colour.
pub fn compute_fill_mask(img: &RgbaImage, origin: Pos2, tolerance: u8) -> RgbaImage {
    let (w, h) = (img.width(), img.height());
    let mut mask = RgbaImage::new(w, h);
    let (ox, oy) = (origin.x as i64, origin.y as i64);
    if ox < 0 || oy < 0 || ox >= w as i64 || oy >= h as i64 {
        return mask;
    }
    let seed = *img.get_pixel(ox as u32, oy as u32);
    let tol = tolerance as i32 * 3;
    let within = |p: &Rgba<u8>| {
        ((p[0] as i32 - seed[0] as i32).abs()
            + (p[1] as i32 - seed[1] as i32).abs()
            + (p[2] as i32 - seed[2] as i32).abs()
            + (p[3] as i32 - seed[3] as i32).abs())
            <= tol
    };
    let mut seen = vec![false; (w * h) as usize];
    let mut q = VecDeque::from([(ox as u32, oy as u32)]);
    seen[(oy * w as i64 + ox) as usize] = true;
    while let Some((x, y)) = q.pop_front() {
        let p = img.get_pixel(x, y);
        if !within(p) {
            continue;
        }
        mask.put_pixel(x, y, Rgba([255, 255, 255, 255]));
        for (nx, ny) in [
            (x as i64 + 1, y as i64),
            (x as i64 - 1, y as i64),
            (x as i64, y as i64 + 1),
            (x as i64, y as i64 - 1),
        ] {
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let idx = (ny * w as i64 + nx) as usize;
            if !seen[idx] {
                seen[idx] = true;
                q.push_back((nx as u32, ny as u32));
            }
        }
    }
    mask
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

/// `#rrggbb` for SVG recolouring.
fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", c.r(), c.g(), c.b())
}
