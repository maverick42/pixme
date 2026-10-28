//! Dev-only session seeder for performance work: `pixme
//! --seed-session [N]` writes N large synthetic captures (default 20,
//! 5120×2880 physical px — a Retina-class frame) into the real
//! `session.db`, every other document carrying a few annotations, so
//! first-paint costs (session decode, mosaic thumbnails) are
//! measurable on a realistic restored session.
//!
//! Seeding REPLACES the current session — the existing database files
//! are copied aside to `<name>.preseed` first. `main` refuses to run
//! this while another PixMe instance owns the store.

use std::path::{Path, PathBuf};

use egui::{Color32, pos2};
use image::RgbaImage;

use crate::capture::{Capture, MonitorInfo, PhysicalRect};
use crate::editor::Document;
use crate::editor::document::{
    Annotation, AnnotationKind, AreaObj, AreaShape, BlurObj, BlurStyle, Dash, EllipseObj, LineEnd,
    LineObj, RectObj, StepMode, StepObj, StepSize, StepStyle,
};
use crate::ui::viewer::Viewer;

/// Seeded capture size — 5K Retina-class pixels.
const W: u32 = 5120;
const H: u32 = 2880;

/// Capture count when the flag carries no value.
const DEFAULT_COUNT: usize = 20;

/// Parses `--seed-session` (bare, `=N`, or followed by `N`).
pub fn requested() -> Option<usize> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--seed-session" {
            return Some(
                args.next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_COUNT),
            );
        }
        if let Some(v) = arg.strip_prefix("--seed-session=") {
            return Some(v.parse().unwrap_or(DEFAULT_COUNT));
        }
    }
    None
}

/// Generates the captures, pushes them into a `Viewer` and writes the
/// whole session through the real save path (PNG encode included) —
/// the next launch restores them like a quit session. Returns the
/// process exit code.
pub fn run(count: usize) -> i32 {
    let path = crate::store::db_path();
    if let Err(err) = backup_existing(&path) {
        tracing::error!("cannot park the existing session db: {err}");
        return 1;
    }
    let t0 = std::time::Instant::now();
    let mut viewer = Viewer::default();
    for i in 0..count {
        let mut doc = Document::new(Capture {
            image: synth_image(i as u64),
            scale_factor: 2.0,
            monitor: MonitorInfo {
                id: 1,
                name: "Seeded".to_owned(),
                rect: PhysicalRect {
                    x: 0,
                    y: 0,
                    width: W / 2,
                    height: H / 2,
                },
                scale_factor: 2.0,
                is_primary: true,
            },
        });
        // Staggered capture times so date sorts see real data.
        doc.meta.captured_at = chrono::Local::now() - chrono::Duration::minutes(i as i64);
        if i % 2 == 0 {
            seed_annotations(&mut doc, i as u64);
        }
        viewer.push_named_document(doc, format!("Seed capture {:02}", i + 1), false);
    }
    let gen_ms = t0.elapsed().as_millis();
    let mut store = crate::store::Store::open();
    let t1 = std::time::Instant::now();
    store.save_session(&viewer.tabs, 0, viewer.next_title());
    tracing::info!(
        count,
        gen_ms,
        save_ms = t1.elapsed().as_millis(),
        path = %path.display(),
        "session seeded"
    );
    0
}

/// Copies the live database files (`session.db` plus a stray `-wal` /
/// `-shm`) to `<name>.preseed` — cheap insurance, not versioning.
fn backup_existing(path: &Path) -> std::io::Result<()> {
    for file in [
        path.to_owned(),
        suffixed(path, "-wal"),
        suffixed(path, "-shm"),
    ] {
        if file.exists() {
            let to = suffixed(&file, ".preseed");
            std::fs::copy(&file, &to)?;
            tracing::info!("existing {} parked as {}", file.display(), to.display());
        }
    }
    Ok(())
}

/// `<path><suffix>` — appends to the file name, like the `.bak` rule.
fn suffixed(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

/// xorshift64* — a deterministic, dependency-free PRNG so seeded
/// content is reproducible run to run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Uniform value in `0..n` (`n` ≥ 1).
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % n as u64) as u32
    }
}

