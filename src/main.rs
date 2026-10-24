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
mod ui;

i18n!("locales", fallback = "en");

fn main() -> eframe::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    app::run()
}
