#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use gloo_timers::future::TimeoutFuture;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use operator_ui::prelude::*;
use serde::{Deserialize, Serialize};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;

/// How long a screen switch made here wins over the screen the server reports.
const LOCAL_VIEW_SETTLE_MS: f64 = 3000.0;
const POLL_SLOW_MS: u32 = 5000;
const POLL_FAST_MS: u32 = 2000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Login,
    Devices,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub seen: u64,
    pub online: bool,
    pub requested: bool,
    pub waiting: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub view: Option<String>,
}

impl DeviceItem {
    fn display_name(&self) -> &str {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            "без назви"
        } else {
            trimmed
        }
    }

    fn display_meta(&self) -> String {
        let seen_str = ago(self.seen);
        if self.version.is_empty() {
            format!("{} · {}", self.id, seen_str)
        } else {
            format!("{} · v{} · {}", self.id, self.version, seen_str)
        }
    }
}

fn sort_devices(devices: &mut [DeviceItem]) {
    devices.sort_by_key(|d| if d.status == "session" { 0 } else { 1 });
}

fn call_original_show(iframe: &web_sys::HtmlIFrameElement, view_name: &str) {
    let Some(cw) = iframe.content_window() else {
        return;
    };
    let Ok(orig) = js_sys::Reflect::get(&cw, &JsValue::from_str("__orig_show")) else {
        return;
    };
    if let Some(func) = orig.dyn_ref::<js_sys::Function>() {
        let arg = JsValue::from_str(view_name);
        let _ = func.call1(&cw, &arg);
    }
}

fn ago(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} с тому")
    } else if seconds < 3600 {
        format!("{} хв тому", seconds / 60)
    } else if seconds < 86400 {
        format!("{} год тому", seconds / 3600)
    } else {
        format!("{} д тому", seconds / 86400)
    }
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char);
            }
            b' ' => encoded.push('+'),
            _ => {
                encoded.push_str(&format!("%{b:02X}"));
            }
        }
    }
    encoded
}

fn parse_query_open(search: &str) -> Option<String> {
    let query = search.strip_prefix('?').unwrap_or(search);
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "open" && !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

async fn api_get(path: &str) -> Result<(u16, serde_json::Value), String> {
    let req = gloo_net::http::Request::get(path).cache(web_sys::RequestCache::NoStore);
    let resp = match req.send().await {
        Ok(r) => r,
        Err(_) => return Err("Немає зв'язку із сервером".to_string()),
    };
    let status = resp.status();
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Err("Сервер не відповів".to_string()),
    };
    Ok((status, json))
}

async fn api_post(path: &str, params: &[(&str, &str)]) -> Result<(u16, serde_json::Value), String> {
    let mut body = String::new();
    for (i, (k, v)) in params.iter().enumerate() {
        if i > 0 {
            body.push('&');
        }
        body.push_str(&url_encode(k));
        body.push('=');
        body.push_str(&url_encode(v));
    }

    let req = gloo_net::http::Request::post(path)
        .header("Content-Type", "application/x-www-form-urlencoded");
    let req_with_body = match req.body(body) {
        Ok(r) => r,
        Err(e) => return Err(e.to_string()),
    };
    let resp = match req_with_body.send().await {
        Ok(r) => r,
        Err(_) => return Err("Немає зв'язку із сервером".to_string()),
    };
    let status = resp.status();
    let json: serde_json::Value = match resp.json().await {
        Ok(j) => j,
        Err(_) => return Err("Сервер не відповів".to_string()),
    };
    Ok((status, json))
}

