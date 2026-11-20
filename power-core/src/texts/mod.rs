//! Text templates, live status message policies, and automations.

pub mod defaults;
pub mod error;
pub mod library;
pub mod stored;
pub mod values;

pub use defaults::{
    default_automations, default_status_config, default_templates, DEFAULT_TEMPLATES,
};
pub use error::TextsError;
pub use library::{
    callers, calls_template, check_delete_template, InMemoryLibrary, Templates, MAX_TEMPLATES,
    STATUS_TEMPLATE,
};
pub use stored::{
    automation_from_form, automation_to_form, check_template, live_from_form, live_to_form,
    status_config_from_form, status_config_to_form,
};
pub use values::{known, values, Snapshot, Values, NAMES};
