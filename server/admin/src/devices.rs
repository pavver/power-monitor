//! The device screen: the list on the side and the picked device's panel.

use leptos::prelude::*;
use operator_ui::prelude::*;
use wasm_bindgen::JsCast;

use crate::model::DeviceItem;
use crate::panel;
use crate::state::Admin;

#[component]
pub fn DevicesScreen(admin: Admin) -> impl IntoView {
    view! {
        <div class="admin-workspace" data-narrow=move || admin.is_narrow.get().then_some("")>
            <SidePanel title=Signal::stored("Пристрої".to_string()) collapsed=admin.side_collapsed width=85.0>
                {move || {
                    let list = admin.devices.get();
                    let messages = admin.device_messages.get();
                    if list.is_empty() {
                        view! {
                            <div class="admin-devices-empty">
                                <EmptyState
                                    title="Пристроїв ще немає".to_string()
                                    description="Кожен з'явиться тут після першої перевірки сервера.".to_string()
                                    compact=true
                                />
                            </div>
                        }
                            .into_any()
                    } else {
                        let cards = list
                            .into_iter()
                            .map(|device| {
                                let message = messages.get(&device.id).cloned();
                                view! { <DeviceCard admin device message/> }
                            })
                            .collect_view();
                        view! { <div class="admin-device-list">{cards}</div> }.into_any()
                    }
                }}
            </SidePanel>
            <div class="admin-panel-container">
                <PanelArea admin/>
            </div>
        </div>
    }
}

#[component]
fn DeviceCard(admin: Admin, device: DeviceItem, message: Option<(Tone, String)>) -> impl IntoView {
    let id = device.id.clone();
    let selected = {
        let id = id.clone();
        Signal::derive(move || admin.selected_id.get().as_deref() == Some(id.as_str()))
    };
    let (badge_status, badge_text) = device.badge();
    let (card_id, head_id, wait_id, end_id) = (id.clone(), id.clone(), id.clone(), id);
    let forget_target = device.clone();
    view! {
        <div
            class="admin-dev-card"
            data-selected=move || selected.get().then_some("")
            on:click=move |ev| {
                // The card's own controls do their own thing.
                let control = ev
                    .target()
                    .and_then(|t| t.dyn_into::<web_sys::Element>().ok())
                    .and_then(|t| t.closest("button, input, [role=switch], [role=button]").ok().flatten());
                if control.is_none() {
                    admin.select(card_id.clone());
                }
            }
        >
            <div class="admin-devhead" on:click=move |_| admin.select(head_id.clone())>
                <div class="admin-devhead-row">
                    <span class="admin-devhead-title">{device.display_name().to_string()}</span>
                    <div class="admin-devhead-badges">
                        <StatusBadge status=Signal::stored(badge_status) text=Signal::stored(badge_text.to_string())/>
                        {device.requested.then(|| view! {
                            <StatusBadge
                                status=Signal::stored(PresenceStatus::Pending)
                                text=Signal::stored("запитано".to_string())
                            />
                        })}
                    </div>
                </div>
                <span class="admin-devhead-meta">{device.display_meta()}</span>
            </div>

            <ListItem
                title=Signal::stored("Чекаю пристрій".to_string())
                subtitle=Signal::stored("перевірка кожні 2 хв замість щогодини".to_string())
                trailing=ViewFn::from(move || {
                    let id = wait_id.clone();
                    view! {
                        <AsyncSwitch
                            checked=Signal::stored(device.waiting)
                            on_toggle=AsyncAction::new(move |on: bool| admin.set_waiting(id.clone(), on))
                            label="Чекаю пристрій"
                        />
                    }
                })
            />

            <div class="admin-actions">
                {device.online.then(|| view! {
                    <ActionButton
                        text=Signal::stored("Завершити сесію".to_string())
                        on_click=AsyncAction::new(move |()| admin.end_session(end_id.clone()))
                    />
                })}
                <ActionButton
                    text=Signal::stored("Забути".to_string())
                    dangerous=true
                    on_click=AsyncAction::new(move |()| {
                        admin.forget_target.set(Some(forget_target.clone()));
                        admin.forget_open.set(true);
                        async move { ActionResult::Success }
                    })
                />
            </div>

            {message.map(|(tone, text)| view! {
                <div class="admin-card-msg">
                    <Banner tone=tone text=Signal::stored(text)/>
                </div>
            })}
        </div>
    }
}

/// The picked device's panel, or why it is not there yet.
#[component]
fn PanelArea(admin: Admin) -> impl IntoView {
    move || {
        match (admin.selected_id.get(), admin.panel_online.get()) {
        (None, _) => view! {
            <div class="admin-waiting-area">
                <EmptyState
                    title="Оберіть пристрій".to_string()
                    description="Виберіть пристрій зі списку зліва, щоб відкрити його панель.".to_string()
                    compact=false
                />
            </div>
        }
            .into_any(),
        (Some(_), false) => view! {
            <div class="admin-waiting-area">
                <EmptyState
                    title="Сесію запитано".to_string()
                    description="Пристрій відкриє її на найближчій перевірці: за кілька хвилин, якщо ввімкнено «Чекаю пристрій», інакше до години.".to_string()
                    compact=false
                />
            </div>
        }
            .into_any(),
        (Some(_), true) => view! {
            <iframe
                node_ref=admin.iframe_ref
                class="admin-iframe"
                src=admin.iframe_src
                on:load=move |_| panel::loaded(admin)
            ></iframe>
        }
            .into_any(),
    }
    }
}
