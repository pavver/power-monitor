//! Stored templates that a template calls as `{@name}`, and the checks
//! that keep the calls from going round in a circle or too deep.

use std::fmt;

use crate::template::MAX_DEPTH;

/// Validates a template name: 1 to 15 bytes of `[a-z0-9_]`.
pub fn template_name(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 15
        && text.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// Read-only access to stored templates.
pub trait Library {
    /// Calls `with` once with the template's text and returns `true`,
    /// or returns `false` when there is no such template.
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool;
}

/// A library with no templates.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoTemplates;

impl Library for NoTemplates {
    fn read(&self, _name: &str, _with: &mut dyn FnMut(&str)) -> bool {
        false
    }
}

impl Library for [(&str, &str)] {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        if let Some((_, text)) = self.iter().find(|(k, _)| *k == name) {
            with(text);
            true
        } else {
            false
        }
    }
}

impl<const N: usize> Library for [(&str, &str); N] {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        self.as_slice().read(name, with)
    }
}

impl<T: Library + ?Sized> Library for &T {
    fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
        (**self).read(name, with)
    }
}

/// Errors detected when checking template call hierarchies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    Cycle(Vec<String>),
    TooDeep(Vec<String>),
}

impl fmt::Display for CallError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CallError::Cycle(chain) => {
                write!(f, "шаблони викликають один одного по колу: {}", chain.join(" → "))
            }
            CallError::TooDeep(chain) => {
                write!(f, "забагато вкладених викликів: {} (можна {})", chain.join(" → "), MAX_DEPTH)
            }
        }
    }
}

/// The template names `template` calls, in order, including those in
/// sections whatever their condition.
pub fn calls(template: &str) -> Vec<&str> {
    let mut names = Vec::new();
    crate::template::each_call(template, &mut |name| names.push(name));
    names
}

/// Checks that saving `name` with `text` does not create a cycle or a call chain deeper than `MAX_DEPTH`.
///
/// `text` is the new text for `name` (it replaces whatever `library` holds for `name`).
/// Other templates called by `text` are read depth-first from `library`; stops at the first fault.
///
/// Note: a message (event) text is not a template, so it is depth 0 and its calls can be checked
/// with this function by passing a name that no template can have (e.g. `""`).
pub fn check_calls(name: &str, text: &str, library: &dyn Library) -> Result<(), CallError> {
    let mut chain: Vec<String> = Vec::new();
    if !name.is_empty() {
        chain.push(name.to_string());
    }
    check_calls_inner(name, text, library, &mut chain)
}

fn check_calls_inner(
    root_name: &str,
    text: &str,
    library: &dyn Library,
    chain: &mut Vec<String>,
) -> Result<(), CallError> {
    for called in calls(text) {
        if let Some(pos) = chain.iter().position(|n| n == called) {
            let mut cycle = chain[pos..].to_vec();
            cycle.push(called.to_string());
            return Err(CallError::Cycle(cycle));
        }

        chain.push(called.to_string());
        if chain.len() > MAX_DEPTH {
            return Err(CallError::TooDeep(chain.clone()));
        }

        let mut err = None;
        let _ = if !root_name.is_empty() && called == root_name {
            err = check_calls_inner(root_name, text, library, chain).err();
            true
        } else {
            library.read(called, &mut |callee_text| {
                err = check_calls_inner(root_name, callee_text, library, chain).err();
            })
        };

        chain.pop();

        if let Some(e) = err {
            return Err(e);
        }
    }
    Ok(())
}

