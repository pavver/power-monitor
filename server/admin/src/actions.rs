//! What the admin does: log in, and act on a device.

use leptos::prelude::*;
use operator_ui::prelude::{ActionResult, Tone};

use crate::api::{self, Reply};
use crate::state::{Admin, Screen};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Closing {
    Session,
    Device,
}

impl Admin {
    pub async fn login(self) -> ActionResult {
        let (password, captcha_id, captcha) = (
            self.password.get_untracked(),
            self.captcha_id.get_untracked(),
            self.captcha.get_untracked(),
        );
        self.login_error.set(None);
        let params = [
            ("password", password.as_str()),
            ("captcha_id", captcha_id.as_str()),
            ("captcha", captcha.as_str()),
        ];
        let error = match api::post("/admin/login", &params).await {
            Reply::Ok(_) => {
                self.password.set(String::new());
                self.screen.set(Screen::Devices);
                self.refresh().await;
                return ActionResult::Success;
            }
            Reply::Login | Reply::Failed(None) => "Невірний пароль або код перевірки".to_string(),
            Reply::Failed(Some(error)) => error,
        };
        self.fetch_captcha();
        self.login_error.set(Some(error.clone()));
        ActionResult::Error(Some(error))
    }

    /// Runs `action` on device `id`; a failure shows on its card if `on_card`.
    async fn act(
        self,
        id: &str,
        action: &str,
        params: &[(&str, &str)],
        on_card: bool,
    ) -> Result<(), String> {
        if on_card {
            self.show_message(id, None);
        }
        match api::post(&format!("/admin/devices/{id}/{action}"), params).await {
            Reply::Ok(_) => Ok(()),
            Reply::Login => {
                self.show_login();
                Err("Потрібно увійти знову".to_string())
            }
            Reply::Failed(error) => {
                let error = error.unwrap_or_else(|| "Помилка".to_string());
                if on_card {
                    self.show_message(id, Some((Tone::Error, error.clone())));
                }
                Err(error)
            }
        }
    }

    pub async fn set_waiting(self, id: String, on: bool) -> ActionResult {
        if let Err(error) = self
            .act(&id, "waiting", &[("on", if on { "1" } else { "" })], true)
            .await
        {
            return ActionResult::Error(Some(error));
        }
        self.devices.update(|list| {
            if let Some(device) = list.iter_mut().find(|d| d.id == id) {
                device.waiting = on;
                device.requested &= on;
            }
        });
        if !on && self.opening.get_untracked().as_deref() == Some(id.as_str()) {
            self.opening.set(None);
        }
        ActionResult::Success
    }

    pub async fn end_session(self, id: String) -> ActionResult {
        self.close(id, Closing::Session).await
    }

    pub async fn forget(self, id: String) -> ActionResult {
        self.close(id, Closing::Device).await
    }

    /// Takes (`accept`) or drops the new secret a device came back with.
    pub async fn settle_key(self, id: String, accept: bool) -> ActionResult {
        let action = if accept { "accept-key" } else { "reject-key" };
        if let Err(error) = self.act(&id, action, &[], true).await {
            return ActionResult::Error(Some(error));
        }
        self.refresh().await;
        ActionResult::Success
    }

    /// Ends the session or forgets the device: its panel, if open, goes
    /// too, and the list is fetched again.
    async fn close(self, id: String, closing: Closing) -> ActionResult {
        let (action, on_card) = match closing {
            Closing::Session => ("end", true),
            // The card goes with the device.
            Closing::Device => ("forget", false),
        };
        if let Err(error) = self.act(&id, action, &[], on_card).await {
            return ActionResult::Error(Some(error));
        }
        if self.selected_id.get_untracked() == Some(id) {
            if closing == Closing::Device {
                self.selected_id.set(None);
            }
            self.close_panel();
        }
        self.refresh().await;
        ActionResult::Success
    }
}
