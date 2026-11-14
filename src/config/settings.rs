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

use crate::config::workspace::{MAIN_ID, Workspace};
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
            hl_opacity: 1.0,
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
    /// The colour picker's "custom colours" bank — up to
    /// [`crate::ui::theme::picker::CUSTOM_SLOTS`] slots, `None` = empty;
    /// the vec is normalised to the slot count by [`Self::custom_slot`]
    /// and [`Self::set_custom_slot`] so hand-edited prefs still load.
    pub custom_colors: Vec<Option<[u8; 3]>>,
}

impl DrawOptions {
    /// Slot `i` of the custom-colours bank (`None` when empty or past
    /// the end of a short/hand-edited list).
    pub fn custom_slot(&self, i: usize) -> Option<[u8; 3]> {
        self.custom_colors.get(i).copied().flatten()
    }

    /// Writes slot `i` of the custom-colours bank, growing the vec to
    /// the slot count on first use and truncating any surplus a
    /// hand-edited prefs file may carry.
    pub fn set_custom_slot(&mut self, i: usize, c: Option<[u8; 3]>) {
        if i >= crate::ui::theme::picker::CUSTOM_SLOTS {
            return;
        }
        self.custom_colors
            .resize(crate::ui::theme::picker::CUSTOM_SLOTS, None);
        self.custom_colors
            .truncate(crate::ui::theme::picker::CUSTOM_SLOTS);
        self.custom_colors[i] = c;
    }
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
    /// The Draw editor window's rect on this monitor `[x, y, w, h]`,
    /// same native units — applied when Draw reopens on this monitor
    /// (`None` = centre it there).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draw_rect_native: Option<[i32; 4]>,
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
    /// Legacy "last used save folder" — read ONCE by the workspaces
    /// migration (it seeds `main`'s `output_dir`), never written back:
    /// the workspaces carry that role per bucket now.
    #[serde(skip_serializing)]
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
    /// Legacy global filename template — read ONCE by the workspaces
    /// migration into `main`'s `name_mask`, never written back.
    #[serde(default = "default_filename_template", skip_serializing)]
    pub filename_template: String,
    /// Editable filename-template presets shared by every workspace's
    /// mask editor (a bank, not a per-workspace value).
    #[serde(default = "default_filename_templates")]
    pub filename_templates: Vec<String>,
    /// Legacy global sequential counter — read ONCE by the workspaces
    /// migration into `main`'s `counter`, never written back.
    #[serde(default = "default_filename_counter", skip_serializing)]
    pub filename_counter: u64,
    /// The capture workspaces — at least `main` after
    /// [`Self::ensure_workspaces`].
    #[serde(default)]
    pub workspaces: Vec<Workspace>,
    /// The workspace new captures land in — unknown ids resolve to
    /// `main` (repaired at load by `ensure_workspaces`).
    #[serde(default = "default_active_workspace")]
    pub active_workspace_id: String,
    /// Last-used Draw tool options (see [`DrawOptions`]).
    #[serde(default)]
    pub draw_opts: DrawOptions,
    /// Global-hotkey bindings: action → accelerator string (the
    /// `global-hotkey` crate's format, e.g. `"control+shift+KeyR"`).
    /// Empty = no hotkey armed — the settings Hotkeys page edits these.
    #[serde(default)]
    pub hotkeys: std::collections::BTreeMap<crate::hotkeys::HotkeyAction, String>,
    /// Per-monitor preferences, keyed by stable id — names and saved
    /// window geometry; entries survive disconnects (see [`MonitorPrefs`]).
    #[serde(default)]
    pub monitors: std::collections::BTreeMap<String, MonitorPrefs>,
    /// The stable id of the monitor the viewer last sat on — restored
    /// at launch so the app reopens where it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_monitor: Option<String>,
}

/// The factory naming mask — `main` and new workspaces seed with it;
/// `pub(crate)` so [`crate::config::workspace`] can build defaults.
pub(crate) fn default_filename_template() -> String {
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
fn default_active_workspace() -> String {
    MAIN_ID.to_owned()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: Language::default(),
            appearance: Appearance::default(),
            icon_style: IconStyle::default(),
            draw_opts: DrawOptions::default(),
            hotkeys: std::collections::BTreeMap::new(),
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
            workspaces: vec![Workspace::main("en")],
            active_workspace_id: default_active_workspace(),
            monitors: std::collections::BTreeMap::new(),
            last_monitor: None,
        }
    }
}

impl Settings {
    /// Reads `prefs.json`; a missing file falls back to a legacy
    /// `prefs.json` copied over once (exe-adjacent or per-user config
    /// dir), then to `settings.toml` (migrated once), then to defaults.
    /// Any parse error yields defaults and a log warning. The workspaces
    /// migration/repair runs last — and persists right away when it
    /// changed something.
    pub fn load() -> Self {
        let mut settings = Self::load_inner();
        if settings.ensure_workspaces() {
            settings.save();
        }
        settings
    }

