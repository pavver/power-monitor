//! Builds the Operator UI part of the device page from the design system
//! itself (github.com/pavver/operator-design-system, the rev in Cargo.toml):
//! the stylesheet of exactly the components the page uses, their markup as
//! `<template>`s rendered by Leptos on the host, the icons and motion timings.
//! The page's own script gives the markup its behaviour.
//!
//! cargo run --manifest-path ui-bundle/Cargo.toml            writes firmware/ui/index.html
//! cargo run --manifest-path ui-bundle/Cargo.toml -- --check fails if it is out of date

use std::{fs, path::PathBuf, process::ExitCode};

use leptos::prelude::*;
use operator_ui::prelude::*;
use operator_ui::tokens::{Icon as IconData, icons, motion};

/// Every component the page uses: its Cargo feature (in Cargo.toml) and a
/// class only its markup has. A component the page does not use fails the
/// build, so its CSS never reaches the flash.
const COMPONENTS: &[(&str, &str)] = &[("action-button", "op-btn")];

/// The icons the page draws, by the name its script uses.
const ICONS: &[(&str, IconData)] = &[
    ("add", icons::ADD),
    ("back", icons::ARROW_BACK),
    ("check", icons::CHECK),
    ("chevronRight", icons::CHEVRON_RIGHT),
    ("close", icons::CLOSE),
    ("dropDown", icons::ARROW_DROP_DOWN),
    ("eye", icons::VISIBILITY),
    ("eyeOff", icons::VISIBILITY_OFF),
];

const BEGIN: &str = "<!-- operator-ui:begin (ui-bundle writes this part, do not edit) -->";
const END: &str = "<!-- operator-ui:end -->";

fn templates() -> Vec<(&'static str, String)> {
    let no_op = || AsyncAction::new(|()| async { ActionResult::Success });
    vec![
        (
            "action-button",
            view! { <ActionButton text=Signal::stored("-".to_string()) on_click=no_op()/> }.to_html(),
        ),
        ("pending", view! { <PendingIndicator/> }.to_html()),
        (
            "action-button-danger",
            view! { <ActionButton text=Signal::stored("-".to_string()) dangerous=true on_click=no_op()/> }
                .to_html(),
        ),
    ]
}

fn stylesheet() -> String {
    let root = view! { <ThemeRoot><span></span></ThemeRoot> }.to_html();
    let css = root
        .split_once("<style>")
        .and_then(|(_, rest)| rest.split_once("</style>"))
        .map(|(css, _)| css)
        .expect("ThemeRoot renders its stylesheet");
    minify(css)
}

/// Drops comments and the whitespace CSS does not need.
fn minify(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start..].split_once("*/").map_or("", |(_, after)| after);
    }
    out.push_str(rest);
    let words: Vec<&str> = out.split_whitespace().collect();
    let mut packed = words.join(" ");
    for token in ["{", "}", ";", ",", ">"] {
        packed = packed.replace(&format!(" {token}"), token).replace(&format!("{token} "), token);
    }
    // ": " only after a property name: selectors keep `a :hover` apart.
    packed.replace(";}", "}")
}

/// Leptos's hydration markers mean nothing to a page without Leptos.
fn clean(html: &str) -> String {
    html.replace("<!>", "").replace("<!---->", "")
}

fn bundle() -> String {
    let mut out = format!("{BEGIN}\n<style>{}</style>\n", stylesheet());
    for (name, html) in templates() {
        out += &format!("<template id=\"op-{name}\">{}</template>\n", clean(&html));
    }
    let icons: Vec<String> = ICONS
        .iter()
        .map(|(name, icon)| format!("{name}:{:?}", icon.paths.join("")))
        .collect();
    out += &format!(
        "<script>const OP={{icons:{{{}}},motion:{{resultFade:{},resultHold:{},flashIcon:{},flashTail:{},pendingDelay:{},rippleGrow:330,rippleFade:220}}}};</script>\n{END}",
        icons.join(","),
        motion::RESULT_FADE_MS,
        motion::RESULT_HOLD_MS,
        motion::LOCAL_FLASH_ICON_MS,
        motion::LOCAL_FLASH_TAIL_MS,
        motion::PENDING_INDICATOR_DELAY_MS,
    );
    out
}

/// The features Cargo.toml turns on must be COMPONENTS, no more, no less.
fn check_features(manifest: &str) -> Result<(), String> {
    let list = manifest
        .split_once("operator-ui = {")
        .and_then(|(_, rest)| rest.split_once("features = ["))
        .and_then(|(_, rest)| rest.split_once(']'))
        .map(|(list, _)| list)
        .ok_or("Cargo.toml: no operator-ui features")?;
    let mut enabled: Vec<&str> = list.split(',').map(|f| f.trim().trim_matches('"')).filter(|f| !f.is_empty()).collect();
    let mut wanted: Vec<&str> = COMPONENTS.iter().map(|(feature, _)| *feature).collect();
    enabled.sort_unstable();
    wanted.sort_unstable();
    if enabled == wanted {
        Ok(())
    } else {
        Err(format!("Cargo.toml features {enabled:?} differ from COMPONENTS {wanted:?}"))
    }
}

/// Each component's own class must appear in the page outside the bundle.
fn check_used(page_without_bundle: &str) -> Result<(), String> {
    let unused: Vec<&str> = COMPONENTS
        .iter()
        .filter(|(_, class)| !page_without_bundle.contains(class))
        .map(|(feature, _)| *feature)
        .collect();
    if unused.is_empty() {
        Ok(())
    } else {
        Err(format!("the page does not use {unused:?}: drop them from COMPONENTS and Cargo.toml"))
    }
}

fn run() -> Result<(), String> {
    any_spawner::Executor::init_futures_executor().map_err(|e| e.to_string())?;
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let page_path = here.join("../firmware/ui/index.html");
    let manifest = fs::read_to_string(here.join("Cargo.toml")).map_err(|e| e.to_string())?;
    check_features(&manifest)?;

    let page = fs::read_to_string(&page_path).map_err(|e| e.to_string())?;
    let (head, rest) = page.split_once(BEGIN).ok_or("index.html: no operator-ui:begin marker")?;
    let (_, tail) = rest.split_once(END).ok_or("index.html: no operator-ui:end marker")?;
    check_used(&format!("{head}{tail}"))?;

    let fresh = format!("{head}{}{tail}", bundle());
    if std::env::args().any(|a| a == "--check") {
        if fresh != page {
            return Err("firmware/ui/index.html is out of date: cargo run --manifest-path ui-bundle/Cargo.toml".into());
        }
    } else if fresh != page {
        fs::write(&page_path, fresh).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("ui-bundle: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minify_keeps_rules_and_drops_comments() {
        assert_eq!(minify("/* x */\n.a > .b ,\n.c {\n  color : red ;\n}\n"), ".a>.b,.c{color : red}");
    }

    #[test]
    fn a_feature_outside_components_fails() {
        let manifest = "operator-ui = { git = \"x\", features = [\n  \"action-button\",\n  \"chip\",\n] }";
        assert!(check_features(manifest).is_err());
    }

    #[test]
    fn an_unused_component_fails() {
        assert!(check_used("<div class=\"x\"></div>").is_err());
        assert!(check_used("<button class=\"op-btn\"></button>").is_ok());
    }
}
