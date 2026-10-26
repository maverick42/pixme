//! PixMe — a lightweight, standalone screenshot capture and annotation tool.
//!
//! Entry point: installs tracing, loads the embedded translations and hands
//! off to [`app::run`]. All application state lives in [`app::PixMeApp`].

// Phase 1 scaffold: public module APIs (capture, editor, export, widgets) are
// consumed from phase 2 onward. Remove this allow as real callers land.
#![allow(dead_code)]

#[macro_use]
extern crate rust_i18n;

mod app;
mod capture;
mod config;
mod editor;
mod export;
mod i18n;
mod platform;
mod store;
mod ui;

i18n!("locales", fallback = "en");

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

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
        std::process::exit(0);
    }
    app::run()
}

/// The OS single-instance lock: a named mutex on Windows, an `flock`ed
/// file on macOS. `None` = the guard itself failed — a resident app
/// must not be blocked by that, so the failure only logs.
fn instance_guard() -> Option<single_instance::SingleInstance> {
    let path = directories::ProjectDirs::from("", "", "pixme")
        .map(|d| d.data_dir().join("instance.lock"))
        .unwrap_or_else(|| std::path::PathBuf::from("pixme-instance.lock"));
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