    /// The load body before the workspaces migration — split out so
    /// `load` owns the one migration-write entry point.
    fn load_inner() -> Self {
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
    /// The write is ATOMIC — a sibling temp file renamed over the target —
    /// so a crash mid-write can never truncate the preferences. Failures
    /// are logged, not fatal: losing a preference is recoverable.
    pub fn save(&self) {
        let path = config_path();
        if let Some(dir) = path.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("cannot create {}: {err}", crate::paths::file_name(dir));
            return;
        }
        let Ok(text) = serde_json::to_string_pretty(self) else {
            tracing::warn!("cannot serialize settings");
            return;
        };
        let tmp = path.with_file_name("prefs.json.tmp");
        if let Err(err) = std::fs::write(&tmp, text) {
            tracing::warn!("cannot write {}: {err}", crate::paths::file_name(&tmp));
            return;
        }
        if std::fs::rename(&tmp, &path).is_err() {
            // `rename` over an existing destination fails on Windows —
            // drop the target first (the atomicity loss is one unlink deep).
            let _ = std::fs::remove_file(&path);
            if let Err(err) = std::fs::rename(&tmp, &path) {
                tracing::warn!("cannot write {}: {err}", crate::paths::file_name(&path));
            }
        }
    }

    /// `id` when it names a known workspace, else [`MAIN_ID`] — the
    /// runtime fallback for captures whose workspace vanished from
    /// prefs (reset, hand-edited file).
    pub fn resolve_workspace<'a>(&'a self, id: &'a str) -> &'a str {
        if self.workspaces.iter().any(|w| w.id == id) {
            id
        } else {
            MAIN_ID
        }
    }

    /// The workspace new captures land in — `active_workspace_id`
    /// resolved. `ensure_workspaces` (run at load, guaranteed by
    /// `Default`) keeps the list non-empty.
    pub fn active_workspace(&self) -> &Workspace {
        let id = self.resolve_workspace(&self.active_workspace_id);
        if let Some(w) = self.workspaces.iter().find(|w| w.id == id) {
            return w;
        }
        if let Some(w) = self.workspaces.iter().find(|w| w.id == MAIN_ID) {
            return w;
        }
        self.workspaces
            .first()
            .expect("ensure_workspaces keeps a 'main' workspace")
    }

