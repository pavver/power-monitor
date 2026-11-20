//! Template library management and dependency tracking.

use autotext::automation::Automation;
use autotext::library::{self, Library};

use crate::texts::error::TextsError;

/// The template the live status message is rendered from.
pub const STATUS_TEMPLATE: &str = "status";

/// Maximum number of named templates allowed in the library.
pub const MAX_TEMPLATES: usize = 32;

/// List of template names currently stored on the device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Templates {
    names: Vec<String>,
}

impl Default for Templates {
    fn default() -> Self {
        Self::new()
    }
}

impl Templates {
    /// Creates an empty template list.
    pub fn new() -> Self {
        Self { names: Vec::new() }
    }

    /// Slice of registered template names.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Checks whether a template by `name` is present.
    pub fn contains(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    /// Adds a new template name, checking validity and capacity.
    pub fn add(&mut self, name: &str) -> Result<(), TextsError> {
        if !library::template_name(name) {
            return Err(TextsError::BadTemplateName(name.to_string()));
        }
        if self.contains(name) {
            return Ok(());
        }
        if self.names.len() >= MAX_TEMPLATES {
            return Err(TextsError::TooManyTemplates);
        }
        self.names.push(name.to_string());
        Ok(())
    }

    /// Removes a template name from the list. Returns true if it was present.
    pub fn remove(&mut self, name: &str) -> bool {
        if let Some(pos) = self.names.iter().position(|n| n == name) {
            self.names.remove(pos);
            true
        } else {
            false
        }
    }

    /// Serializes template names to a stored comma-separated string.
    pub fn to_stored(&self) -> String {
        self.names.join(",")
    }

    /// Restores template names from a stored comma-separated string.
    pub fn from_stored(stored: &str) -> Self {
        let mut t = Self::new();
        for part in stored.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            let _ = t.add(part);
        }
        t
    }
}

/// An in-memory template library for tests and host evaluation.
#[derive(Debug, Clone, Default)]
pub struct InMemoryLibrary {
    templates: Vec<(String, String)>,
}

impl InMemoryLibrary {
    pub fn new() -> Self {
        Self {
            templates: Vec::new(),
        }
    }

    pub fn insert(&mut self, name: impl Into<String>, text: impl Into<String>) {
        let name = name.into();
        let text = text.into();
        if let Some(pos) = self.templates.iter().position(|(k, _)| k == &name) {
            self.templates[pos].1 = text;
        } else {
            self.templates.push((name, text));
        }
    }

    pub fn remove(&mut self, name: &str) -> bool {
        if let Some(pos) = self.templates.iter().position(|(k, _)| k == name) {
            self.templates.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.templates
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }
}

impl Library for InMemoryLibrary {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        if let Some((_, text)) = self.templates.iter().find(|(k, _)| k == name) {
            with(text);
            true
        } else {
            false
        }
    }
}

/// Checks whether `text` contains a call to `target` (as `{@target}`).
pub fn calls_template(text: &str, target: &str) -> bool {
    library::calls(text).into_iter().any(|c| c == target)
}

/// Finds which other templates or automations depend on `target`.
///
/// Designed so the firmware can check dependencies by reading each template text once.
pub fn callers<'a>(
    target: &str,
    templates: impl IntoIterator<Item = (&'a str, &'a str)>,
    automations: impl IntoIterator<Item = &'a Automation>,
) -> Vec<String> {
    let mut result = Vec::new();
    if target == STATUS_TEMPLATE {
        result.push("статус".to_string());
    }
    for (name, text) in templates {
        if name != target && calls_template(text, target) {
            result.push(name.to_string());
        }
    }
    for (i, auto) in automations.into_iter().enumerate() {
        if calls_template(&auto.text, target) {
            result.push(format!("автоматизація #{}", i + 1));
        }
    }
    result
}

/// Refuses deletion of `target` if any other template or automation calls it.
pub fn check_delete_template<'a>(
    target: &str,
    templates: impl IntoIterator<Item = (&'a str, &'a str)>,
    automations: impl IntoIterator<Item = &'a Automation>,
) -> Result<(), TextsError> {
    let in_use = callers(target, templates, automations);
    if !in_use.is_empty() {
        return Err(TextsError::TemplateInUse {
            name: target.to_string(),
            callers: in_use,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use autotext::automation::{Action, Trigger};

    #[test]
    fn templates_add_remove_and_capacity() {
        let mut list = Templates::new();
        assert!(list.add("test").is_ok());
        assert!(list.contains("test"));
        assert_eq!(list.names(), &["test".to_string()]);

        // Duplicate add is ok
        assert!(list.add("test").is_ok());
        assert_eq!(list.names().len(), 1);

        // Remove
        assert!(list.remove("test"));
        assert!(!list.contains("test"));
        assert!(!list.remove("test"));

        // Capacity check
        for i in 0..MAX_TEMPLATES {
            let name = format!("t_{i}");
            assert!(list.add(&name).is_ok());
        }
        assert_eq!(list.names().len(), MAX_TEMPLATES);
        assert_eq!(list.add("one_more"), Err(TextsError::TooManyTemplates));
    }

    #[test]
    fn templates_storage_round_trip() {
        let mut list = Templates::new();
        list.add("alpha").unwrap();
        list.add("beta").unwrap();
        list.add("gamma").unwrap();

        let stored = list.to_stored();
        assert_eq!(stored, "alpha,beta,gamma");

        let restored = Templates::from_stored(&stored);
        assert_eq!(restored, list);
    }

    #[test]
    fn in_memory_library_read() {
        let mut lib = InMemoryLibrary::new();
        lib.insert("battery", "100%");
        let mut out = String::new();
        let found = lib.read("battery", &mut |text| out.push_str(text));
        assert!(found);
        assert_eq!(out, "100%");

        let missing = lib.read("absent", &mut |_| ());
        assert!(!missing);
    }

    #[test]
    fn callers_detection_for_templates_and_automations() {
        let target = "battery_icon";
        let templates = [
            ("battery", "{@battery_icon} 90%"),
            ("svitlo", "Мережа є"),
            ("status", "{@battery}"),
        ];
        let auto1 = Automation {
            trigger: Trigger::StartUp,
            conditions: vec![],
            action: Action::Send,
            text: "Старт: {@battery_icon}".to_string(),
        };
        let auto2 = Automation {
            trigger: Trigger::StartUp,
            conditions: vec![],
            action: Action::Send,
            text: "Без виклику".to_string(),
        };
        let autos = [auto1, auto2];

        let found = callers(target, templates, &autos);
        assert_eq!(found, vec!["battery".to_string(), "автоматизація #1".to_string()]);

        let err = check_delete_template(target, templates, &autos);
        assert!(matches!(err, Err(TextsError::TemplateInUse { .. })));
        assert_eq!(
            err.unwrap_err().to_string(),
            "неможливо видалити шаблон 'battery_icon', його використовують: battery, автоматизація #1"
        );

        // A template not called anywhere can be deleted
        assert!(check_delete_template("svitlo", templates, &autos).is_ok());
    }

    #[test]
    fn the_status_template_is_used_by_the_status() {
        let templates = [(STATUS_TEMPLATE, "{device}"), ("svitlo", "Мережа є")];
        assert_eq!(callers(STATUS_TEMPLATE, templates, &[]), vec!["статус".to_string()]);
        assert_eq!(
            check_delete_template(STATUS_TEMPLATE, templates, &[]).unwrap_err().to_string(),
            "неможливо видалити шаблон 'status', його використовують: статус"
        );
        assert!(check_delete_template("svitlo", templates, &[]).is_ok());
    }
}
