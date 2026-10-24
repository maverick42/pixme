//! User settings persisted as TOML under the OS config directory.
//!
//! Only serializable preferences live here. `Settings::load` never panics: a
//! missing or corrupt file falls back to defaults.

mod settings;

// `Appearance` is re-exported for the settings UI (phase 2).
#[allow(unused_imports)]
pub use settings::{Appearance, Settings};
