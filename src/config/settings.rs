//! Persisted settings: `prefs.json` inside the app `data/` dir
//! (`crate::paths::settings_file()` — `<exe>\data\prefs.json` on Windows,
//! `~/Library/Application Support/pixme/prefs.json` on macOS).
//!
//! Older locations are migrated ONCE, by copy (the source is never
//! deleted): a `prefs.json` beside the exe or in the per-user config
//! dir is validated then copied when no new file exists yet; the even
//! older `settings.toml` is parsed and re-saved in place.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::editor::document::{
    AreaShape, BlurStyle, BraceKind, Dash, EraserMode, LineEnd, StepMode, StepSize, StepStyle,
    TextAlign, TipShape,
};
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

/// How the capture-bar icons are drawn — the "Style des icônes" setting.
/// Applies to the vertical capture bar only; every other surface keeps
/// the standard monochrome glyphs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IconStyle {
    /// Lucide line icons tinted with the theme tokens — the reference look.
    #[default]
    Standard,
    /// The same Lucide glyphs, each in its own fixed colour.
    Color,
    /// Phosphor two-tone glyphs (`assets/icons-duotone/`), each in its
    /// own fixed colour.
    Duotone,
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

/// How capture tabs are ordered in the viewer — chosen from the tab
/// context menu only (deliberately not a settings-window option).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabSort {
    /// Alphabetically by tab title (numeric runs compare naturally).
    ByName,
    /// Chronologically by capture date, oldest first.
    ByDate,
}

/// One tool's complete option set — every armed tool owns its own
/// copy of every option it can touch (colour, width, dash, opacity,
/// brush, blur, text, step, emoji…), so nothing ever propagates
/// between tools. Fields a tool does not use stay inert.
/// Struct-level `#[serde(default)]` fills any missing key from
/// [`ToolStyle::default`], so new fields never need a migration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolStyle {
    /// Stroke/tool colour.
    pub color: [u8; 3],
    /// Filled-shape colour.
    pub fill_color: [u8; 3],
    /// Stroke width px.
    pub width: f32,
    /// Dashing pattern.
    pub dash: Dash,
    /// Drop-shadow offset px.
    pub shadow: f32,
    /// Object opacity 0–1.
    pub opacity: f32,
    /// Line start glyph (Fin).
    pub line_start: LineEnd,
    /// Line end glyph (Début).
    pub line_end: LineEnd,
    /// Outline thickness.
    pub outline: f32,
    /// Outline colour.
    pub line_outline_color: [u8; 3],
    /// Line drop-shadow toggle.
    pub line_shadow: bool,
    /// Corner radius.
    pub radius: f32,
    /// Freehand tip shape.
    pub tip: TipShape,
    /// Bracket kind.
    pub brace_kind: BraceKind,
    /// Area shape.
    pub shape: AreaShape,
    /// Highlighter coverage opacity.
    pub hl_opacity: f32,
    /// Eraser colour.
    pub eraser_color: [u8; 3],
    /// Eraser size px.
    pub eraser_size: f32,
    /// Eraser brush mode.
    pub eraser_mode: EraserMode,
    /// Blur style.
    pub blur_style: BlurStyle,
    /// Blur strength.
    pub blur_strength: u8,
    /// Text font family.
    pub font: String,
    /// Text font size px.
    pub font_size: f32,
    /// Text bold toggle.
    pub bold: bool,
    /// Text italic toggle.
    pub italic: bool,
    /// Text outline width.
    pub text_outline: f32,
    /// Text outline colour.
    pub text_outline_color: [u8; 3],
    /// Text alignment.
    pub align: TextAlign,
    /// Step marker mode.
    pub step_mode: StepMode,
    /// Step marker style.
    pub step_style: StepStyle,
    /// Step marker size.
    pub step_size: StepSize,
    /// Next number counter.
    pub step_next_num: u32,
    /// Next letter counter.
    pub step_next_let: u32,
    /// Step badge background colour.
    pub step_bg: [u8; 3],
    /// Step badge foreground colour.
    pub step_fg: [u8; 3],
    /// Step outline thickness.
    pub step_ring: f32,
    /// Step outline colour.
    pub step_ring_color: [u8; 3],
    /// Step shadow toggle.
    pub step_shadow: bool,
    /// Step lowercase toggle.
    pub step_lower: bool,
    /// Emoji id (Unicode code point).
    pub emoji_id: u32,
    /// Last side the user dragged an emoji to, image px.
    pub emoji_size: Option<f32>,
}

