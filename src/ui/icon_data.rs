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
        Icon::SquareDashedMousePointer,
        "square-dashed-mouse-pointer",
        include_bytes!("../../assets/icons/square-dashed-mouse-pointer.svg"),
    ),
    (
        Icon::Crop,
        "crop",
        include_bytes!("../../assets/icons/crop.svg"),
    ),
    (
        Icon::Pin,
        "pin",
        include_bytes!("../../assets/icons/pin.svg"),
    ),
    (
        Icon::Clock,
        "clock",
        include_bytes!("../../assets/icons/clock.svg"),
    ),
    (
        Icon::Folder,
        "folder",
        include_bytes!("../../assets/icons/folder.svg"),
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
    (
        Icon::Plus,
        "plus",
        include_bytes!("../../assets/icons/plus.svg"),
    ),
    (
        Icon::Scan,
        "scan",
        include_bytes!("../../assets/icons/scan.svg"),
    ),
    (
        Icon::GripVertical,
        "grip-vertical",
        include_bytes!("../../assets/icons/grip-vertical.svg"),
    ),
    (
        Icon::Copy,
        "copy",
        include_bytes!("../../assets/icons/copy.svg"),
    ),
    (
        Icon::Download,
        "download",
        include_bytes!("../../assets/icons/download.svg"),
    ),
];
