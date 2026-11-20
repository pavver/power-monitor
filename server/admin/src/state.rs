//! What the admin page knows and does: one `Copy` handle on its signals.

use std::collections::HashMap;

use gloo_timers::future::TimeoutFuture;
use leptos::ev;
use leptos::html;
use leptos::prelude::*;
use leptos::task::spawn_local;
use operator_ui::prelude::Tone;

use crate::api::{self, Reply};
use crate::model::{follows, opened_device, sort_devices, DeviceItem};
use crate::panel;

pub const POLL_SLOW_MS: u32 = 5000;
pub const POLL_FAST_MS: u32 = 2000;
const BLANK: &str = "about:blank";
/// Narrower than this, the device list folds away while a panel is open.
const NARROW_PX: f64 = 600.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Login,
    Devices,
}

#[derive(Clone, Copy)]
pub struct Admin {
    pub dark: RwSignal<bool>,
    pub screen: RwSignal<Screen>,
    /// The device to open once the list is in, from `?open=…`.
    pub opening: RwSignal<Option<String>>,

    pub selected_id: RwSignal<Option<String>>,
    pub panel_online: RwSignal<bool>,
    pub iframe_src: RwSignal<String>,
    pub iframe_ref: NodeRef<html::Iframe>,
    /// The panel's own screen.
    pub last_view: StoredValue<Option<String>>,
    /// The shared screen the server reported last: the panel follows only
    /// when it changes.
    pub server_view: StoredValue<Option<String>>,
    /// Counts the panel's own screen switches: a list asked for before one
    /// carries the screen from before it, so it is not followed.
    pub local_moves: StoredValue<u32>,

    pub is_narrow: RwSignal<bool>,
    pub side_collapsed: RwSignal<bool>,

    pub password: RwSignal<String>,
    pub captcha: RwSignal<String>,
    pub captcha_id: RwSignal<String>,
    pub login_error: RwSignal<Option<String>>,

    pub devices: RwSignal<Vec<DeviceItem>>,
    pub device_messages: RwSignal<HashMap<String, (Tone, String)>>,

    pub forget_open: RwSignal<bool>,
    pub forget_target: RwSignal<Option<DeviceItem>>,
    pub accept_open: RwSignal<bool>,
    pub accept_target: RwSignal<Option<DeviceItem>>,
}

impl Admin {
    /// The page's state, with the device from `?open=…` (taken off the
    /// address) and the system's colour scheme.
    pub fn new() -> Self {
        let admin = Self {
            dark: RwSignal::new(true),
            screen: RwSignal::new(Screen::Login),
            opening: RwSignal::new(None),
            selected_id: RwSignal::new(None),
            panel_online: RwSignal::new(false),
            iframe_src: RwSignal::new(BLANK.to_string()),
            iframe_ref: NodeRef::new(),
            last_view: StoredValue::new(None),
            server_view: StoredValue::new(None),
            local_moves: StoredValue::new(0),
            is_narrow: RwSignal::new(false),
            side_collapsed: RwSignal::new(false),
            password: RwSignal::new(String::new()),
            captcha: RwSignal::new(String::new()),
            captcha_id: RwSignal::new(String::new()),
            login_error: RwSignal::new(None),
            devices: RwSignal::new(Vec::new()),
            device_messages: RwSignal::new(HashMap::new()),
            forget_open: RwSignal::new(false),
            forget_target: RwSignal::new(None),
            accept_open: RwSignal::new(false),
            accept_target: RwSignal::new(None),
        };
        if let Some(window) = web_sys::window() {
            if let Some(id) = window
                .location()
                .search()
                .ok()
                .as_deref()
                .and_then(opened_device)
            {
                admin.opening.set(Some(id));
                if let Ok(history) = window.history() {
                    let _ = history.replace_state_with_url(
                        &wasm_bindgen::JsValue::NULL,
                        "",
                        Some("/admin"),
                    );
                }
            }
            if let Ok(Some(media)) = window.match_media("(prefers-color-scheme: dark)") {
                admin.dark.set(media.matches());
            }
        }
        admin
    }

    /// Follows the window's width for as long as the calling component lives.
    pub fn watch_width(self) {
        self.check_narrow();
        let handle = window_event_listener(ev::resize, move |_| self.check_narrow());
        on_cleanup(move || handle.remove());
    }

    fn check_narrow(self) {
        let Some(width) = web_sys::window()
            .and_then(|w| w.inner_width().ok())
            .and_then(|w| w.as_f64())
        else {
            return;
        };
        let narrow = width < NARROW_PX;
        let was_narrow = self.is_narrow.get_untracked();
        self.is_narrow.set(narrow);
        if narrow && !was_narrow && self.selected_id.get_untracked().is_some() {
            self.side_collapsed.set(true);
        } else if !narrow && was_narrow {
            self.side_collapsed.set(false);
        }
    }

    fn collapse_if_narrow(self) {
        if self.is_narrow.get_untracked() {
            self.side_collapsed.set(true);
        }
    }

    fn set_frame(self, src: &str) {
        self.iframe_src.set(src.to_string());
        if let Some(iframe) = self.iframe_ref.get_untracked() {
            iframe.set_src(src);
        }
    }

