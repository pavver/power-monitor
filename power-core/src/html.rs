//! The single web page: current status plus the settings form.

use crate::config::Config;
use crate::message::format_duration;
use crate::power::PowerState;

pub struct Status<'a> {
    /// Confirmed state and how long it has lasted; `None` until confirmed.
    pub power: Option<(PowerState, u64)>,
    pub network: &'a str,
    pub telegram: &'a str,
    pub pending: usize,
}

/// Renders the page. Secrets are never sent back to the browser.
pub fn page(status: &Status, config: Option<&Config>, notice: Option<&str>) -> String {
    let notice = notice.map_or(String::new(), |text| format!("<p class=notice>{}</p>", escape(text)));
    format!(
        r#"<!doctype html><html lang=uk><head><meta charset=utf-8>
<meta name=viewport content="width=device-width,initial-scale=1">
<title>Монітор світла</title><style>{STYLE}</style></head><body><main>
<h1>Монітор світла</h1>{notice}{status}{settings}{test}
</main></body></html>"#,
        status = status_section(status),
        settings = settings_form(config),
        test = if config.is_some() { TEST_FORM } else { "" },
    )
}

pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

const STYLE: &str = "body{font-family:system-ui,sans-serif;margin:0;background:#f4f4f5;color:#18181b}\
main{max-width:28rem;margin:0 auto;padding:1rem}\
section{background:#fff;border-radius:.75rem;padding:1rem;margin-bottom:1rem}\
.power{font-size:1.5rem;font-weight:600}\
label{display:block;margin-top:.75rem;font-size:.9rem}\
input{box-sizing:border-box;width:100%;padding:.5rem;font-size:1rem;margin-top:.25rem}\
button{width:100%;padding:.75rem;margin-top:1rem;font-size:1rem}\
.notice{background:#fef9c3;padding:.75rem;border-radius:.5rem}\
small{color:#71717a}";

const TEST_FORM: &str = r#"<section><form method=post action="/test">
<button>Надіслати тестове повідомлення</button></form></section>"#;

fn status_section(status: &Status) -> String {
    let power = match status.power {
        None => "⏳ Визначається…".to_string(),
        Some((PowerState::Present, ms)) => format!("🟢 Світло є · {}", format_duration(ms)),
        Some((PowerState::Absent, ms)) => format!("🔴 Світла немає · {}", format_duration(ms)),
    };
    format!(
        "<section><div class=power>{power}</div><p>{}</p><p>Telegram: {}<br>В черзі: {}</p></section>",
        escape(status.network),
        escape(status.telegram),
        status.pending,
    )
}

fn settings_form(config: Option<&Config>) -> String {
    let value = |get: fn(&Config) -> String| config.map_or(String::new(), |c| escape(&get(c)));
    let secret_hint = if config.is_some() { "залиште порожнім, щоб не змінювати" } else { "" };
    format!(
        r#"<section><h2>Налаштування</h2><form method=post action="/save">
<label>Wi-Fi мережа<input name=ssid required maxlength=32 value="{ssid}"></label>
<label>Пароль Wi-Fi<input name=password type=password maxlength=63 placeholder="{secret_hint}"></label>
<label>Токен бота<input name=token placeholder="{secret_hint}"></label>
<small>Отримайте у @BotFather</small>
<label>Chat ID<input name=chat_id required inputmode=numeric value="{chat_id}"></label>
<small>Напишіть боту, потім відкрийте api.telegram.org/bot&lt;токен&gt;/getUpdates</small>
<label>Назва пристрою<input name=name required maxlength=32 value="{name}"></label>
<button>Зберегти й перезавантажити</button></form></section>"#,
        ssid = value(|c| c.ssid().to_string()),
        chat_id = value(|c| c.chat_id().to_string()),
        name = config.map_or("Дім".to_string(), |c| escape(c.device_name())),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form;

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    fn status(power: Option<(PowerState, u64)>) -> Status<'static> {
        Status { power, network: "Wi-Fi: Home, 192.168.1.50", telegram: "надіслано", pending: 2 }
    }

    fn config(ssid: &str) -> Config {
        let body = form::encode(&[
            ("ssid", ssid),
            ("password", "secret-pass"),
            ("token", TOKEN),
            ("chat_id", "-100"),
            ("name", "Дім"),
        ]);
        Config::from_form(&body, None).unwrap()
    }

    #[test]
    fn escapes_html_special_characters() {
        assert_eq!(escape(r#"<a href="x">'&'</a>"#), "&lt;a href=&quot;x&quot;&gt;&#39;&amp;&#39;&lt;/a&gt;");
    }

    #[test]
    fn shows_power_state_and_how_long_it_lasted() {
        let html = page(&status(Some((PowerState::Present, 2 * 3_600_000 + 15 * 60_000))), None, None);
        assert!(html.contains("Світло є"), "{html}");
        assert!(html.contains("2 год 15 хв"), "{html}");
        let html = page(&status(Some((PowerState::Absent, 60_000))), None, None);
        assert!(html.contains("Світла немає"), "{html}");
    }

    #[test]
    fn shows_that_power_is_not_yet_known() {
        assert!(page(&status(None), None, None).contains("Визначається"));
    }

    #[test]
    fn shows_network_telegram_and_queue() {
        let html = page(&status(None), None, None);
        assert!(html.contains("Wi-Fi: Home, 192.168.1.50"));
        assert!(html.contains("надіслано"));
        assert!(html.contains("В черзі: 2"));
    }

    #[test]
    fn prefills_settings_but_never_secrets() {
        let html = page(&status(None), Some(&config("Home<1>")), None);
        assert!(html.contains(r#"value="Home&lt;1&gt;""#), "{html}");
        assert!(html.contains(r#"value="-100""#));
        assert!(html.contains(r#"value="Дім""#));
        assert!(!html.contains("secret-pass"));
        assert!(!html.contains(TOKEN));
    }

    #[test]
    fn test_message_button_needs_a_config() {
        assert!(!page(&status(None), None, None).contains(r#"action="/test""#));
        assert!(page(&status(None), Some(&config("Home")), None).contains(r#"action="/test""#));
    }

    #[test]
    fn shows_escaped_notice() {
        let html = page(&status(None), None, Some("Помилка: <токен>"));
        assert!(html.contains("Помилка: &lt;токен&gt;"));
    }
}
