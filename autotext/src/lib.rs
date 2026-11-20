//! Text templates and automations for small devices.
//!
//! Compact template engine designed for microcontrollers and embedded environments:
//! minimal heap allocation, predictable memory, conditional sections (`{name?text}`,
//! `{!name?text}`, `{name=val?text}`), and placeholders (`{name}`).

pub mod condition;
pub mod status;
pub mod template;
pub mod automation;