    fn open_panel(self) {
        self.panel_online.set(true);
        self.set_frame("/");
    }

    pub fn close_panel(self) {
        self.panel_online.set(false);
        self.set_frame(BLANK);
    }

    pub(crate) fn show_message(self, id: &str, message: Option<(Tone, String)>) {
        self.device_messages.update(|messages| match message {
            Some(message) => {
                messages.insert(id.to_string(), message);
            }
            None => {
                messages.remove(id);
            }
        });
    }

    pub fn show_login(self) {
        self.screen.set(Screen::Login);
        self.fetch_captcha();
    }

    pub fn fetch_captcha(self) {
        spawn_local(async move {
            match api::get("/admin/api/captcha").await {
                Reply::Ok(body) => {
                    if let Some(id) = body.get("id").and_then(|v| v.as_str()) {
                        self.captcha_id.set(id.to_string());
                        self.captcha.set(String::new());
                    }
                }
                Reply::Login => self.login_error.set(Some("Сервер не відповів".to_string())),
                Reply::Failed(error) => self
                    .login_error
                    .set(Some(error.unwrap_or_else(|| "Сервер не відповів".into()))),
            }
        });
    }

    /// On start: the device list if this browser is logged in, else the login.
    pub fn start(self) {
        spawn_local(async move {
            let list = match api::get("/admin/api/devices").await {
                Reply::Ok(body) => serde_json::from_value::<Vec<DeviceItem>>(body).ok(),
                _ => None,
            };
            let Some(mut list) = list else {
                self.show_login();
                return;
            };
            sort_devices(&mut list);
            self.devices.set(list);
            self.screen.set(Screen::Devices);
            self.refresh().await;
        });
    }

    /// Refreshes the list every 2 s while a device is picked, else every 5 s,
    /// for as long as the device screen is shown.
    pub fn poll(self) {
        Effect::new(move |_| {
            if self.screen.get() == Screen::Devices {
                spawn_local(async move {
                    loop {
                        let every = if self.selected_id.get_untracked().is_some() {
                            POLL_FAST_MS
                        } else {
                            POLL_SLOW_MS
                        };
                        TimeoutFuture::new(every).await;
                        if self.screen.get_untracked() != Screen::Devices {
                            break;
                        }
                        self.refresh().await;
                    }
                });
            }
        });
    }

    pub async fn refresh(self) {
        let moves = self.local_moves.get_value();
        match api::get("/admin/api/devices").await {
            Reply::Ok(body) => {
                let Ok(mut list) = serde_json::from_value::<Vec<DeviceItem>>(body) else {
                    return;
                };
                sort_devices(&mut list);
                if let Some(id) = self.opening.get_untracked() {
                    self.opening.set(None);
                    self.select(id);
                } else if let Some(device) = self
                    .selected_id
                    .get_untracked()
                    .and_then(|id| list.iter().find(|d| d.id == id))
                {
                    if self.local_moves.get_value() == moves {
                        self.follow(device);
                    }
                }
                self.devices.set(list);
            }
            Reply::Login => self.show_login(),
            Reply::Failed(_) => {}
        }
    }

    /// Brings the open panel in line with what the server says of `device`.
    fn follow(self, device: &DeviceItem) {
        let online = self.panel_online.get_untracked();
        if device.online && !online {
            self.open_panel();
        } else if !device.online && online {
            self.close_panel();
        }
        if !self.panel_online.get_untracked() {
            return;
        }
        let Some(view) = &device.view else { return };
        let seen = self.server_view.get_value();
        self.server_view.set_value(Some(view.clone()));
        if follows(seen.as_deref(), self.last_view.get_value().as_deref(), view) {
            self.last_view.set_value(Some(view.clone()));
            if let Some(iframe) = self.iframe_ref.get_untracked() {
                panel::show(&iframe, view);
            }
        }
    }

    /// Picks the device and opens its panel, asking for a session if needed.
    pub fn select(self, id: String) {
        if self.selected_id.get_untracked().as_deref() == Some(&id)
            && self.panel_online.get_untracked()
        {
            self.collapse_if_narrow();
            return;
        }
        self.selected_id.set(Some(id.clone()));
        self.collapse_if_narrow();
        self.close_panel();
        self.last_view.set_value(None);
        self.server_view.set_value(None);
        spawn_local(async move {
            self.show_message(&id, None);
            match api::post(&format!("/admin/devices/{id}/open"), &[]).await {
                Reply::Ok(body) if body.get("online").and_then(|v| v.as_bool()) == Some(true) => {
                    self.open_panel()
                }
                Reply::Ok(_) => {
                    self.panel_online.set(false);
                    self.devices.update(|list| {
                        if let Some(device) = list.iter_mut().find(|d| d.id == id) {
                            device.requested = true;
                        }
                    });
                }
                Reply::Login => self.show_login(),
                Reply::Failed(error) => self.show_message(
                    &id,
                    Some((Tone::Error, error.unwrap_or_else(|| "Помилка".into()))),
                ),
            }
        });
    }
}
