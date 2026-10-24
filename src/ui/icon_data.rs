//! Embedded Lucide icon set (subset needed by the toolbar and settings).
//!
//! The SVG bytes live in `assets/icons/` with `LICENSE-lucide.txt` (ISC).
//! Rasterization and styling happen in `icons.rs`; this module only maps
//! each [`Icon`] to its embedded bytes and name.

use super::icons::Icon;

/// `(icon, name, raw SVG bytes)`. `name` is the Lucide file name, used in the
/// `bytes://icons/<name>.svg` loader URI.
pub(crate) const DATA: &[(Icon, &str, &[u8])] = &[
    (
        Icon::Monitor,
        "monitor",
        include_bytes!("../../assets/icons/monitor.svg"),
    ),
    (
        Icon::SquareDashed,
        "square-dashed",
        include_bytes!("../../assets/icons/square-dashed.svg"),
    ),
    (
        Icon::AppWindow,
        "app-window",
        include_bytes!("../../assets/icons/app-window.svg"),
    ),
    (
        Icon::Settings,
        "settings",
        include_bytes!("../../assets/icons/settings.svg"),
    ),
    (Icon::X, "x", include_bytes!("../../assets/icons/x.svg")),
    (
        Icon::ChevronDown,
        "chevron-down",
        include_bytes!("../../assets/icons/chevron-down.svg"),
    ),
    (
        Icon::Check,
        "check",
        include_bytes!("../../assets/icons/check.svg"),
    ),
];
