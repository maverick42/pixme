//! Persisted settings: `settings.toml` in the per-OS config directory
//! (`~/Library/Application Support/pixme` on macOS, `%APPDATA%\pixme` on
//! Windows).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::i18n::Language;
use crate::ui::theme::ThemeKind;

/// UI appearance preference: follow the OS, or force one theme.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    /// Follow the operating system's light/dark mode; falls back to dark when
    /// the OS does not report one.
    #[default]
    Auto,
    /// Always use the dark palette.
    Dark,
    /// Always use the light palette.
    Light,
}

impl Appearance {
    /// Resolves the preference to a concrete theme, given the OS-reported
    /// theme (`ctx.system_theme()`).
    pub fn kind(self, system_theme: Option<egui::Theme>) -> ThemeKind {
        match self {
            Self::Dark => ThemeKind::Pro,
            Self::Light => ThemeKind::ProLight,
            Self::Auto => match system_theme {
                Some(egui::Theme::Light) => ThemeKind::ProLight,
                // Dark, or no OS-reported theme: fall back to dark.
                Some(egui::Theme::Dark) | None => ThemeKind::Pro,
            },
        }
    }
}

/// The complete persisted settings file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: `auto` follows the OS locale.
    pub language: Language,
    /// Theme preference: `auto` follows the OS appearance.
    pub appearance: Appearance,
}

impl Settings {
    /// Reads `settings.toml` from the config directory; any error (missing
    /// file, bad TOML, unknown value) yields defaults and a log warning.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(settings) => return settings,
                Err(err) => {
                    tracing::warn!("invalid settings file {}: {err}", path.display())
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => tracing::warn!("cannot read {}: {err}", path.display()),
        }
        Self::default()
    }

    /// Writes the settings file, creating the config directory if needed.
    /// Failures are logged, not fatal: losing a preference is recoverable.
    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot create {}: {err}", dir.display());
            return;
        }
        match toml::to_string(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    tracing::warn!("cannot write {}: {err}", path.display());
                }
            }
            Err(err) => tracing::warn!("cannot serialize settings: {err}"),
        }
    }
}

/// `<config-dir>/pixme/settings.toml`; falls back to `./settings.toml` when the
/// OS config directory cannot be determined (never silently fails to persist).
fn config_path() -> PathBuf {
    directories::ProjectDirs::from("", "", "pixme").map_or_else(
        || PathBuf::from("settings.toml"),
        |dirs| dirs.config_dir().join("settings.toml"),
    )
}
