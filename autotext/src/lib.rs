//! Text templates and automations for small devices.
//!
//! Compact template engine designed for microcontrollers and embedded environments:
//! minimal heap allocation, predictable memory, conditional sections (`{name?text}`,
//! `{!name?text}`), and placeholders (`{name}`).

pub mod status;
pub mod template;
