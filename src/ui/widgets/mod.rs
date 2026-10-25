//! Reusable widgets in the visual reference's style. Every colour and radius
//! comes from [`crate::ui::theme::tokens`] — never hard-coded.

// Re-exports are the public widget API; they are consumed from phase 2 on.
#![allow(unused_imports)]

mod button;
mod card;
mod dropdown;
mod elided_label;
mod icon_button;
mod separator;
mod tool_button;

pub use button::{primary_button, secondary_button};
pub use card::{card_frame, card_shadow};
pub use dropdown::dropdown_button;
pub use elided_label::elided_label;
pub use icon_button::icon_button;
pub use separator::{hairline, vline};
pub use tool_button::{TOOL_H, caret_hit, tool_button};
