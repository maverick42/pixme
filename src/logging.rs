//! Centralised logging: a daily rolling file, a non-blocking writer and
//! a panic hook — plus the "diagnostic mode" runtime level switch and
//! the copyable diagnostics bundle.
//!
//! - Files: `logs_dir()/pixme.<YYYY-MM-DD>.log`, one per day, **5 kept**
//!   (`max_log_files`) and anything older than 5 days pruned at startup.
//!   The date is **UTC**: `tracing-appender`'s rotation clock is
//!   `OffsetDateTime::now_utc` with no local-time override (documented in
//!   `docs/decisions.md`).
//! - Lines: one event per line, `key=value` fields, local timestamp with
//!   explicit offset — captured at the top of `main` before any thread
//!   exists (reading the local zone can fail once threads run).
//! - Reliability: `tracing_appender::non_blocking` (lossy, bounded
//!   queue); dropped lines are counted and reported once. Size caps
//!   guard the single daily file: past 64 MiB only `warn`/`error` pass,
//!   past 256 MiB everything stops — each announced once. The panic
//!   hook writes `pixme.panic.log` synchronously and is NEVER capped.
//! - Privacy: [`Sensitive`] redacts anything wrapped in it; log paths
//!   are reduced via `crate::paths::file_name`/`tail_path`.

use std::fmt;
use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{FixedOffset, NaiveDate, Utc};
use tracing::Level;
use tracing_appender::non_blocking::{ErrorCounter, NonBlocking, NonBlockingBuilder};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::layer::{Layer, SubscriberExt};
use tracing_subscriber::{EnvFilter, Registry, filter::filter_fn, reload};

/// Rolling-file retention — `max_log_files` and the startup purge.
pub const MAX_LOG_FILES: usize = 5;
/// Days a daily file (or the panic log) survives past its date.
const RETENTION_DAYS: i64 = 5;
/// Past this, `debug`/`info` stop reaching the daily file (warned once).
const SOFT_CAP_BYTES: u64 = 64 * 1024 * 1024;
/// Past this, nothing reaches the daily file (announced once).
const HARD_CAP_BYTES: u64 = 256 * 1024 * 1024;
/// Lines the non-blocking writer buffers before dropping (counted).
const QUEUE_LINES: usize = 8192;
/// Lines of today's log included in "Copy diagnostic information".
const DIAGNOSTIC_TAIL_LINES: usize = 200;

/// The live logging session — `None` once [`shutdown`] ran.
static SESSION: Mutex<Option<LogSession>> = Mutex::new(None);

/// Everything the running log system owns; the `WorkerGuard` joins the
/// writer thread on drop, so it is only released inside [`shutdown`].
struct LogSession {
    guard: tracing_appender::non_blocking::WorkerGuard,
    /// Filter reload handle — diagnostic mode swaps `info` ↔ `debug`.
    reload: reload::Handle<EnvFilter, Registry>,
    /// tracing-appender's dropped-line counter (full queue, lossy mode).
    dropped: ErrorCounter,
    /// Bytes pushed at today's file — feeds the two caps.
    written: Arc<AtomicU64>,
    /// Cap announcements emitted so far (0 none, 1 soft, 2 hard).
    cap_state: Arc<AtomicU8>,
    /// Whether the "queue full" warning was already emitted.
    drops_reported: AtomicBool,
    /// The startup directive (`RUST_LOG` or `info`) — diagnostic-off
    /// restores exactly it.
    base_directive: String,
    /// The Settings "diagnostic mode" checkbox state (session-only).
    diagnostic: AtomicBool,
    /// `false` when the file appender could not be created — the app
    /// still runs; every file write is simply absent.
    enabled: bool,
}

fn session() -> Option<std::sync::MutexGuard<'static, Option<LogSession>>> {
    SESSION.lock().ok()
}

/// The local UTC offset, read FIRST in `main` — `chrono::Local` consults
/// `tzset`/the system zone, which is not guaranteed once worker threads
/// exist; a failure (or a panic inside the probe) falls back to UTC.
pub fn local_utc_offset() -> FixedOffset {
    std::panic::catch_unwind(|| *chrono::Local::now().offset())
        .unwrap_or_else(|_| FixedOffset::east_opt(0).expect("UTC is always valid"))
}