/// A 5120×2880 frame with screenshot-ish content: a vertical
/// gradient, translucent "window" blocks, thin "text" strips and a
/// few high-entropy "photo" patches so the PNG keeps a realistic
/// size. All output is deterministic per `seed`.
fn synth_image(seed: u64) -> RgbaImage {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1));
    let mut img = RgbaImage::new(W, H);
    let buf = img.as_mut();
    let stride = W as usize;

    // Vertical gradient — top shade → bottom shade, hue varies by seed.
    let hue = (seed % 24) as f32 * 15.0;
    let (top, bot) = (hsv(hue, 0.5, 0.16), hsv(hue + 30.0, 0.35, 0.42));
    for y in 0..H as usize {
        let t = y as f32 / H as f32;
        let c = [
            lerp(top[0], bot[0], t),
            lerp(top[1], bot[1], t),
            lerp(top[2], bot[2], t),
        ];
        for px in buf[y * stride * 4..(y + 1) * stride * 4]
            .as_chunks_mut::<4>()
            .0
        {
            px[0] = c[0];
            px[1] = c[1];
            px[2] = c[2];
            px[3] = 255;
        }
    }

    // "Windows": translucent light/dark blocks scattered on top.
    for _ in 0..40 {
        let (w, h) = (300 + rng.below(1600), 200 + rng.below(900));
        let (x, y) = (rng.below(W - w), rng.below(H - h));
        let bright = rng.below(3) == 0;
        let v = if bright {
            210 + rng.below(45)
        } else {
            rng.below(70)
        };
        let a = 120 + rng.below(135);
        fill_blend(
            buf,
            stride,
            x,
            y,
            w,
            h,
            [v as u8, v as u8, v as u8, a as u8],
        );
    }

    // "Text": runs of thin dark segments on light lines.
    for _ in 0..14 {
        let x = rng.below(W - 2200) as usize;
        let y = rng.below(H - 120) as usize;
        for line in 0..6usize {
            let mut cx = x;
            let ly = y + line * 16;
            while cx < x + 1800 + rng.below(400) as usize {
                let seg = 30 + rng.below(160) as usize;
                fill_blend(
                    buf,
                    stride,
                    cx as u32,
                    ly as u32,
                    seg as u32,
                    3,
                    [20, 20, 24, 220],
                );
                cx += seg + 14 + rng.below(30) as usize;
            }
        }
    }

    // "Photos": opaque noise patches — high entropy so the stored PNG
    // reaches a realistic multi-MB size and decode cost.
    for _ in 0..3 {
        let (w, h) = (600 + rng.below(500), 400 + rng.below(350));
        let (x, y) = (rng.below(W - w) as usize, rng.below(H - h) as usize);
        for row in 0..h as usize {
            let line =
                &mut buf[((y + row) * stride + x) * 4..((y + row) * stride + x + w as usize) * 4];
            for px in line.as_chunks_mut::<4>().0 {
                let v = rng.next();
                px[0] = v as u8;
                px[1] = (v >> 8) as u8;
                px[2] = (v >> 16) as u8;
                px[3] = 255;
            }
        }
    }
    img
}

/// Alpha-blends a solid rectangle into the raw RGBA buffer (clamped).
fn fill_blend(buf: &mut [u8], stride: usize, x: u32, y: u32, w: u32, h: u32, c: [u8; 4]) {
    let (x1, y1) = ((x + w).min(W), (y + h).min(H));
    let a = c[3] as u32;
    for row in y..y1 {
        for col in x..x1 {
            let i = (row as usize * stride + col as usize) * 4;
            for k in 0..3 {
                buf[i + k] = ((buf[i + k] as u32 * (255 - a) + c[k] as u32 * a) / 255) as u8;
            }
        }
    }
}

/// u8 linear interpolation.
fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t) as u8
}