/// Factory defaults — the same values `Opts::default` seeds.
impl Default for ToolStyle {
    fn default() -> Self {
        Self {
            color: [225, 40, 40],
            fill_color: [80, 160, 235],
            width: 3.0,
            dash: Dash::Solid,
            shadow: 3.0,
            opacity: 1.0,
            line_start: LineEnd::None,
            line_end: LineEnd::Arrow,
            outline: 2.0,
            line_outline_color: [255, 255, 255],
            line_shadow: true,
            radius: 8.0,
            tip: TipShape::Round,
            brace_kind: BraceKind::CurlyLeft,
            shape: AreaShape::Rect,
            hl_opacity: 0.45,
            eraser_color: [255, 255, 255],
            eraser_size: 12.0,
            eraser_mode: EraserMode::Color,
            blur_style: BlurStyle::Blur,
            blur_strength: 75,
            font: "Arial".to_owned(),
            font_size: 32.0,
            bold: false,
            italic: false,
            text_outline: 2.0,
            text_outline_color: [255, 255, 255],
            align: TextAlign::Left,
            step_mode: StepMode::Numbers,
            step_style: StepStyle::Circle,
            step_size: StepSize::Medium,
            step_next_num: 1,
            step_next_let: 1,
            step_bg: [225, 40, 40],
            step_fg: [255, 255, 255],
            step_ring: 5.0,
            step_ring_color: [255, 255, 255],
            step_shadow: true,
            step_lower: false,
            emoji_id: 0x1F449,
            emoji_size: None,
        }
    }
}

/// Last-used Draw tool options, persisted so each tool restores its
/// complete styling on the next Draw session — one `ToolStyle` bank
/// per tool, nothing shared.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DrawOptions {
    /// Per-tool option banks, keyed by tool.
    pub tools: std::collections::BTreeMap<crate::ui::draw::Tool, ToolStyle>,
}

/// Per-monitor persisted preferences, keyed by the monitor's stable id
/// ([`crate::platform::MonitorIdentity::stable_id`]). Entries are never
/// dropped when the monitor disconnects — a reconnected display gets its
/// name and geometry back silently.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MonitorPrefs {
    /// The user's display name for the monitor (`None` = the OS name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The viewer window's rect on this monitor `[x, y, w, h]`, in the
    /// monitor's native units (physical px on Windows, points on macOS)
    /// — applied when the monitor is re-selected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rect_native: Option<[i32; 4]>,
}

/// The complete persisted settings file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// UI language: `auto` follows the OS locale.
    pub language: Language,
    /// Theme preference: `auto` follows the OS appearance.
    pub appearance: Appearance,
    /// Capture-bar icon style (see [`IconStyle`]).
    #[serde(default)]
    pub icon_style: IconStyle,
    /// Include the mouse pointer in captures. Session-only by user
    /// decision: it survives between captures but is never written to
    /// disk — every launch starts without the pointer.
    #[serde(skip)]
    pub include_pointer: bool,
    /// Also copy every capture to the clipboard.
    pub copy_to_clipboard: bool,
    /// Seconds to wait before capturing (0 = capture immediately).
    /// Session-only by user decision: it survives between captures but
    /// is never written to disk — every launch starts with no delay.
    #[serde(skip)]
    pub capture_delay_secs: u32,
    /// Last folder a capture/document was saved to — reopens the next
    /// save dialog there (was session-only before prefs persisted it).
    pub last_save_dir: Option<PathBuf>,
    /// Last folder a file was opened from — seeds File → Open and the
    /// "open last used folder" menu item.
    pub last_open_dir: Option<PathBuf>,
    /// Most-recently-used files (newest first), at most
    /// [`RECENT_FILES_MAX`]: every saved or opened file lands here and
    /// feeds File → Open's recent section.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Session-database retention (never / today / always).
    pub db_retention: DbRetention,
    /// Show the session-database READ/WRITE activity indicators in the
    /// viewer's status strip.
    #[serde(default = "default_db_status")]
    pub db_status: bool,
    /// Tab ordering — `None` = insertion order.
    pub tab_sort: Option<TabSort>,
    /// Filename template for batch/auto saves ("$Y-$M-$D_$H$N$S").
    #[serde(default = "default_filename_template")]
    pub filename_template: String,
    /// Editable filename-template presets shown in the settings dropdown.
    #[serde(default = "default_filename_templates")]
    pub filename_templates: Vec<String>,
    /// Sequential counter consumed by `#` placeholders in the template.
    #[serde(default = "default_filename_counter")]
    pub filename_counter: u64,
    /// Last-used Draw tool options (see [`DrawOptions`]).
    #[serde(default)]
    pub draw_opts: DrawOptions,
    /// Per-monitor preferences, keyed by stable id — names and saved
    /// window geometry; entries survive disconnects (see [`MonitorPrefs`]).
    #[serde(default)]
    pub monitors: std::collections::BTreeMap<String, MonitorPrefs>,
    /// The stable id of the monitor the viewer last sat on — restored
    /// at launch so the app reopens where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_monitor: Option<String>,
}

