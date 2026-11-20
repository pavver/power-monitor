#![deny(clippy::unwrap_used, clippy::expect_used)]

//! The remote support admin: login, the devices, and a device's own panel
//! relayed through the server.

mod actions;
mod api;
mod devices;
mod login;
mod model;
mod panel;
mod state;

use leptos::prelude::*;
use operator_ui::prelude::*;

use devices::DevicesScreen;
use login::LoginScreen;
use state::{Admin, Screen, POLL_FAST_MS, POLL_SLOW_MS};

#[component]
fn App() -> impl IntoView {
    let admin = Admin::new();
    admin.watch_width();
    admin.start();
    admin.poll();

    let subtitle = Signal::derive(move || match admin.screen.get() {
        Screen::Login => "Вхід адміністратора".to_string(),
        Screen::Devices => {
            let every = if admin.selected_id.get().is_some() {
                POLL_FAST_MS
            } else {
                POLL_SLOW_MS
            };
            format!("Пристрої · оновлюється кожні {} с", every / 1000)
        }
    });
    let forget_message = Signal::derive(move || {
        admin.forget_target.get().map_or_else(String::new, |target| {
            format!(
                "«{}» зникне зі списку. Якщо пристрій ще працює, він з'явиться знову на наступній перевірці.",
                target.display_name()
            )
        })
    });
    let accept_message = Signal::derive(move || {
        admin
            .accept_target
            .get()
            .map_or_else(String::new, |target| {
                format!(
                    "Прийняти новий секретний ключ для «{}»? Старий ключ більше не діятиме.",
                    target.display_name()
                )
            })
    });
    let dark = admin.dark;

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

                {move || match admin.screen.get() {
                    Screen::Login => view! { <LoginScreen admin/> }.into_any(),
                    Screen::Devices => view! { <DevicesScreen admin/> }.into_any(),
                }}

                <ConfirmDialog
                    open=admin.forget_open
                    title=Signal::stored("Забути пристрій?".to_string())
                    message=forget_message
                    confirm_text=Signal::stored("Забути".to_string())
                    cancel_text=Signal::stored("Скасувати".to_string())
                    destructive=true
                    on_confirm=AsyncAction::new(move |()| async move {
                        match admin.forget_target.get_untracked() {
                            Some(target) => admin.forget(target.id).await,
                            None => ActionResult::Success,
                        }
                    })
                />

                <ConfirmDialog
                    open=admin.accept_open
                    title=Signal::stored("Прийняти новий ключ?".to_string())
                    message=accept_message
                    confirm_text=Signal::stored("Прийняти ключ".to_string())
                    cancel_text=Signal::stored("Скасувати".to_string())
                    destructive=false
                    on_confirm=AsyncAction::new(move |()| async move {
                        match admin.accept_target.get_untracked() {
                            Some(target) => admin.settle_key(target.id, true).await,
                            None => ActionResult::Success,
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
