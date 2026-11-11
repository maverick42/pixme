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
