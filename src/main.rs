//! PixMe — a lightweight, standalone screenshot capture and annotation tool.
//!
//! Entry point: resolves the data/log paths, installs file logging, checks
//! the single-instance guard and hands off to [`app::run`]. All application
//! state lives in [`app::PixMeApp`].
//!
//! Startup order is load-bearing: the local UTC offset is read BEFORE any
//! thread exists, `paths::init` creates the directories logging needs, and
//! `logging::init` installs the subscriber — every later event lands in
//! `logs_dir()/pixme.<date>.log`.

// Phase 1 scaffold: public module APIs (capture, editor, export, widgets) are
// consumed from phase 2 onward. Remove this allow as real callers land.
#![allow(dead_code)]

#[macro_use]
extern crate rust_i18n;

mod app;
mod capture;
mod config;
#[cfg(feature = "dev-tools")]
mod devseed;
mod editor;
mod export;
mod i18n;
mod logging;
mod paths;
mod platform;
mod store;
mod ui;

i18n!("locales", fallback = "en");

fn main() -> eframe::Result<()> {
    // The local offset must be captured before ANY thread spawns —
    // reading the system zone late can fail or block.
    let utc_offset = logging::local_utc_offset();
    paths::init();
    logging::init(utc_offset);
    logging::install_panic_hook();

    // Dev tool (dev-tools feature only): `pixme --seed-session [N]`
    // writes N large synthetic captures into session.db and exits —
    // performance test data. Use PIXME_DATA_DIR to keep it away from
    // a real session.
    #[cfg(feature = "dev-tools")]
    if let Some(count) = devseed::requested() {
        let instance = instance_guard();
        if instance.as_ref().is_some_and(|i| !i.is_single()) {
            eprintln!("pixme: quit the running instance before seeding the session");
            logging::shutdown();
            std::process::exit(2);
        }
        let _guard = instance;
        let code = devseed::run(count);
        logging::shutdown();
        std::process::exit(code);
    }

    // Single instance: PixMe is a resident app (menu bar / tray) owning
    // one session and its database — a second process would race both.
    // The guard must stay bound in `main` for the whole run.
    let instance = instance_guard();
    if instance.as_ref().is_some_and(|i| !i.is_single()) {
        // The message honours the configured language before any
        // window exists — i18n is loaded from settings directly.
        i18n::apply(config::Settings::load().language);
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title(t!("app.title").as_ref())
            .set_description(t!("app.already_running").as_ref())
            .show();
        logging::shutdown();
        std::process::exit(0);
    }
    platform::drag_out::cleanup_stale_files();
    let result = app::run();
    // Every quit path (menu Quit, quit_now, window close) lands back
    // here: the writer guard flushes and joins before the process dies.
    logging::shutdown();
    result
}

/// The OS single-instance lock: a named mutex on Windows, an `flock`ed
/// file on macOS — an OS-level lock the kernel releases even on a kill
/// or crash (no orphan lockfile). Keyed on `data/instance.lock`, i.e.
/// PER INSTALLATION under the Windows portable layout: two PixMe
/// folders may run side by side, each owning its own session and logs.
/// `None` = the guard itself failed — a resident app must not be
/// blocked by that, so the failure only logs.
fn instance_guard() -> Option<single_instance::SingleInstance> {
    let path = paths::instance_lock();
    // The data dir may not exist on a first-ever run — without it the
    // lockfile cannot be created and the guard silently disables.
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Windows mutex names may not contain a backslash — the same key
    // doubles as a lockfile path on macOS where '/' is normal.
    let key = path.to_string_lossy().replace('\\', "/");
    match single_instance::SingleInstance::new(&key) {
        Ok(instance) => Some(instance),
        Err(err) => {
            tracing::warn!("single-instance guard unavailable: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    /// The per-installation lock: two launches from the SAME folder are
    /// refused, two DIFFERENT folders may coexist, and a dropped guard
    /// (kill included — the OS owns the lock, not the file) frees the
    /// path for the next launch.
    #[test]
    fn instance_lock_is_per_directory_and_released() {
        let base = std::env::temp_dir().join(format!("pixme-lock-{}", std::process::id()));
        let dir_a = base.join("a");
        let dir_b = base.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        let key =
            |d: &std::path::Path| d.join("instance.lock").to_string_lossy().replace('\\', "/");

        let first = single_instance::SingleInstance::new(&key(&dir_a)).unwrap();
        assert!(first.is_single());
        // Same folder → refused.
        let second = single_instance::SingleInstance::new(&key(&dir_a)).unwrap();
        assert!(!second.is_single());
        // Different folder → independent instance allowed.
        let other = single_instance::SingleInstance::new(&key(&dir_b)).unwrap();
        assert!(other.is_single());
        // Guard dropped (crash/kill equivalent) → the folder frees up.
        drop(first);
        let relaunch = single_instance::SingleInstance::new(&key(&dir_a)).unwrap();
        assert!(relaunch.is_single());

        std::fs::remove_dir_all(&base).unwrap();
    }
}
