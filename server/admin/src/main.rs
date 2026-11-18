#![deny(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashMap;

use gloo_timers::future::TimeoutFuture;
use leptos::prelude::*;
use leptos::task::spawn_local;
use operator_ui::prelude::*;
use serde::{Deserialize, Serialize};

const POLL_MS: u32 = 5000;

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
        if self.version.is_empty() {
            self.id.clone()
        } else {
            format!("{} · v{}", self.id, self.version)
        }
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

    let show_login = move || {
        screen.set(Screen::Login);
        fetch_captcha();
    };

    // Refresh device list
    let refresh_devices = move || async move {
        match api_get("/admin/api/devices").await {
            Ok((200, json)) => {
                let parsed: Result<Vec<DeviceItem>, _> = serde_json::from_value(json);
                if let Ok(list) = parsed {
                    // Check if requested open device is online
                    if let Some(ref target_id) = opening.get_untracked() {
                        if let Some(d) = list.iter().find(|dev| dev.id == *target_id) {
                            if d.online {
                                if let Some(w) = web_sys::window() {
                                    let _ = w.location().set_href("/");
                                }
                            }
                        }
                    }
                    devices.set(list);
                }
            }
            Ok((401, json)) if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) => {
                show_login();
            }
            _ => {}
        }
    };

    // Startup check
    {
        let refresh = refresh_devices;
        spawn_local(async move {
            match api_get("/admin/api/devices").await {
                Ok((200, json)) => {
                    if let Ok(list) = serde_json::from_value::<Vec<DeviceItem>>(json) {
                        devices.set(list);
                        screen.set(Screen::Devices);
                        refresh().await;
                    } else {
                        show_login();
                    }
                }
                _ => {
                    show_login();
                }
            }
        });
    }

    // Polling effect: poll every 5s while on Screen::Devices
    Effect::new(move |_| {
        if screen.get() == Screen::Devices {
            let refresh = refresh_devices;
            spawn_local(async move {
                loop {
                    TimeoutFuture::new(POLL_MS).await;
                    if screen.get_untracked() != Screen::Devices {
                        break;
                    }
                    refresh().await;
                }
            });
        }
    });

    // Login action
    let login_action = AsyncAction::new(move |()| {
        let pass = password.get_untracked();
        let cid = captcha_id.get_untracked();
        let cap = captcha.get_untracked();
        let refresh = refresh_devices;

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
                    refresh().await;
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
        Screen::Devices => format!("Пристрої · оновлюється кожні {} с", POLL_MS / 1000),
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
                        show_login();
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

    let on_open_panel = move |dev_id: String| {
        let id_val = dev_id.clone();
        async move {
            device_messages.update(|m| {
                m.remove(&id_val);
            });
            match api_post(&format!("/admin/devices/{id_val}/open"), &[]).await {
                Ok((200, json)) => {
                    let online = json
                        .get("online")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if online {
                        if let Some(w) = web_sys::window() {
                            let _ = w.location().set_href("/");
                        }
                    } else {
                        opening.set(Some(id_val.clone()));
                        devices.update(|list| {
                            if let Some(d) = list.iter_mut().find(|dev| dev.id == id_val) {
                                d.requested = true;
                            }
                        });
                    }
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login();
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
        let refresh = refresh_devices;
        async move {
            device_messages.update(|m| {
                m.remove(&id_val);
            });
            match api_post(&format!("/admin/devices/{id_val}/end"), &[]).await {
                Ok((200, _)) => {
                    if opening.get_untracked() == Some(id_val) {
                        opening.set(None);
                    }
                    refresh().await;
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login();
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
        let refresh = refresh_devices;
        async move {
            match api_post(&format!("/admin/devices/{id_val}/forget"), &[]).await {
                Ok((200, _)) => {
                    refresh().await;
                    ActionResult::Success
                }
                Ok((401, json)) => {
                    if json.get("login").and_then(|v| v.as_bool()).unwrap_or(false) {
                        show_login();
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

                <main class="admin-main">
                    {move || match screen.get() {
                        Screen::Login => view! {
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
                        }.into_any(),

                        Screen::Devices => {
                            let dev_list = devices.get();
                            view! {
                                <section>
                                    {if dev_list.is_empty() {
                                        view! {
                                            <div class="admin-card">
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
                                            let id_for_open = dev.id.clone();
                                            let id_for_end = dev.id.clone();
                                            let dev_for_forget = dev.clone();
                                            let dev_id_key = dev.id.clone();
                                            let is_waiting_for_open = opening.get() == Some(dev.id.clone()) && !dev.online;
                                            let msg = device_messages.get().get(&dev_id_key).cloned();

                                            view! {
                                                <div class="admin-card" style="margin-bottom: var(--op-dim-margin-screen);">
                                                    <div class="admin-devhead">
                                                        <StatusBadge status=Signal::stored(
                                                            if dev.online { PresenceStatus::Online } else { PresenceStatus::Offline }
                                                        )/>
                                                        <div class="admin-devhead-text">
                                                            <span class="admin-devhead-title">{dev.display_name().to_string()}</span>
                                                            <span class="admin-devhead-meta">{dev.display_meta()}</span>
                                                        </div>
                                                    </div>

                                                    <PropertyList>
                                                        <Property
                                                            label="На зв'язку".to_string()
                                                            value=Signal::stored(ago(dev.seen))
                                                        />
                                                        <Property label="Сесія".to_string()>
                                                            <span class=if dev.online { "admin-session-ok" } else if dev.requested { "admin-session-pending" } else { "" }>
                                                                {if dev.online { "відкрита" } else if dev.requested { "запитано" } else { "немає" }}
                                                            </span>
                                                        </Property>
                                                    </PropertyList>

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
                                                        <ActionButton
                                                            text=Signal::stored("Відкрити панель".to_string())
                                                            keep_flash_icon_on_success=true
                                                            on_click=AsyncAction::new(move |()| {
                                                                let oid = id_for_open.clone();
                                                                async move { on_open_panel(oid).await }
                                                            })
                                                        />
                                                        {dev.online.then(|| {
                                                            view! {
                                                                <ActionButton
                                                                    text=Signal::stored("Завершити сесію".to_string())
                                                                    on_click=AsyncAction::new(move |()| {
                                                                        let eid = id_for_end.clone();
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

                                                    {is_waiting_for_open.then(|| {
                                                        view! {
                                                            <div class="admin-card-msg">
                                                                <Banner
                                                                    tone=Tone::Warning
                                                                    text=Signal::stored("Сесію запитано. Пристрій відкриє її на найближчій перевірці: за кілька хвилин, якщо ввімкнено «Чекаю пристрій», інакше до години. Панель відкриється сама.".to_string())
                                                                />
                                                            </div>
                                                        }
                                                    })}

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

                                        cards.into_any()
                                    }}

                                    <div class="admin-card admin-card-alt">
                                        <p class="admin-muted admin-small">
                                            "Пристрій перевіряє сервер раз на годину, а з «Чекаю пристрій» кожні 2 хв. «Відкрити панель» запитує сесію й відкриває панель пристрою, щойно він з'явиться."
                                        </p>
                                    </div>
                                </section>
                            }.into_any()
                        }
                    }}
                </main>

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