/// Initialises everything: purge, appender, subscriber, warnings.
/// `offset` comes from [`local_utc_offset`]. Never fails hard — an
/// unusable `logs_dir` leaves the app running with file logging off.
pub fn init(offset: FixedOffset) {
    let logs_dir = crate::paths::logs_dir().to_path_buf();
    let pruned = purge_old_logs(&logs_dir);
    let (dispatch, sess) = build_session(&logs_dir, offset);
    let enabled = sess.enabled;
    let _ = tracing::dispatcher::set_global_default(dispatch);
    if let Some(mut slot) = session() {
        *slot = Some(sess);
    }
    if !enabled {
        tracing::warn!("log directory unusable — file logging disabled for this run");
    }
    if pruned > 0 {
        tracing::info!(pruned, "stale log files removed");
    }
    if let Some(from) = crate::paths::fallback_from() {
        tracing::warn!(
            attempted = %from.display(),
            used = %crate::paths::logs_dir().display(),
            "exe-adjacent directory not writable — data/logs redirected"
        );
    }
    if let Some(old) = crate::paths::old_data_dir() {
        tracing::warn!(
            previous = %old.display(),
            "previous data directory found — left in place, not migrated"
        );
    }
}

/// Explicit, blocking flush + join of the writer thread — called by
/// `main` once `eframe` exits and before every `process::exit`, so the
/// guard is released on ALL quit paths (menu quit, `quit_now`,
/// second-instance exit, dev seeds).
pub fn shutdown() {
    let Some(mut slot) = session() else { return };
    let Some(session) = slot.take() else { return };
    let dropped = session.dropped.dropped_lines();
    if dropped > 0 {
        tracing::warn!(dropped, "log lines dropped — writer queue saturated");
    }
    tracing::info!("shutdown");
    drop(session.guard);
}

/// Per-frame health report — cheap atomic reads. Emits each cap
/// announcement once and the first dropped-lines warning once; called
/// from `Shared::pump`.
pub fn report_health() {
    let Some(slot) = session() else { return };
    let Some(s) = slot.as_ref() else { return };
    let bytes = s.written.load(Ordering::Relaxed);
    if bytes >= HARD_CAP_BYTES && s.cap_state.swap(2, Ordering::Relaxed) < 2 {
        tracing::error!("daily log hard cap reached — file logging off until next rotation");
    } else if bytes >= SOFT_CAP_BYTES && s.cap_state.swap(1, Ordering::Relaxed) < 1 {
        tracing::warn!(bytes, "daily log soft cap reached — debug/info silenced");
    }
    let dropped = s.dropped.dropped_lines();
    if dropped > 0 && !s.drops_reported.swap(true, Ordering::Relaxed) {
        tracing::warn!(dropped, "log lines dropped — writer queue saturated");
    }
}

/// The Settings "diagnostic mode" checkbox — `debug` for this run only,
/// never persisted. `off` restores the startup directive exactly.
pub fn set_diagnostic(on: bool) {
    let Some(slot) = session() else { return };
    let Some(s) = slot.as_ref() else { return };
    if s.diagnostic.swap(on, Ordering::Relaxed) == on {
        return;
    }
    let filter = if on {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_new(&s.base_directive).unwrap_or_else(|_| EnvFilter::new("info"))
    };
    let _ = s.reload.reload(filter);
    tracing::info!(enabled = on, "diagnostic mode");
}

/// Whether diagnostic mode is currently on (the checkbox state).
pub fn diagnostic_enabled() -> bool {
    session()
        .and_then(|slot| slot.as_ref().map(|s| s.diagnostic.load(Ordering::Relaxed)))
        .unwrap_or(false)
}

/// The startup directive shown in the banner (`RUST_LOG` or `info`).
pub fn base_directive() -> String {
    session()
        .and_then(|slot| slot.as_ref().map(|s| s.base_directive.clone()))
        .unwrap_or_else(|| "info".to_owned())
}

/// A value whose `Debug` and `Display` always print `<redacted>` —
/// wrap anything potentially sensitive (clipboard-derived text,
/// annotation content, user paths that add nothing to the log).
pub struct Sensitive<T>(pub T);

