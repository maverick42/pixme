//! Dev-only store benchmark: `pixme --bench-store [N]` measures the
//! write-through pipeline on N synthetic 5K captures (default 20):
//!
//! - PNG compression trade-off (`Fast` vs `Default`, adaptive vs no
//!   filter) — the blob format's size/time balance, in ms and KiB.
//! - The UI-side cost of handing a document to the writer (snapshot
//!   clone + channel send) — what a frame would pay, in µs.
//! - The writer pipeline end to end: per-document `queued_ms`
//!   (enqueue → committed = the crash-loss window) and `work_ms`
//!   (encode + SQLite), plus the flush wall time.
//! - The restore side: `load_session` on the written set.
//!
//! Writes go to a throwaway database under the OS temp dir — the real
//! `session.db` is never touched.

use std::sync::Arc;
use std::time::Instant;

use crate::capture::{Capture, MonitorInfo, PhysicalRect};
use crate::editor::Document;
use crate::store::writer::{WriteEvent, Writer};
use crate::ui::viewer::Viewer;

/// Capture count when the flag carries no value.
const DEFAULT_COUNT: usize = 20;

/// Parses `--bench-store` (bare, `=N`, or followed by `N`).
pub fn requested() -> Option<usize> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--bench-store" {
            return Some(
                args.next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(DEFAULT_COUNT),
            );
        }
        if let Some(v) = arg.strip_prefix("--bench-store=") {
            return Some(v.parse().unwrap_or(DEFAULT_COUNT));
        }
    }
    None
}

/// PNG-encodes `img` with explicit compression/filter — the timing arm
/// of the size/speed comparison.
fn encode_timed(
    img: &image::RgbaImage,
    compression: image::codecs::png::CompressionType,
    filter: image::codecs::png::FilterType,
) -> (std::time::Duration, usize) {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;
    let t = Instant::now();
    let mut buf = Vec::new();
    PngEncoder::new_with_quality(&mut buf, compression, filter)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .expect("png encode");
    (t.elapsed(), buf.len())
}

/// Wraps a synthetic frame in the `Document`/`Tab` shape the writer
/// consumes — same shape `devseed` produces.
fn synth_document(seed: u64) -> Document {
    let image = crate::devseed::synth_image(seed);
    let mut doc = Document::new(Capture {
        scale_factor: 2.0,
        monitor: MonitorInfo {
            id: 1,
            name: "Bench".to_owned(),
            rect: PhysicalRect {
                x: 0,
                y: 0,
                width: image.width() / 2,
                height: image.height() / 2,
            },
            scale_factor: 2.0,
            is_primary: true,
        },
        image: Arc::new(image),
    });
    doc.meta.captured_at = chrono::Local::now();
    doc
}

/// Runs the measurements and prints the report; returns the exit code.
pub fn run(count: usize) -> i32 {
    let dir = std::env::temp_dir().join(format!("pixme-bench-{}", std::process::id()));
    let db = dir.join("session.db");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("bench dir");

    // -- PNG compression trade-off -----------------------------------
    let probe = crate::devseed::synth_image(0);
    println!(
        "== PNG encode, {}x{} RGBA ({} MiB raw) ==",
        probe.width(),
        probe.height(),
        probe.as_raw().len() / (1024 * 1024)
    );
    use image::codecs::png::{CompressionType as C, FilterType as F};
    for (name, c, f) in [
        ("fast/adaptive (current)", C::Fast, F::Adaptive),
        ("fast/none              ", C::Fast, F::NoFilter),
        ("default/adaptive       ", C::Default, F::Adaptive),
        ("best/adaptive          ", C::Best, F::Adaptive),
    ] {
        let (dt, bytes) = encode_timed(&probe, c, f);
        println!(
            "  {name} : {:>5} ms  {:>7} KiB",
            dt.as_millis(),
            bytes / 1024
        );
    }

    // -- Session write-through ---------------------------------------
    println!("\n== writer pipeline, {count} x 5K docs ==");
    let t_gen = Instant::now();
    let mut viewer = Viewer::default();
    for i in 0..count as u64 {
        viewer.push_named_document(synth_document(i), format!("Bench {i:02}"), false);
    }
    println!(
        "  generated {} docs in {} ms",
        count,
        t_gen.elapsed().as_millis()
    );

    let writer = Writer::spawn(crate::store::Store::open_at(db.clone()), None);
    let mut enqueue_us = 0u128;
    let t_enqueue = Instant::now();
    for (ord, tab) in viewer.tabs.iter_mut().enumerate() {
        let t = Instant::now();
        let w = tab.to_doc_write(ord);
        writer.write_doc(w);
        enqueue_us += t.elapsed().as_micros();
        tab.persisted_rev = Some(tab.doc.revision);
        tab.base_dirty = false;
        tab.stored_fp = Some(tab.fingerprint());
    }
    let enqueue_all = t_enqueue.elapsed();
    println!(
        "  UI-side snapshot+send: {:>6} µs/doc ({} docs in {:?})",
        enqueue_us as u64 / count as u64,
        count,
        enqueue_all
    );

    let t_flush = Instant::now();
    writer.flush().recv().expect("flush reply");
    let flush_ms = t_flush.elapsed().as_millis();
    let events = writer.drain_events();
    let (mut q_min, mut q_max, mut q_sum, mut w_min, mut w_max, mut w_sum) =
        (u64::MAX, 0u64, 0u64, u64::MAX, 0u64, 0u64);
    let mut written = 0usize;
    for ev in events {
        match ev {
            WriteEvent::DocWritten {
                queued_ms, work_ms, ..
            } => {
                written += 1;
                q_min = q_min.min(queued_ms);
                q_max = q_max.max(queued_ms);
                q_sum += queued_ms;
                w_min = w_min.min(work_ms);
                w_max = w_max.max(work_ms);
                w_sum += work_ms;
            }
            WriteEvent::DocFailed { id } => println!("  FAILED doc {id}"),
        }
    }
    println!("  flush (queue drained): {flush_ms} ms, {written}/{count} docs committed");
    if written > 0 {
        println!(
            "  queued_ms (loss window): min {q_min}  mean {}  max {q_max}",
            q_sum / written as u64
        );
        println!(
            "  work_ms (encode+sql) : min {w_min}  mean {}  max {w_max}",
            w_sum / written as u64
        );
    }
    let db_kb = std::fs::metadata(&db).map(|m| m.len() / 1024).unwrap_or(0);
    println!("  session.db on disk: {db_kb} KiB");

    // -- Restore side --------------------------------------------------
    let mut store = crate::store::Store::open_at(db.clone());
    let t_load = Instant::now();
    let session = store.load_session();
    println!("\n== restore ==",);
    println!(
        "  load_session: {} ms for {} tabs",
        t_load.elapsed().as_millis(),
        session.tabs.len()
    );
    drop(store);
    drop(writer);
    let _ = std::fs::remove_dir_all(&dir);
    println!("\nbench db removed: {}", crate::paths::file_name(&db));
    0
}