fn default_filename_template() -> String {
    "$Y-$M-$D_$H$N$S".to_owned()
}
/// The preset list matches the functional reference's defaults.
fn default_filename_templates() -> Vec<String> {
    [
        "$Y-$M-$D_$H$N$S",
        "Screenshot#####",
        "$Y-$M-$D_#####",
        "%USERNAME%_$Y-$M-$D_$H$N$S",
        "%COMPUTERNAME%_$Y-$M-$D_$H$N$S",
        "@@@@@@@@@@@",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}
/// First `#` expansion yields `…001`, not `…000`.
fn default_filename_counter() -> u64 {
    1
}
fn default_db_status() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            appearance: Appearance::default(),
            icon_style: IconStyle::default(),
            draw_opts: DrawOptions::default(),
            include_pointer: false,
            copy_to_clipboard: false,
            capture_delay_secs: 0,
            last_save_dir: None,
            last_open_dir: None,
            recent_files: Vec::new(),
            db_retention: DbRetention::default(),
            db_status: default_db_status(),
            tab_sort: None,
            filename_template: default_filename_template(),
            filename_templates: default_filename_templates(),
            filename_counter: default_filename_counter(),
            monitors: std::collections::BTreeMap::new(),
            last_monitor: None,
        }
    }
}

impl Settings {
    /// Reads `prefs.json`; a missing file falls back to a legacy
    /// `prefs.json` copied over once (exe-adjacent or per-user config
    /// dir), then to `settings.toml` (migrated once), then to defaults.
    /// Any parse error yields defaults and a log warning.
    pub fn load() -> Self {
        let path = config_path();
        if !path.exists() {
            Self::migrate_legacy_prefs(&path);
        }
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(settings) => return settings,
                Err(err) => {
                    tracing::warn!(
                        "invalid preferences file {}: {err}",
                        crate::paths::file_name(&path)
                    )
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                if let Some(legacy) = Self::load_legacy_toml() {
                    legacy.save();
                    return legacy;
                }
            }
            Err(err) => tracing::warn!("cannot read {}: {err}", crate::paths::file_name(&path)),
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
            tracing::warn!("cannot create {}: {err}", crate::paths::file_name(dir));
            return;
        }
        match serde_json::to_string_pretty(self) {
            Ok(text) => {
                if let Err(err) = std::fs::write(&path, text) {
                    tracing::warn!("cannot write {}: {err}", crate::paths::file_name(&path));
                }
            }
            Err(err) => tracing::warn!("cannot serialize settings: {err}"),
        }
    }

    /// Pushes `path` to the front of [`Self::recent_files`] (re-pushing an
    /// existing entry just moves it back to the top) and trims the list to
    /// [`RECENT_FILES_MAX`].
    pub fn push_recent(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(RECENT_FILES_MAX);
    }

    /// Drops `path` from [`Self::recent_files`] — used when a recent
    /// entry turns out to be a dead path.
    pub fn remove_recent(&mut self, path: &PathBuf) {
        self.recent_files.retain(|p| p != path);
    }

    /// One-shot copy of a legacy `prefs.json` into `path` — only when
    /// `path` does not exist yet, only after the candidate parses as a
    /// valid `Settings` (a corrupt old file is skipped, never copied),
    /// and the source is always left intact.
    fn migrate_legacy_prefs(path: &PathBuf) {
        for legacy in crate::paths::legacy_settings_files() {
            if legacy == *path || !legacy.is_file() {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&legacy) else {
                continue;
            };
            if serde_json::from_str::<Self>(&text).is_err() {
                continue;
            }
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            match std::fs::copy(&legacy, path) {
                Ok(_) => tracing::info!(
                    "migrated prefs.json from previous location {}",
                    crate::paths::file_name(&legacy)
                ),
                Err(err) => tracing::warn!(
                    "cannot migrate prefs.json from {}: {err}",
                    crate::paths::file_name(&legacy)
                ),
            }
            return;
        }
    }

    /// The pre-`prefs.json` location (`settings.toml` in the OS config
    /// dir), read once for migration. `None` when absent or unreadable.
    fn load_legacy_toml() -> Option<Self> {
        let path = crate::paths::legacy_settings_dir()?.join("settings.toml");
        let text = std::fs::read_to_string(&path).ok()?;
        let settings: Self = toml::from_str(&text).ok()?;
        tracing::info!(
            "migrated settings.toml → {}",
            crate::paths::file_name(&config_path())
        );
        Some(settings)
    }
}