/// Minimal HSV → RGB (h 0–360, s/v 0–1) for the gradient shades.
fn hsv(h: f32, s: f32, v: f32) -> [u8; 3] {
    let h = h.rem_euclid(360.0);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match h as u32 {
        0..=59 => (c, x, 0.0),
        60..=119 => (x, c, 0.0),
        120..=179 => (0.0, c, x),
        180..=239 => (0.0, x, c),
        240..=299 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    [
        ((r + m) * 255.0) as u8,
        ((g + m) * 255.0) as u8,
        ((b + m) * 255.0) as u8,
    ]
}

/// A plausible handful of annotations: an arrow, a stroked rect, an
/// ellipse, a highlight zone and a step badge — plus a blur region on
/// every third annotated document (the expensive rasterization case).
fn seed_annotations(doc: &mut Document, seed: u64) {
    let mut rng = Rng(seed ^ 0xC0FF_EE11);
    let push = |doc: &mut Document, kind| {
        let id = doc.alloc_id();
        doc.annotations.push(Annotation { id, kind });
    };
    let (rx, ry) = (rng.below(W - 1400), rng.below(H - 900));
    push(
        doc,
        AnnotationKind::Rect(RectObj {
            rect: PhysicalRect {
                x: rx as i32,
                y: ry as i32,
                width: 800 + rng.below(600),
                height: 400 + rng.below(400),
            },
            stroke: Color32::from_rgb(55, 142, 240),
            stroke_w: 4.0,
            dash: Dash::Solid,
            fill: Some(Color32::from_rgba_premultiplied(55, 142, 240, 40)),
            radius: 8.0,
            outline: 0.0,
            outline_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 3.0,
        }),
    );
    push(
        doc,
        AnnotationKind::Ellipse(EllipseObj {
            rect: PhysicalRect {
                x: rng.below(W - 900) as i32,
                y: rng.below(H - 700) as i32,
                width: 500 + rng.below(400),
                height: 300 + rng.below(300),
            },
            stroke: Color32::from_rgb(230, 70, 60),
            stroke_w: 5.0,
            dash: Dash::Solid,
            fill: None,
            outline: 0.0,
            outline_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 0.0,
        }),
    );
    let (ax, ay) = (rng.below(W - 1200) as f32, rng.below(H - 800) as f32);
    push(
        doc,
        AnnotationKind::Line(LineObj {
            a: pos2(ax, ay),
            b: pos2(ax + 600.0 + rng.below(500) as f32, ay + 300.0),
            ka: pos2(ax + 150.0, ay),
            kb: pos2(ax + 400.0, ay + 300.0),
            color: Color32::from_rgb(255, 170, 30),
            width: 6.0,
            dash: Dash::Solid,
            start: LineEnd::None,
            end: LineEnd::Arrow,
            outline: 0.0,
            outline_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 0.0,
        }),
    );
    push(
        doc,
        AnnotationKind::HighlightArea(AreaObj {
            rect: PhysicalRect {
                x: rng.below(W - 1500) as i32,
                y: rng.below(H - 400) as i32,
                width: 1200 + rng.below(300),
                height: 120 + rng.below(200),
            },
            color: Color32::from_rgb(255, 235, 60),
            shape: AreaShape::Rect,
            border: None,
            dash: Dash::Solid,
            soft: false,
            opacity: 0.35,
        }),
    );
    push(
        doc,
        AnnotationKind::Step(StepObj {
            pos: pos2(
                200.0 + rng.below(W - 400) as f32,
                200.0 + rng.below(H - 400) as f32,
            ),
            count: 1 + seed as u32,
            mode: StepMode::Numbers,
            style: StepStyle::Circle,
            size: StepSize::Large,
            bg: Color32::from_rgb(55, 142, 240),
            fg: Color32::WHITE,
            ring: 3.0,
            ring_color: Color32::WHITE,
            opacity: 1.0,
            shadow: 2.0,
            lower: false,
        }),
    );
    if seed.is_multiple_of(3) {
        push(
            doc,
            AnnotationKind::Blur(BlurObj {
                rect: PhysicalRect {
                    x: rng.below(W - 1000) as i32,
                    y: rng.below(H - 700) as i32,
                    width: 700 + rng.below(300),
                    height: 400 + rng.below(300),
                },
                style: if seed.is_multiple_of(6) {
                    BlurStyle::Blur
                } else {
                    BlurStyle::Pixelate
                },
                shape: AreaShape::Rect,
                strength: 60,
            }),
        );
    }
}