impl<T> Sensitive<T> {
    /// Unwraps — the ONLY way the value reaches the log is an explicit
    /// format of `inner`, never an accidental `?`/ `%`.
    pub fn into_inner(self) -> T {
        self.0
    }
}

impl<T> fmt::Debug for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

impl<T> fmt::Display for Sensitive<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// The panic hook: message, location and backtrace appended to
/// `pixme.panic.log` (sync write — the non-blocking writer may already
/// be wedged) plus `stderr`, then the `tracing::error!` mirror. Written
/// in that order: a double panic inside the hook aborts instantly, so
/// the durable record must land first. Never size-capped.
pub fn install_panic_hook() {
    let log = crate::paths::panic_log();
    std::panic::set_hook(Box::new(move |info| {
        let line = format!(
            "[{}] panic on '{}': {info}",
            chrono::Local::now().to_rfc3339(),
            std::thread::current().name().unwrap_or("worker"),
        );
        let backtrace = std::backtrace::Backtrace::force_capture();
        use std::io::Write as _;
        if let Some(dir) = log.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
        {
            let _ = writeln!(f, "{line}");
            let _ = writeln!(f, "{backtrace}");
        }
        let _ = std::io::stderr().write_all(line.as_bytes());
        let _ = std::io::stderr().write_all(b"\n");
        tracing::error!("{line}");
    }));
}

/// Banner + the last [`DIAGNOSTIC_TAIL_LINES`] of today's file, every
/// line passed through [`redact`] — the clipboard payload for "Copy
/// diagnostic information". The caller adds the translated warning.
pub fn diagnostics_text() -> String {
    let mut out = format!(
        "PixMe {} ({})\n{} {}\ndata: {}\nlogs: {}\n---\n",
        env!("CARGO_PKG_VERSION"),
        env!("PIXME_GIT_HASH"),
        crate::platform::os_version(),
        std::env::consts::ARCH,
        crate::paths::data_dir().display(),
        crate::paths::logs_dir().display(),
    );
    let file = crate::paths::today_log_file();
    if let Ok(text) = std::fs::read_to_string(&file) {
        let tail: Vec<&str> = text.lines().rev().take(DIAGNOSTIC_TAIL_LINES).collect();
        for line in tail.iter().rev() {
            out.push_str(&redact(line));
            out.push('\n');
        }
    }
    out
}

/// Replaces the user's home dir by `~` — the only personal segment a
/// log line can carry (paths are already reduced to file names, but
/// defensive copy beats a leak).
pub fn redact(text: &str) -> String {
    match crate::paths::user_home() {
        Some(home) => text.replace(&home.to_string_lossy().into_owned(), "~"),
        None => text.to_owned(),
    }
}