/// Cap on [`Settings::recent_files`] — File → Open shows them all.
pub const RECENT_FILES_MAX: usize = 5;

/// `data/prefs.json` — resolved by `crate::paths`, the single owner
/// of data locations.
fn config_path() -> PathBuf {
    crate::paths::settings_file()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn push_recent_dedups_moves_to_front_and_caps() {
        let mut s = Settings::default();
        for i in 0..7 {
            s.push_recent(PathBuf::from(format!("/tmp/{i}.png")));
        }
        assert_eq!(s.recent_files.len(), RECENT_FILES_MAX);
        assert_eq!(s.recent_files[0], PathBuf::from("/tmp/6.png"));
        assert_eq!(
            s.recent_files[RECENT_FILES_MAX - 1],
            PathBuf::from("/tmp/2.png")
        );

        s.push_recent(PathBuf::from("/tmp/3.png"));
        assert_eq!(s.recent_files[0], PathBuf::from("/tmp/3.png"));
        assert_eq!(
            s.recent_files
                .iter()
                .filter(|p| p.as_path() == Path::new("/tmp/3.png"))
                .count(),
            1,
            "re-pushed entry must not duplicate"
        );
        assert_eq!(s.recent_files.len(), RECENT_FILES_MAX);
    }

    #[test]
    fn remove_recent_drops_dead_paths() {
        let mut s = Settings::default();
        s.push_recent(PathBuf::from("/tmp/a.png"));
        s.push_recent(PathBuf::from("/tmp/b.png"));
        s.remove_recent(&PathBuf::from("/tmp/b.png"));
        assert_eq!(s.recent_files, vec![PathBuf::from("/tmp/a.png")]);
        s.remove_recent(&PathBuf::from("/tmp/missing.png"));
        assert_eq!(s.recent_files, vec![PathBuf::from("/tmp/a.png")]);
    }

    /// The monitors map round-trips through JSON, including the sparse
    /// optional fields (a name-only entry writes no `rect_native` key).
    #[test]
    fn monitor_prefs_serde_round_trip() {
        let mut s = Settings::default();
        s.monitors.insert(
            "uuid-1".into(),
            MonitorPrefs {
                name: Some("Dell bureau".into()),
                rect_native: Some([0, 0, 2560, 1440]),
            },
        );
        s.monitors.insert(
            "uuid-2".into(),
            MonitorPrefs {
                name: Some("Side".into()),
                rect_native: None,
            },
        );
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("rect_native\":null"));
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.monitors, s.monitors);
        // A prefs file without the key at all still loads (older versions).
        let bare: Settings = serde_json::from_str("{}").unwrap();
        assert!(bare.monitors.is_empty());
    }
}
