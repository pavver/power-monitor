//! The device's own panel in the iframe. Its page switches screens with a
//! global `show(name)`; the admin wraps it to share the screen with the
//! other admins, and calls the original to follow them.

use leptos::prelude::*;
use leptos::task::spawn_local;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

use crate::api;
use crate::state::Admin;

const SHOW: &str = "show";
const ORIGINAL: &str = "__orig_show";

/// Switches the panel to screen `view` without telling the server.
pub fn show(iframe: &web_sys::HtmlIFrameElement, view: &str) {
    let Some(window) = iframe.content_window() else {
        return;
    };
    let Ok(original) = js_sys::Reflect::get(&window, &JsValue::from_str(ORIGINAL)) else {
        return;
    };
    if let Some(original) = original.dyn_ref::<js_sys::Function>() {
        let _ = original.call1(&window, &JsValue::from_str(view));
    }
}

/// After the iframe loaded: a page of the admin itself (the relay sent the
/// browser back) closes the panel; the device's panel gets its `show`
/// wrapped and jumps to the screen the server has.
pub fn loaded(admin: Admin) {
    let Some(window) = admin
        .iframe_ref
        .get_untracked()
        .and_then(|iframe| iframe.content_window())
    else {
        return;
    };
    let location = window.location();
    if location.href().is_ok_and(|href| href == "about:blank") {
        return;
    }
    if location
        .pathname()
        .is_ok_and(|path| path.starts_with("/admin"))
    {
        let _ = location.replace("about:blank");
        admin.panel_online.set(false);
        admin.iframe_src.set("about:blank".to_string());
        return;
    }
    let Some(original) = original_show(&window) else {
        return;
    };

    // A newly opened panel jumps to the server's view right after load.
    if let Some(id) = admin.selected_id.get_untracked() {
        let view = admin.devices.with_untracked(|list| {
            list.iter()
                .find(|d| d.id == id)
                .and_then(|d| d.view.clone())
        });
        if let Some(view) = view {
            let _ = original.call1(&window, &JsValue::from_str(&view));
            admin.server_view.set_value(Some(view.clone()));
            admin.last_view.set_value(Some(view));
        }
    }

    let target = window.clone();
    let wrapped = Closure::wrap(Box::new(move |arg: JsValue| {
        let _ = original.call1(&target, &arg);
        let Some(name) = arg.as_string() else { return };
        admin.last_view.set_value(Some(name.clone()));
        admin.local_moves.update_value(|n| *n = n.wrapping_add(1));
        if let Some(id) = admin.selected_id.get_untracked() {
            spawn_local(async move {
                let reply =
                    api::post(&format!("/admin/devices/{id}/view"), &[("view", &name)]).await;
                // The server has it now: a list showing it is no news. A
                // screen it does not share (an editor) is simply kept here.
                if matches!(reply, api::Reply::Ok(_)) {
                    admin.server_view.set_value(Some(name));
                }
            });
        }
    }) as Box<dyn FnMut(JsValue)>);
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str(SHOW),
        wrapped.as_ref().unchecked_ref(),
    );
    wrapped.forget();
}

/// The page's own `show`, kept aside the first time it is wrapped.
fn original_show(window: &web_sys::Window) -> Option<js_sys::Function> {
    let show = js_sys::Reflect::get(window, &JsValue::from_str(SHOW))
        .ok()
        .filter(JsValue::is_function)?;
    let kept = js_sys::Reflect::get(window, &JsValue::from_str(ORIGINAL))
        .ok()
        .filter(JsValue::is_function);
    if let Some(kept) = kept {
        return Some(kept.unchecked_into());
    }
    let _ = js_sys::Reflect::set(window, &JsValue::from_str(ORIGINAL), &show);
    Some(show.unchecked_into())
}