    /// Mutable twin of [`Self::active_workspace`] — the capture
    /// counter and `output_dir` write through it.
    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        let id = self.resolve_workspace(&self.active_workspace_id).to_owned();
        if self.workspaces.iter().any(|w| w.id == id) {
            return self.workspaces.iter_mut().find(|w| w.id == id).unwrap();
        }
        if self.workspaces.iter().any(|w| w.id == MAIN_ID) {
            return self
                .workspaces
                .iter_mut()
                .find(|w| w.id == MAIN_ID)
                .unwrap();
        }
        self.workspaces
            .first_mut()
            .expect("ensure_workspaces keeps a 'main' workspace")
    }

    /// `base`, or `base (2)`, `base (3)`… — the first variant not
    /// already a workspace name (case-insensitive). Pre-fills the
    /// create dialog so its unique-name rule starts satisfied.
    pub fn unique_workspace_name(&self, base: &str) -> String {
        let base = base.trim();
        if !self.workspace_name_taken(base, None) {
            return base.to_owned();
        }
        for n in 2.. {
            let candidate = format!("{base} ({n})");
            if !self.workspace_name_taken(&candidate, None) {
                return candidate;
            }
        }
        unreachable!()
    }

    /// `true` when `name` is already a workspace name, compared
    /// case-insensitively after trimming (`except` = the id being
    /// edited, so a workspace keeps its own name).
    pub fn workspace_name_taken(&self, name: &str, except: Option<&str>) -> bool {
        let name = name.trim().to_lowercase();
        self.workspaces
            .iter()
            .any(|w| Some(w.id.as_str()) != except && w.name.trim().to_lowercase() == name)
    }

    /// Migration + repair, run at the tail of every load: an absent
    /// `workspaces` list is created from the legacy global naming
    /// settings (mask → `main.name_mask`, `last_save_dir` →
    /// `main.output_dir`, counter → `main.counter`); a list missing
    /// `main` gets it re-seeded; a dangling `active_workspace_id`
    /// falls back to `main`. Returns `true` when anything changed —
    /// the caller persists.
    pub fn ensure_workspaces(&mut self) -> bool {
        let mut changed = false;
        if self.workspaces.is_empty() {
            let mask = if self.filename_template.trim().is_empty() {
                default_filename_template()
            } else {
                self.filename_template.clone()
            };
            let mut main = Workspace::main(self.language.locale());
            main.name_mask = mask;
            main.output_dir = self.last_save_dir.take();
            main.counter = self.filename_counter;
            self.workspaces.push(main);
            changed = true;
            tracing::info!("workspaces: migrated legacy naming settings into 'main'");
        }
        if !self.workspaces.iter().any(|w| w.id == MAIN_ID) {
            self.workspaces
                .insert(0, Workspace::main(self.language.locale()));
            changed = true;
            tracing::info!("workspaces: re-seeded missing 'main' workspace");
        }
        let resolved = self.resolve_workspace(&self.active_workspace_id);
        if resolved != self.active_workspace_id {
            self.active_workspace_id = resolved.to_owned();
            changed = true;
        }
        changed
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
                draw_rect_native: Some([100, 50, 1100, 700]),
            },
        );
        s.monitors.insert(
            "uuid-2".into(),
            MonitorPrefs {
                name: Some("Side".into()),
                rect_native: None,
                draw_rect_native: None,
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

    /// The pre-workspaces migration: no `workspaces` list → `main` is
    /// seeded from the legacy global naming keys (mask, folder,
    /// counter), which then stop serializing back.
    #[test]
    fn workspaces_migration_seeds_main_from_legacy_globals() {
        let json = r#"{
            "last_save_dir": "/tmp/shots",
            "filename_template": "Shot##",
            "filename_counter": 7
        }"#;
        let mut s: Settings = serde_json::from_str(json).unwrap();
        assert!(s.workspaces.is_empty());
        assert!(s.ensure_workspaces());
        assert_eq!(s.workspaces.len(), 1);
        let main = &s.workspaces[0];
        assert_eq!(main.id, MAIN_ID);
        assert_eq!(main.name_mask, "Shot##");
        assert_eq!(main.output_dir.as_deref(), Some(Path::new("/tmp/shots")));
        assert_eq!(main.counter, 7);
        assert_eq!(s.active_workspace_id, MAIN_ID);
        // The legacy keys are consumed — a re-save carries none of
        // them (`filename_templates`, the preset bank, legitimately
        // stays — it is not a legacy global).
        let out = serde_json::to_string(&s).unwrap();
        for key in ["last_save_dir", "filename_template", "filename_counter"] {
            assert!(
                !out.contains(&format!("\"{key}\"")),
                "\"{key}\" must not serialize back"
            );
        }
        assert!(out.contains("\"workspaces\""));
        assert!(out.contains("\"active_workspace_id\""));
        // Second pass is a no-op.
        assert!(!s.ensure_workspaces());
    }

    /// A prefs file that HAS workspaces keeps them verbatim — the
    /// migration only seeds a missing list, and a list missing `main`
    /// gets it back.
    #[test]
    fn workspaces_migration_preserves_existing_and_reseeds_main() {
        let mut s = Settings::default();
        s.workspaces.clear();
        let mut w = Workspace::new("Work".to_owned());
        w.counter = 42;
        s.workspaces.push(w);
        assert!(s.ensure_workspaces(), "missing 'main' is re-seeded");
        assert_eq!(s.workspaces[0].id, MAIN_ID);
        assert_eq!(s.workspaces[1].name, "Work");
        assert_eq!(s.workspaces[1].counter, 42, "entries are never rewritten");
        assert!(!s.ensure_workspaces());
    }

    /// An `active_workspace_id` pointing nowhere falls back to `main`,
    /// and `resolve_workspace` never honours an unknown id.
    #[test]
    fn unknown_workspace_falls_back_to_main() {
        let mut s = Settings {
            active_workspace_id: "gone".to_owned(),
            ..Settings::default()
        };
        assert_eq!(s.resolve_workspace("gone"), MAIN_ID);
        assert!(s.ensure_workspaces());
        assert_eq!(s.active_workspace_id, MAIN_ID);
        assert_eq!(s.active_workspace().id, MAIN_ID);
    }

    /// Case-insensitive uniqueness — `except` lets the edited
    /// workspace keep its own name — plus the create-dialog prefill.
    #[test]
    fn workspace_name_taken_is_case_insensitive() {
        let mut s = Settings::default();
        s.workspaces[0].name = "Main".to_owned();
        let mut w = Workspace::new("Work".to_owned());
        w.id = "abc".to_owned();
        s.workspaces.push(w);
        assert!(s.workspace_name_taken(" MAIN ", None));
        assert!(s.workspace_name_taken("work", Some("xyz")));
        assert!(!s.workspace_name_taken("work", Some("abc")));
        assert_eq!(s.unique_workspace_name("main"), "main (2)");
        assert_eq!(s.unique_workspace_name("free"), "free");
    }
}