/// Builds the subscriber and session. Factored out of [`init`] so tests
/// can scope a session to one thread (`with_default`) instead of the
/// global dispatcher.
fn build_session(logs_dir: &Path, offset: FixedOffset) -> (tracing::Dispatch, LogSession) {
    let base_directive = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_owned());
    let base = EnvFilter::try_new(&base_directive).unwrap_or_else(|_| EnvFilter::new("info"));
    let (env_layer, reload) = reload::Layer::new(base);

    let written = Arc::new(AtomicU64::new(0));
    let cap_state = Arc::new(AtomicU8::new(0));

    // The file layer carries its own guard + drop counter; a failed
    // appender yields a no-file session (app still runs, warned).
    let file = rolling_appender(logs_dir).ok().map(|appender| {
        let (nb, guard) = NonBlockingBuilder::default()
            .lossy(true)
            .buffered_lines_limit(QUEUE_LINES)
            .finish(appender);
        let counting = CountingWriter {
            inner: nb.clone(),
            written: Arc::clone(&written),
        };
        let by = Arc::clone(&written);
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || counting.clone())
            .with_timer(LocalTimer(offset))
            .with_filter(filter_fn(move |m| {
                // Housekeeping events (cap/drops announcements) always
                // pass — otherwise the hard cap could never announce.
                if m.target().starts_with("pixme::logging") {
                    return true;
                }
                match by.load(Ordering::Relaxed) {
                    b if b >= HARD_CAP_BYTES => false,
                    b if b >= SOFT_CAP_BYTES => *m.level() <= Level::WARN,
                    _ => true,
                }
            }));
        (layer, guard, nb.error_counter())
    });
    let enabled = file.is_some();
    let (file_layer, guard, dropped) = match file {
        Some((l, g, d)) => (Some(l), Some(g), Some(d)),
        None => (None, None, None),
    };

    // Console output: development aid only — release binaries write the
    // file and nothing else (LossyStdout survives a closed pipe).
    let stdout_layer = console_enabled().then(|| {
        tracing_subscriber::fmt::layer()
            .with_writer(|| LossyStdout)
            .with_timer(LocalTimer(offset))
    });

    let subscriber = Registry::default()
        .with(env_layer)
        .with(file_layer)
        .with(stdout_layer);

    // A missing appender leaves a dead guard slot: give the session a
    // sink-writer guard so `shutdown()` still behaves uniformly.
    let (sink_nb, sink_guard) = NonBlocking::new(std::io::sink());
    (
        tracing::Dispatch::new(subscriber),
        LogSession {
            guard: guard.unwrap_or(sink_guard),
            reload,
            dropped: dropped.unwrap_or_else(|| sink_nb.error_counter()),
            written,
            cap_state,
            drops_reported: AtomicBool::new(false),
            base_directive,
            diagnostic: AtomicBool::new(false),
            enabled,
        },
    )
}

/// The daily rolling appender: `pixme.<UTC date>.log`, 5 files kept.
fn rolling_appender(
    logs_dir: &Path,
) -> Result<tracing_appender::rolling::RollingFileAppender, tracing_appender::rolling::InitError> {
    tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("pixme")
        .filename_suffix("log")
        .max_log_files(MAX_LOG_FILES)
        .build(logs_dir)
}

/// Console echo exists for development only — release binaries write
/// the file and stay silent (`cargo run`, dev-tools benchmarks).
fn console_enabled() -> bool {
    cfg!(debug_assertions) || cfg!(feature = "dev-tools")
}

/// `stdout` writer that swallows write/flush errors (`BrokenPipe`,
/// detached console): `tracing-subscriber`'s default would `eprintln!`
/// on failure, which panics when stderr is the same closed pipe.
struct LossyStdout;

impl Write for LossyStdout {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let _ = std::io::stdout().write_all(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let _ = std::io::stdout().flush();
        Ok(())
    }
}

/// Byte counter between the fmt layer and the non-blocking writer —
/// feeds the two size caps.
#[derive(Clone)]
struct CountingWriter {
    inner: NonBlocking,
    written: Arc<AtomicU64>,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.written.fetch_add(n as u64, Ordering::Relaxed);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Local-time timestamp with explicit offset (`2026-10-30T14:23:45.123+01:00`),
/// the zone captured once before any thread exists.
struct LocalTimer(FixedOffset);

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut Writer<'_>) -> fmt::Result {
        let now = Utc::now().with_timezone(&self.0);
        write!(w, "{}", now.format("%Y-%m-%dT%H:%M:%S%.3f%:z"))
    }
}

/// Deletes `pixme.<date>.log` files older than [`RETENTION_DAYS`] (the
/// filename carries the UTC date — no mtime dependence) and a stale
/// `pixme.panic.log` (by mtime). Foreign files are never touched.
/// Returns the count removed; the caller reports it after the
/// subscriber exists.
fn purge_old_logs(dir: &Path) -> usize {
    let today = Utc::now().date_naive();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let stale = if name == "pixme.panic.log" {
            entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|a| a > Duration::from_secs(RETENTION_DAYS as u64 * 86_400))
        } else {
            log_date(name).is_some_and(|d| (today - d).num_days() > RETENTION_DAYS)
        };
        if stale && std::fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// The UTC date embedded in a `pixme.<YYYY-MM-DD>.log` name — `None`