#[component]
fn App() -> impl IntoView {
    let dark = RwSignal::new(true);
    let screen = RwSignal::new(Screen::Login);
    let opening = RwSignal::new(None::<String>);

    // Device panel selection & iframe
    let selected_id = RwSignal::new(None::<String>);
    let panel_online = RwSignal::new(false);
    let iframe_src = RwSignal::new("about:blank".to_string());
    let iframe_ref = NodeRef::<html::Iframe>::new();
    let last_view = StoredValue::new(None::<String>);
    // When this admin last switched the panel screen (ms since epoch). A device
    // list fetched before that switch reached the server still carries the old
    // screen, so for a short while the server's view is not applied.
    let local_view_at = StoredValue::new(0.0_f64);

    // Responsive / side panel state
    let is_narrow = RwSignal::new(false);
    let side_collapsed = RwSignal::new(false);

    // Login state
    let password = RwSignal::new(String::new());
    let captcha = RwSignal::new(String::new());
    let captcha_id = RwSignal::new(String::new());
    let login_error = RwSignal::new(None::<String>);

    // Devices state
    let devices = RwSignal::new(Vec::<DeviceItem>::new());
    let device_messages = RwSignal::new(HashMap::<String, (Tone, String)>::new());

    // Forget confirmation dialog
    let forget_open = RwSignal::new(false);
    let forget_target = RwSignal::new(None::<DeviceItem>);

    // Check query string for ?open={id}
    if let Some(w) = web_sys::window() {
        if let Ok(search) = w.location().search() {
            if let Some(id) = parse_query_open(&search) {
                opening.set(Some(id));
                if let Ok(history) = w.history() {
                    let _ = history.replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some("/admin"),
                    );
                }
            }
        }
        // Detect dark mode preference
        if let Ok(Some(media)) = w.match_media("(prefers-color-scheme: dark)") {
            dark.set(media.matches());
        }
    }

    // Window resize listener to detect narrow screens
    let check_narrow = move || {
        if let Some(w) = web_sys::window() {
            if let Ok(inner_w) = w.inner_width() {
                if let Some(w_num) = inner_w.as_f64() {
                    let narrow = w_num < 600.0;
                    let was_narrow = is_narrow.get_untracked();
                    is_narrow.set(narrow);
                    if narrow && !was_narrow && selected_id.get_untracked().is_some() {
                        side_collapsed.set(true);
                    } else if !narrow && was_narrow {
                        side_collapsed.set(false);
                    }
                }
            }
        }
    };
    check_narrow();
    let resize_handle = window_event_listener(ev::resize, move |_| check_narrow());
    on_cleanup(move || resize_handle.remove());

    // Refresh captcha
    let fetch_captcha = move || {
        spawn_local(async move {
            match api_get("/admin/api/captcha").await {
                Ok((200, json)) => {
                    if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
                        captcha_id.set(id.to_string());
                        captcha.set(String::new());
                    }
                }
                Ok((_, json)) => {
                    let msg = json
                        .get("error")
                        .and_then(|e| e.as_str())
                        .unwrap_or("Сервер не відповів");
                    login_error.set(Some(msg.to_string()));
                }
                Err(err) => {
                    login_error.set(Some(err));
                }
            }
        });
    };

    let show_login = Callback::new(move |()| {
        screen.set(Screen::Login);
        fetch_captcha();
    });

    // Selecting a device: POST /admin/devices/{id}/open
    let select_device = Callback::new(move |target_id: String| {
        if selected_id.get_untracked().as_deref() == Some(&target_id)
            && panel_online.get_untracked()
        {
            if is_narrow.get_untracked() {
                side_collapsed.set(true);
            }
            return;
        }

        selected_id.set(Some(target_id.clone()));
        panel_online.set(false);
        if is_narrow.get_untracked() {
            side_collapsed.set(true);
        }

        // Before switching, set the panel iframe to about:blank first:
        iframe_src.set("about:blank".to_string());
        if let Some(iframe) = iframe_ref.get_untracked() {
            iframe.set_src("about:blank");
        }
        last_view.set_value(None);

        let dev_id = target_id.clone();
        spawn_local(async move {
            device_messages.update(|m| {
                m.remove(&dev_id);
            });
            match api_post(&format!("/admin/devices/{dev_id}/open"), &[]).await {
                Ok((200, json)) => {
                    let online = json
                        .get("online")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if online {
                        panel_online.set(true);
                        iframe_src.set("/".to_string());
                        if let Some(iframe) = iframe_ref.get_untracked() {
                            iframe.set_src("/");
                        }
                    } else {
                        panel_online.set(false);
                        devices.update(|list| {
                            if let Some(d) = list.iter_mut().find(|dev| dev.id == dev_id) {
                                d.requested = true;
                            }
                        });
                    }
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login.run(());
                    }
                }
                Ok((_, json)) => {
                    let err = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Помилка");
                    device_messages.update(|m| {
                        m.insert(dev_id, (Tone::Error, err.to_string()));
                    });
                }
                Err(e) => {
                    device_messages.update(|m| {
                        m.insert(dev_id, (Tone::Error, e));
                    });
                }
            }
        });
    });

    // Refresh device list
    let refresh_devices = Callback::new(move |()| async move {
        match api_get("/admin/api/devices").await {
            Ok((200, json)) => {
                let parsed: Result<Vec<DeviceItem>, _> = serde_json::from_value(json);
                if let Ok(mut list) = parsed {
                    sort_devices(&mut list);

                    // Check if initial opening requested device was specified
                    if let Some(target_id) = opening.get_untracked() {
                        opening.set(None);
                        select_device.run(target_id);
                    } else if let Some(cur_id) = selected_id.get_untracked() {
                        if let Some(dev) = list.iter().find(|d| d.id == cur_id) {
                            // Did device state change?
                            if dev.online && !panel_online.get_untracked() {
                                panel_online.set(true);
                                iframe_src.set("/".to_string());
                                if let Some(iframe) = iframe_ref.get_untracked() {
                                    iframe.set_src("/");
                                }
                            } else if !dev.online && panel_online.get_untracked() {
                                panel_online.set(false);
                                iframe_src.set("about:blank".to_string());
                                if let Some(iframe) = iframe_ref.get_untracked() {
                                    iframe.set_src("about:blank");
                                }
                            }

                            // View synchronization:
                            // When selected device's view differs from last view, call original show(view)
                            if panel_online.get_untracked() {
                                if let Some(ref server_view) = dev.view {
                                    let current_last = last_view.get_value();
                                    let settled = js_sys::Date::now() - local_view_at.get_value()
                                        > LOCAL_VIEW_SETTLE_MS;
                                    if settled
                                        && current_last.as_deref() != Some(server_view.as_str())
                                    {
                                        last_view.set_value(Some(server_view.clone()));
                                        if let Some(iframe) = iframe_ref.get_untracked() {
                                            call_original_show(&iframe, server_view);
                                        }
                                    }
                                }
                            }
                        }
                    }

                    devices.set(list);
                }
            }
            Ok((401, json)) if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) => {
                show_login.run(());
            }
            _ => {}
        }
    });

    // Startup check
    {
        spawn_local(async move {
            match api_get("/admin/api/devices").await {
                Ok((200, json)) => {
                    if let Ok(mut list) = serde_json::from_value::<Vec<DeviceItem>>(json) {
                        sort_devices(&mut list);
                        devices.set(list);
                        screen.set(Screen::Devices);
                        refresh_devices.run(()).await;
                    } else {
                        show_login.run(());
                    }
                }
                _ => {
                    show_login.run(());
                }
            }
        });
    }

    // Polling effect: poll every 2s while device selected, 5s otherwise
    Effect::new(move |_| {
        if screen.get() == Screen::Devices {
            spawn_local(async move {
                loop {
                    let poll_ms = if selected_id.get_untracked().is_some() {
                        POLL_FAST_MS
                    } else {
                        POLL_SLOW_MS
                    };
                    TimeoutFuture::new(poll_ms).await;
                    if screen.get_untracked() != Screen::Devices {
                        break;
                    }
                    refresh_devices.run(()).await;
                }
            });
        }
    });

    // Wrapped iframe on_load handler for shared view synchronization
    let on_iframe_load = Callback::new(move |()| {
        let Some(iframe) = iframe_ref.get_untracked() else {
            return;
        };
        let Some(cw) = iframe.content_window() else {
            return;
        };

        if let Ok(href) = cw.location().href() {
            if href == "about:blank" {
                return;
            }
        }
        if let Ok(pathname) = cw.location().pathname() {
            if pathname.starts_with("/admin") {
                let _ = cw.location().replace("about:blank");
                panel_online.set(false);
                iframe_src.set("about:blank".to_string());
                return;
            }
        }

        let Ok(show_val) = js_sys::Reflect::get(&cw, &JsValue::from_str("show")) else {
            return;
        };
        if !show_val.is_function() {
            return;
        }

        let orig_show: js_sys::Function = if let Ok(existing_orig) =
            js_sys::Reflect::get(&cw, &JsValue::from_str("__orig_show"))
        {
            if existing_orig.is_function() {
                existing_orig.unchecked_into()
            } else {
                let _ = js_sys::Reflect::set(&cw, &JsValue::from_str("__orig_show"), &show_val);
                show_val.unchecked_into()
            }
        } else {
            let _ = js_sys::Reflect::set(&cw, &JsValue::from_str("__orig_show"), &show_val);
            show_val.unchecked_into()
        };

        // A newly opened panel jumps to the server's view right after load:
        if let Some(ref cur_id) = selected_id.get_untracked() {
            let server_view_opt = devices.with_untracked(|list| {
                list.iter()
                    .find(|d| d.id == *cur_id)
                    .and_then(|d| d.view.clone())
            });
            if let Some(ref sv) = server_view_opt {
                let arg = JsValue::from_str(sv);
                let _ = orig_show.call1(&cw, &arg);
                last_view.set_value(Some(sv.clone()));
            }
        }

        let cw_clone = cw.clone();
        let orig_clone = orig_show.clone();

        let closure = Closure::wrap(Box::new(move |arg: JsValue| {
            let _ = orig_clone.call1(&cw_clone, &arg);
            if let Some(name) = arg.as_string() {
                last_view.set_value(Some(name.clone()));
                local_view_at.set_value(js_sys::Date::now());
                if let Some(id) = selected_id.get_untracked() {
                    spawn_local(async move {
                        let _ = api_post(&format!("/admin/devices/{id}/view"), &[("view", &name)])
                            .await;
                    });
                }
            }
        }) as Box<dyn FnMut(JsValue)>);

        let _ = js_sys::Reflect::set(
            &cw,
            &JsValue::from_str("show"),
            closure.as_ref().unchecked_ref(),
        );
        closure.forget();
    });

    // Login action
    let login_action = AsyncAction::new(move |()| {
        let pass = password.get_untracked();
        let cid = captcha_id.get_untracked();
        let cap = captcha.get_untracked();

        async move {
            login_error.set(None);
            let params = [
                ("password", pass.as_str()),
                ("captcha_id", cid.as_str()),
                ("captcha", cap.as_str()),
            ];
            match api_post("/admin/login", &params).await {
                Ok((200, _)) => {
                    password.set(String::new());
                    screen.set(Screen::Devices);
                    refresh_devices.run(()).await;
                    ActionResult::Success
                }
                Ok((_, json)) => {
                    fetch_captcha();
                    let msg = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Невірний пароль або код перевірки");
                    login_error.set(Some(msg.to_string()));
                    ActionResult::Error(Some(msg.to_string()))
                }
                Err(err) => {
                    fetch_captcha();
                    login_error.set(Some(err.clone()));
                    ActionResult::Error(Some(err))
                }
            }
        }
    });

    // Subtitle based on screen
    let subtitle = Signal::derive(move || match screen.get() {
        Screen::Login => "Вхід адміністратора".to_string(),
        Screen::Devices => {
            let s = if selected_id.get().is_some() {
                POLL_FAST_MS / 1000
            } else {
                POLL_SLOW_MS / 1000
            };
            format!("Пристрої · оновлюється кожні {s} с")
        }
    });

    // Device actions
    let on_set_waiting = move |dev_id: String, target: bool| {
        let id_val = dev_id.clone();
        async move {
            device_messages.update(|m| {
                m.remove(&id_val);
            });
            let on_val = if target { "1" } else { "" };
            match api_post(
                &format!("/admin/devices/{id_val}/waiting"),
                &[("on", on_val)],
            )
            .await
            {
                Ok((200, _)) => {
                    devices.update(|list| {
                        if let Some(dev) = list.iter_mut().find(|d| d.id == id_val) {
                            dev.waiting = target;
                            dev.requested &= target;
                        }
                    });
                    if !target && opening.get_untracked().as_deref() == Some(id_val.as_str()) {
                        opening.set(None);
                    }
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login.run(());
                    }
                    ActionResult::Error(Some("Потрібно увійти знову".to_string()))
                }
                Ok((_, json)) => {
                    let err = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Помилка");
                    device_messages.update(|m| {
                        m.insert(id_val, (Tone::Error, err.to_string()));
                    });
                    ActionResult::Error(Some(err.to_string()))
                }
                Err(e) => {
                    device_messages.update(|m| {
                        m.insert(id_val, (Tone::Error, e.clone()));
                    });
                    ActionResult::Error(Some(e))
                }
            }
        }
    };

    let on_end_session = move |dev_id: String| {
        let id_val = dev_id.clone();
        async move {
            device_messages.update(|m| {
                m.remove(&id_val);
            });
            match api_post(&format!("/admin/devices/{id_val}/end"), &[]).await {
                Ok((200, _)) => {
                    if selected_id.get_untracked() == Some(id_val.clone()) {
                        panel_online.set(false);
                        iframe_src.set("about:blank".to_string());
                        if let Some(iframe) = iframe_ref.get_untracked() {
                            iframe.set_src("about:blank");
                        }
                    }
                    refresh_devices.run(()).await;
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login.run(());
                    }
                    ActionResult::Error(Some("Потрібно увійти знову".to_string()))
                }
                Ok((_, json)) => {
                    let err = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Помилка");
                    device_messages.update(|m| {
                        m.insert(id_val, (Tone::Error, err.to_string()));
                    });
                    ActionResult::Error(Some(err.to_string()))
                }
                Err(e) => {
                    device_messages.update(|m| {
                        m.insert(id_val, (Tone::Error, e.clone()));
                    });
                    ActionResult::Error(Some(e))
                }
            }
        }
    };

    let on_forget_device = move |dev_id: String| {
        let id_val = dev_id.clone();
        async move {
            match api_post(&format!("/admin/devices/{id_val}/forget"), &[]).await {
                Ok((200, _)) => {
                    if selected_id.get_untracked() == Some(id_val.clone()) {
                        selected_id.set(None);
                        panel_online.set(false);
                        iframe_src.set("about:blank".to_string());
                        if let Some(iframe) = iframe_ref.get_untracked() {
                            iframe.set_src("about:blank");
                        }
                    }
                    refresh_devices.run(()).await;
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login.run(());
                    }
                    ActionResult::Error(Some("Потрібно увійти знову".to_string()))
                }
                Ok((_, json)) => {
                    let err = json
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Помилка");
                    ActionResult::Error(Some(err.to_string()))
                }
                Err(e) => ActionResult::Error(Some(e)),
            }
        }
    };

    let forget_dialog_message = Signal::derive(move || {
        if let Some(ref target) = forget_target.get() {
            format!(
                "«{}» зникне зі списку. Якщо пристрій ще працює, він з'явиться знову на наступній перевірці.",
                target.display_name()
            )
        } else {
            String::new()
        }
    });

    view! {
        <ThemeRoot dark=dark>
            <div class="admin-app">
                <Header
                    title=Signal::stored("Віддалена підтримка".to_string())
                    subtitle=subtitle
                    actions=ViewFn::from(move || view! {
                        <ThemeToggle dark=dark on_toggle=Callback::new(move |d| dark.set(d))/>
                    })
                />

                {move || match screen.get() {
                    Screen::Login => view! {
                        <main class="admin-main">
                            <div class="admin-card">
                                <h2 class="op-t-section-title">"Вхід"</h2>
                                <label class="admin-label">"Пароль"</label>
                                <TextField
                                    value=password
                                    placeholder=Signal::stored("Пароль".to_string())
                                    password=true
                                    show_reveal=true
                                    label="Пароль"
                                />

                                <label class="admin-label">"Цифри з картинки"</label>
                                <div class="admin-captcha-row">
                                    <img
                                        class="admin-captcha-img"
                                        src=move || {
                                            let id = captcha_id.get();
                                            if id.is_empty() { String::new() } else { format!("/admin/captcha/{id}") }
                                        }
                                        alt="Цифри для перевірки"
                                        width="200"
                                        height="64"
                                    />
                                    <ActionButton
                                        text=Signal::stored("Інші цифри".to_string())
                                        on_click=AsyncAction::new(move |()| {
                                            fetch_captcha();
                                            async move { ActionResult::Success }
                                        })
                                    />
                                </div>
                                <TextField
                                    value=captcha
                                    placeholder=Signal::stored("Цифри з картинки".to_string())
                                    label="Цифри з картинки"
                                />

                                <div class="admin-actions">
                                    <ActionButton
                                        text=Signal::stored("Увійти".to_string())
                                        keep_flash_icon_on_success=true
                                        on_click=login_action.clone()
                                    />
                                </div>

                                {move || login_error.get().map(|msg| view! {
                                    <div class="admin-card-msg">
                                        <Banner tone=Tone::Error text=Signal::stored(msg)/>
                                    </div>
                                })}
                            </div>
                        </main>
                    }.into_any(),

                    Screen::Devices => {
                        view! {
                            <div
                                class="admin-workspace"
                                data-narrow=move || is_narrow.get().then_some("")
                            >
                                <SidePanel
                                    title=Signal::stored("Пристрої".to_string())
                                    collapsed=side_collapsed
                                    width=85.0
                                >
                                    {move || {
                                        let dev_list = devices.get();
                                        if dev_list.is_empty() {
                                            view! {
                                                <div class="admin-devices-empty">
                                                    <EmptyState
                                                        title="Пристроїв ще немає".to_string()
                                                        description="Кожен з'явиться тут після першої перевірки сервера.".to_string()
                                                        compact=true
                                                    />
                                                </div>
                                            }.into_any()
                                        } else {
                                            let cards = dev_list.into_iter().map(|dev| {
                                                let id_for_wait = dev.id.clone();
                                                let id_for_end = dev.id.clone();
                                                let id_for_card = dev.id.clone();
                                                let id_for_header = dev.id.clone();
                                                let dev_id_sel = dev.id.clone();
                                                let dev_for_forget = dev.clone();
                                                let dev_id_key = dev.id.clone();
                                                let msg = device_messages.get().get(&dev_id_key).cloned();
                                                let is_selected = Signal::derive(move || selected_id.get().as_deref() == Some(&dev_id_sel));

                                                let (badge_status, badge_text) = match dev.status.as_str() {
                                                    "session" => (PresenceStatus::Online, "з'єднано"),
                                                    "alive" => (PresenceStatus::Online, "на зв'язку"),
                                                    _ => (PresenceStatus::Offline, "не на зв'язку"),
                                                };

                                                view! {
                                                    <div
                                                        class="admin-dev-card"
                                                        data-selected=move || is_selected.get().then_some("")
                                                        on:click=move |ev| {
                                                            if let Some(target) = ev.target().and_then(|t| t.dyn_into::<web_sys::Element>().ok()) {
                                                                if target.closest("button, input, [role=switch], [role=button]").ok().flatten().is_some() {
                                                                    return;
                                                                }
                                                            }
                                                            select_device.run(id_for_card.clone());
                                                        }
                                                    >
                                                        <div
                                                            class="admin-devhead"
                                                            on:click=move |_| {
                                                                select_device.run(id_for_header.clone());
                                                            }
                                                        >
                                                            <div class="admin-devhead-row">
                                                                <span class="admin-devhead-title">{dev.display_name().to_string()}</span>
                                                                <div class="admin-devhead-badges">
                                                                    <StatusBadge
                                                                        status=Signal::stored(badge_status)
                                                                        text=Signal::stored(badge_text.to_string())
                                                                    />
                                                                    {dev.requested.then(|| view! {
                                                                        <StatusBadge
                                                                            status=Signal::stored(PresenceStatus::Pending)
                                                                            text=Signal::stored("запитано".to_string())
                                                                        />
                                                                    })}
                                                                </div>
                                                            </div>
                                                            <span class="admin-devhead-meta">{dev.display_meta()}</span>
                                                        </div>

                                                        <ListItem
                                                            title=Signal::stored("Чекаю пристрій".to_string())
                                                            subtitle=Signal::stored("перевірка кожні 2 хв замість щогодини".to_string())
                                                            trailing=ViewFn::from(move || {
                                                                let wait_id = id_for_wait.clone();
                                                                view! {
                                                                    <AsyncSwitch
                                                                        checked=Signal::stored(dev.waiting)
                                                                        on_toggle=AsyncAction::new(move |target: bool| {
                                                                            let wid = wait_id.clone();
                                                                            async move { on_set_waiting(wid, target).await }
                                                                        })
                                                                        label="Чекаю пристрій"
                                                                    />
                                                                }
                                                            })
                                                        />

                                                        <div class="admin-actions">
                                                            {dev.online.then(|| {
                                                                let eid = id_for_end.clone();
                                                                view! {
                                                                    <ActionButton
                                                                        text=Signal::stored("Завершити сесію".to_string())
                                                                        on_click=AsyncAction::new(move |()| {
                                                                            let eid = eid.clone();
                                                                            async move { on_end_session(eid).await }
                                                                        })
                                                                    />
                                                                }
                                                            })}
                                                            <ActionButton
                                                                text=Signal::stored("Забути".to_string())
                                                                dangerous=true
                                                                on_click=AsyncAction::new(move |()| {
                                                                    let target = dev_for_forget.clone();
                                                                    async move {
                                                                        forget_target.set(Some(target));
                                                                        forget_open.set(true);
                                                                        ActionResult::Success
                                                                    }
                                                                })
                                                            />
                                                        </div>

                                                        {msg.map(|(tone, text)| {
                                                            view! {
                                                                <div class="admin-card-msg">
                                                                    <Banner tone=tone text=Signal::stored(text)/>
                                                                </div>
                                                            }
                                                        })}
                                                    </div>
                                                }
                                            }).collect_view();

                                            view! {
                                                <div class="admin-device-list">
                                                    {cards}
                                                </div>
                                            }.into_any()
                                        }
                                    }}
                                </SidePanel>

                                <div class="admin-panel-container">
                                    {move || {
                                        let cur_id = selected_id.get();
                                        let is_online = panel_online.get();

                                        match cur_id {
                                            None => view! {
                                                <div class="admin-waiting-area">
                                                    <EmptyState
                                                        title="Оберіть пристрій".to_string()
                                                        description="Виберіть пристрій зі списку зліва, щоб відкрити його панель.".to_string()
                                                        compact=false
                                                    />
                                                </div>
                                            }.into_any(),

                                            Some(_) if !is_online => view! {
                                                <div class="admin-waiting-area">
                                                    <EmptyState
                                                        title="Сесію запитано".to_string()
                                                        description="Пристрій відкриє її на найближчій перевірці: за кілька хвилин, якщо ввімкнено «Чекаю пристрій», інакше до години.".to_string()
                                                        compact=false
                                                    />
                                                </div>
                                            }.into_any(),

                                            Some(_) => {
                                                view! {
                                                    <iframe
                                                        node_ref=iframe_ref
                                                        class="admin-iframe"
                                                        src=iframe_src
                                                        on:load=move |_| on_iframe_load.run(())
                                                    ></iframe>
                                                }.into_any()
                                            }
                                        }
                                    }}
                                </div>
                            </div>
                        }.into_any()
                    }
                }}

                <ConfirmDialog
                    open=forget_open
                    title=Signal::stored("Забути пристрій?".to_string())
                    message=forget_dialog_message
                    confirm_text=Signal::stored("Забути".to_string())
                    cancel_text=Signal::stored("Скасувати".to_string())
                    destructive=true
                    on_confirm=AsyncAction::new(move |()| async move {
                        if let Some(target) = forget_target.get_untracked() {
                            on_forget_device(target.id).await
                        } else {
                            ActionResult::Success
                        }
                    })
                />
            </div>
        </ThemeRoot>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(App);
}
