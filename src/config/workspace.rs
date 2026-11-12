//! Workspaces: named buckets partitioning the capture session.
//!
//! Each workspace owns a display name, the naming mask its new
//! captures are titled with (plus the `#` counter that feeds it) and
//! the folder its captures are proposed to save to. `main` is the
//! undeletable default every capture resolves to; it is created from
//! the pre-workspaces global settings on the first launch after the
//! feature landed.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The reserved id of the default workspace — undeletable but fully
/// editable (name, mask, folder). Unknown ids resolve to it so a
/// prefs reset never loses captures.
pub const MAIN_ID: &str = "main";

/// One capture bucket: a name, a naming mask and a save folder.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Workspace {
    /// Stable generated id — never reused, never derived from `name`
    /// (renaming must not silently move captures between buckets).
    pub id: String,
    /// Display name — unique case-insensitively.
    pub name: String,
    /// Filename template titling the workspace's new captures
    /// (`$Y`, `#`, `@`, `%VAR%` — see [`crate::config::filename`]).
    pub name_mask: String,
    /// The folder this workspace's captures are proposed to save to —
    /// updated to the folder actually used on every successful save,
    /// the per-workspace heir of the removed global `last_save_dir`.
    /// `None` falls back to the user's Pictures folder.
    pub output_dir: Option<PathBuf>,
    /// The image format Save All writes in this workspace — the
    /// single-file save dialog still leaves the format free.
    pub format: crate::export::ImageFormat,
    /// Sequential counter consumed by `#` placeholders in `name_mask`.
    pub counter: u64,
}

impl Workspace {
    /// A fresh workspace: new id, default mask, untouched counter.
    pub fn new(name: String) -> Self {
        Self {
            id: crate::editor::meta::new_id(),
            name,
            name_mask: crate::config::settings::default_filename_template(),
            output_dir: None,
            format: crate::export::ImageFormat::default(),
            counter: 1,
        }
    }

    /// The reserved default workspace — `main` cannot be created by
    /// the user (ids are generated), only seeded here.
    pub fn main(locale: &str) -> Self {
        Self {
            id: MAIN_ID.to_owned(),
            name: Self::default_name(locale),
            name_mask: crate::config::settings::default_filename_template(),
            output_dir: None,
            format: crate::export::ImageFormat::default(),
            counter: 1,
        }
    }

    /// The name `main` is created with — localized ONCE at creation
    /// (the stored string never re-translates afterwards).
    pub fn default_name(locale: &str) -> String {
        match locale {
            "fr" => "Principal",
            "es" => "Principal",
            _ => "Main",
        }
        .to_owned()
    }
}

/// The display order every workspace list shares: `main` first, then
/// the others alphabetically (case-insensitive, id as tie-break). The
/// status-strip switcher and the tab menu's "Workspace →" destinations
/// both read it so the two lists never disagree.
pub fn ordered(workspaces: &[Workspace]) -> Vec<&Workspace> {
    let mut rest: Vec<&Workspace> = workspaces.iter().filter(|w| w.id != MAIN_ID).collect();
    rest.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then_with(|| a.id.cmp(&b.id))
    });
    workspaces
        .iter()
        .find(|w| w.id == MAIN_ID)
        .into_iter()
        .chain(rest)
        .collect()
}

/// The workspaces a capture of `active` may move to — [`ordered`]
/// minus the workspace it already belongs to. Empty when no
/// destination exists (only `main`, or `active` is the only one):
/// the menu entry greys out rather than hiding.
pub fn move_destinations<'a>(workspaces: &'a [Workspace], active: &str) -> Vec<&'a Workspace> {
    ordered(workspaces)
        .into_iter()
        .filter(|w| w.id != active)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(id: &str, name: &str) -> Workspace {
        Workspace {
            id: id.to_owned(),
            name: name.to_owned(),
            ..Workspace::new(name.to_owned())
        }
    }

    /// `main` leads, the rest sorts alphabetically case-insensitively.
    #[test]
    fn ordered_puts_main_first_then_alphabetical() {
        let list = vec![
            ws("b", "Zulu"),
            ws("a", "apple"),
            ws(MAIN_ID, "Main"),
            ws("c", "Banana"),
        ];
        let ids: Vec<&str> = ordered(&list).iter().map(|w| w.id.as_str()).collect();
        assert_eq!(ids, [MAIN_ID, "a", "c", "b"]);
    }

    /// The destinations drop the capture's current workspace; a store
    /// reduced to `main` (or to the active one alone) offers none —
    /// the caller greys the entry out.
    #[test]
    fn move_destinations_exclude_the_active_workspace() {
        let list = vec![ws("b", "B"), ws("a", "A"), ws(MAIN_ID, "Main")];
        let ids: Vec<&str> = move_destinations(&list, "a")
            .iter()
            .map(|w| w.id.as_str())
            .collect();
        assert_eq!(ids, [MAIN_ID, "b"]);
        assert!(move_destinations(&list[..1], "b").is_empty());
        let main_only = vec![ws(MAIN_ID, "Main")];
        assert!(move_destinations(&main_only, MAIN_ID).is_empty());
    }
}
