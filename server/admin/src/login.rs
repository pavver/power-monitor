//! The login screen: password and the digits from the picture.

use leptos::prelude::*;
use operator_ui::prelude::*;

use crate::state::Admin;

#[component]
pub fn LoginScreen(admin: Admin) -> impl IntoView {
    let captcha_src = move || {
        let id = admin.captcha_id.get();
        if id.is_empty() {
            String::new()
        } else {
            format!("/admin/captcha/{id}")
        }
    };
    view! {
        <main class="admin-main">
            <div class="admin-card">
                <h2 class="op-t-section-title">"Вхід"</h2>
                <label class="admin-label">"Пароль"</label>
                <TextField
                    value=admin.password
                    placeholder=Signal::stored("Пароль".to_string())
                    password=true
                    show_reveal=true
                    label="Пароль"
                />

                <label class="admin-label">"Цифри з картинки"</label>
                <div class="admin-captcha-row">
                    <img class="admin-captcha-img" src=captcha_src alt="Цифри для перевірки" width="200" height="64"/>
                    <ActionButton
                        text=Signal::stored("Інші цифри".to_string())
                        on_click=AsyncAction::new(move |()| {
                            admin.fetch_captcha();
                            async move { ActionResult::Success }
                        })
                    />
                </div>
                <TextField
                    value=admin.captcha
                    placeholder=Signal::stored("Цифри з картинки".to_string())
                    label="Цифри з картинки"
                />

                <div class="admin-actions">
                    <ActionButton
                        text=Signal::stored("Увійти".to_string())
                        keep_flash_icon_on_success=true
                        on_click=AsyncAction::new(move |()| admin.login())
                    />
                </div>

                {move || admin.login_error.get().map(|msg| view! {
                    <div class="admin-card-msg">
                        <Banner tone=Tone::Error text=Signal::stored(msg)/>
                    </div>
                })}
            </div>
        </main>
    }
}