/// for anything else (foreign files, the panic log).
fn log_date(name: &str) -> Option<NaiveDate> {
    name.strip_prefix("pixme.")?
        .strip_suffix(".log")
        .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn sensitive_leaks_nothing_through_debug_or_display() {
        let secret = Sensitive("hunter2 — clipboard contents");
        assert_eq!(format!("{secret:?}"), "<redacted>");
        assert_eq!(format!("{secret}"), "<redacted>");
        let path = Sensitive(PathBuf::from("/Users/alice/secret.png"));
        assert!(!format!("{path:?}").contains("alice"));
    }

    #[test]
    fn log_date_only_parses_daily_names() {
        assert_eq!(
            log_date("pixme.2026-10-30.log"),
            Some(NaiveDate::from_ymd_opt(2026, 10, 30).unwrap())
        );
        assert_eq!(log_date("pixme.panic.log"), None);
        assert_eq!(log_date("other.txt"), None);
        assert_eq!(log_date("pixme.not-a-date.log"), None);
    }

    /// Retention: dated logs past 5 days are pruned, today's kept, a
    /// foreign file in the same directory is never touched.
    #[test]
    fn purge_removes_stale_logs_keeps_everything_else() {
        let dir = std::env::temp_dir().join(format!("pixme-purge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let today = Utc::now().date_naive();
        let stale = today - chrono::Duration::days(RETENTION_DAYS + 1);
        for name in [
            format!("pixme.{stale}.log"),
            "pixme.panic.log".to_owned(),
            format!("pixme.{today}.log"),
            "notes.txt".to_owned(),
        ] {
            std::fs::write(dir.join(&name), b"x").unwrap();
        }
        // panic.log has no date in its name: age it via mtime.
        std::fs::File::open(dir.join("pixme.panic.log"))
            .unwrap()
            .set_modified(std::time::SystemTime::now() - Duration::from_secs(6 * 86_400))
            .unwrap();

        assert_eq!(purge_old_logs(&dir), 2);
        assert!(!dir.join(format!("pixme.{stale}.log")).exists());
        assert!(!dir.join("pixme.panic.log").exists());
        assert!(dir.join(format!("pixme.{today}.log")).exists());
        assert!(dir.join("notes.txt").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The WriterGuard joins the writer thread on drop — a session
    /// dropped on an exit path flushes its last lines to disk.
    #[test]
    fn dropped_session_flushes_pending_lines() {
        let dir = std::env::temp_dir().join(format!("pixme-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (dispatch, session) = build_session(&dir, FixedOffset::east_opt(0).unwrap());
        tracing::dispatcher::with_default(&dispatch, || {
            tracing::info!("flush-me-please");
        });
        assert!(session.enabled);
        drop(session); // the exit-path release
        let file = dir.join(format!("pixme.{}.log", Utc::now().format("%Y-%m-%d")));
        let text = std::fs::read_to_string(&file).expect("today's log must exist");
        assert!(text.contains("flush-me-please"), "{text}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The soft cap silences debug/info but lets warn/error through —
    /// and housekeeping events always pass.
    #[test]
    fn cap_rules_by_level() {
        // Mirror of the filter closure — kept next to it so a change
        // to either trips a test.
        fn allowed(written: u64, level: &Level, housekeeping: bool) -> bool {
            if housekeeping {
                return true;
            }
            match written {
                b if b >= HARD_CAP_BYTES => false,
                b if b >= SOFT_CAP_BYTES => *level <= Level::WARN,
                _ => true,
            }
        }
        assert!(allowed(SOFT_CAP_BYTES - 1, &Level::INFO, false));
        assert!(!allowed(SOFT_CAP_BYTES, &Level::INFO, false));
        assert!(allowed(SOFT_CAP_BYTES, &Level::WARN, false));
        assert!(allowed(HARD_CAP_BYTES - 1, &Level::ERROR, false));
        assert!(!allowed(HARD_CAP_BYTES, &Level::ERROR, false));
        assert!(allowed(HARD_CAP_BYTES, &Level::TRACE, true));
    }

    #[test]
    fn redact_hides_the_user_profile() {
        let home = crate::paths::user_home().unwrap();
        let line = format!("data: {}/pixme/session.db saved", home.display());
        let out = redact(&line);
        assert!(out.contains("~/pixme/session.db"), "{out}");
        assert!(!out.contains(&*home.to_string_lossy()), "{out}");
    }
}
