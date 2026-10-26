//! Persisted settings: `prefs.json`.
//!
//! The file lives **next to the executable** on both OSes (portable-mode
//! behaviour — e.g. `C:\portable\pixme\prefs.json`). The one exception: a
//! binary running inside a `.app` bundle cannot write beside itself, so
//! it falls back to `~/Library/Application Support/pixme/prefs.json`.
//! An older `settings.toml` in the per-OS config dir is migrated once,
//! silently.

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

/// How long captures persist in the session database — the
/// "Sauvegarde base de données" setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DbRetention {
    /// The session is never persisted.
    Never,
    /// Only today's captures are kept — anything older is pruned at
    /// launch and when the option is chosen.
    Today,
    /// Everything persists until the user clears it.
    #[default]
    Always,
}

impl DbRetention {
    /// `true` when switching from `self` to `other` may lose stored
    /// captures — the settings UI warns before applying.
    pub fn more_restrictive(self, other: Self) -> bool {
        fn rank(r: DbRetention) -> u8 {
            match r {
                DbRetention::Never => 0,
                DbRetention::Today => 1,
                DbRetention::Always => 2,
            }
        }
        rank(other) < rank(self)
    }
}

/// The complete persisted settings file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: `auto` follows the OS locale.
    pub language: Language,
    /// Theme preference: `auto` follows the OS appearance.
    pub appearance: Appearance,
    /// Include the mouse pointer in captures.
    pub include_pointer: bool,
    /// Also copy every capture to the clipboard.
    pub copy_to_clipboard: bool,
    /// Seconds to wait before capturing (0 = capture immediately).
    pub capture_delay_secs: u32,
    /// Reset the capture delay to 0 after each capture (the reference's
    /// default behaviour).
    #[serde(default = "default_true")]
    pub delay_auto_reset: bool,
    /// Last folder a capture/document was saved to — reopens the next
    /// save dialog there (was session-only before prefs persisted it).
    pub last_save_dir: Option<PathBuf>,
    /// Last folder a file was opened from — seeds File → Open and the
    /// "open last used folder" menu item.
    pub last_open_dir: Option<PathBuf>,
    /// Session-database retention (never / today / always).
    pub db_retention: DbRetention,
    /// Text-tool colour `[r, g, b]` persisted across sessions.
    #[serde(default = "default_text_color")]
    pub text_color: [u8; 3],
    /// Text-tool font family ("Arial", "Inter", …).
    #[serde(default = "default_text_font")]
    pub text_font: String,
    /// Text-tool font size in px (menu 8..=72, default 12).
    #[serde(default = "default_text_size")]
    pub text_size: u8,
    /// Text-tool bold toggle.
    pub text_bold: bool,
    /// Text-tool italic toggle.
    pub text_italic: bool,
    /// Text-tool outline width (0..=10).
    pub text_outline: u8,
    /// Text-tool outline colour `[r, g, b]`.
    #[serde(default = "default_outline_color")]
    pub text_outline_color: [u8; 3],
}

fn default_true() -> bool {
    true
}
fn default_text_color() -> [u8; 3] {
    [220, 40, 40]
}
fn default_text_font() -> String {
    "Arial".to_owned()
}
fn default_text_size() -> u8 {
    12
}
fn default_outline_color() -> [u8; 3] {
    [255, 255, 255]
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            appearance: Appearance::default(),
            include_pointer: false,
            copy_to_clipboard: false,
            capture_delay_secs: 0,
            delay_auto_reset: true,
            last_save_dir: None,
            last_open_dir: None,
            db_retention: DbRetention::default(),
            text_color: default_text_color(),
            text_font: default_text_font(),
            text_size: default_text_size(),
            text_bold: false,
            text_italic: false,
            text_outline: 0,
            text_outline_color: default_outline_color(),
        }
    }
}

impl Settings {
    /// Reads `prefs.json`; a missing file falls back to the legacy
    /// `settings.toml` (migrated once), then to defaults. Any parse error
    /// yields defaults and a log warning.
    pub fn load() -> Self {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => return settings,
                Err(err) => {
                    tracing::warn!("invalid preferences file {}: {err}", path.display())
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if let Some(legacy) = Self::load_legacy_toml() {
                    legacy.save();
                    return legacy;
                }
            }
            Err(err) => tracing::warn!("cannot read {}: {err}", path.display()),
        }
        Self::default()
    }

    /// Writes the preferences file, creating the directory if needed.
    /// Failures are logged, not fatal: losing a preference is recoverable.
    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot create {}: {err}", dir.display());
            return;
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    tracing::warn!("cannot write {}: {err}", path.display());
                }
            }
            Err(err) => tracing::warn!("cannot serialize settings: {err}"),
        }
    }

    /// The pre-`prefs.json` location (`settings.toml` in the OS config
    /// dir), read once for migration. `None` when absent or unreadable.
    fn load_legacy_toml() -> Option<Self> {
        let path = directories::ProjectDirs::from("", "", "pixme")
            .map(|dirs| dirs.config_dir().join("settings.toml"))?;
        let text = std::fs::read_to_string(&path).ok()?;
        let settings: Self = toml::from_str(&text).ok()?;
        tracing::info!("migrated settings.toml → {}", config_path().display());
        Some(settings)
    }
}

/// `prefs.json` beside the executable; a `.app`-bundled binary cannot
/// write inside its bundle, so it lands in the per-user config dir.
/// The bare `prefs.json` fallback never silently fails to persist.
fn config_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if exe.to_string_lossy().contains(".app/Contents/MacOS") {
            return directories::ProjectDirs::from("", "", "pixme").map_or_else(
                || PathBuf::from("prefs.json"),
                |dirs| dirs.config_dir().join("prefs.json"),
            );
        }
        if let Some(dir) = exe.parent() {
            return dir.join("prefs.json");
        }
    }
    PathBuf::from("prefs.json")
}
