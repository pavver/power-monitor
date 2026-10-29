//! Telegram Bot API: request shapes and response parsing.

use serde::Serialize;
use serde_json::Value;

use crate::form;
use crate::settings::BotToken;

pub struct Request {
    pub url: String,
    /// `application/x-www-form-urlencoded` body.
    pub body: String,
}

/// A chat that wrote to the bot, offered as the report destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chat {
    pub id: i64,
    pub title: String,
}

pub fn send_message(token: &BotToken, chat_id: i64, text: &str) -> Request {
    Request {
        url: method_url(token, "sendMessage"),
        body: form::encode(&[("chat_id", &chat_id.to_string()), ("text", text)]),
    }
}

/// Checks the token and gives the bot's username.
pub fn get_me(token: &BotToken) -> String {
    method_url(token, "getMe")
}

/// Recent messages to the bot; the chats they came from.
pub fn get_updates(token: &BotToken) -> String {
    method_url(token, "getUpdates")
}

fn method_url(token: &BotToken, method: &str) -> String {
    format!("https://api.telegram.org/bot{}/{method}", token.as_str())
}

/// The `result` of a successful call, or a message for the user.
pub fn result(status: u16, body: &str) -> Result<Value, String> {
    let Ok(mut response) = serde_json::from_str::<Value>(body) else {
        return Err(format!("Telegram: незрозуміла відповідь (код {status})"));
    };
    if response["ok"] == Value::Bool(true) {
        return Ok(response["result"].take());
    }
    let description = response["description"].as_str().unwrap_or("");
    Err(match status {
        401 | 404 => "Telegram не приймає токен бота".to_string(),
        400 if description.contains("chat not found") => {
            "Чат не знайдено — напишіть боту /start і перевірте Chat ID".to_string()
        }
        403 => "Бот не може писати в цей чат: його заблоковано або видалено з групи".to_string(),
        409 => "У бота ввімкнено webhook — вимкніть його (deleteWebhook)".to_string(),
        _ => format!("Telegram: {description}"),
    })
}

/// `@username` from a `getMe` result.
pub fn bot_username(result: &Value) -> Option<String> {
    result["username"].as_str().map(str::to_string)
}

/// Distinct chats from a `getUpdates` result, newest first.
pub fn chats(result: &Value) -> Vec<Chat> {
    const SOURCES: [&str; 5] =
        ["message", "edited_message", "channel_post", "my_chat_member", "chat_member"];
    let updates = result.as_array().map_or(&[][..], Vec::as_slice);
    let mut found: Vec<Chat> = Vec::new();
    for update in updates.iter().rev() {
        let chat = SOURCES.iter().map(|key| &update[*key]["chat"]).find(|chat| chat.is_object());
        let Some((chat, id)) = chat.and_then(|chat| Some((chat, chat["id"].as_i64()?))) else {
            continue;
        };
        if !found.iter().any(|known| known.id == id) {
            found.push(Chat { id, title: chat_title(chat) });
        }
    }
    found
}

/// Group title, person's name, or `@username`.
fn chat_title(chat: &Value) -> String {
    if let Some(title) = chat["title"].as_str() {
        return title.to_string();
    }
    let name = [&chat["first_name"], &chat["last_name"]]
        .iter()
        .filter_map(|part| part.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    match (name.is_empty(), chat["username"].as_str()) {
        (false, _) => name,
        (true, Some(username)) => format!("@{username}"),
        (true, None) => chat["id"].to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    fn token() -> BotToken {
        BotToken::parse(TOKEN).unwrap()
    }

    #[test]
    fn builds_requests() {
        let request = send_message(&token(), -100, "Світло є & ok");
        assert_eq!(request.url, format!("https://api.telegram.org/bot{TOKEN}/sendMessage"));
        assert_eq!(
            form::parse(&request.body),
            vec![
                ("chat_id".to_string(), "-100".to_string()),
                ("text".to_string(), "Світло є & ok".to_string()),
            ]
        );
        assert_eq!(get_me(&token()), format!("https://api.telegram.org/bot{TOKEN}/getMe"));
        assert_eq!(get_updates(&token()), format!("https://api.telegram.org/bot{TOKEN}/getUpdates"));
    }

    #[test]
    fn successful_call_gives_its_result() {
        assert_eq!(result(200, r#"{"ok":true,"result":{"id":1}}"#), Ok(json!({"id": 1})));
    }

    #[test]
    fn errors_are_explained_to_the_user() {
        let cases = [
            (401, r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#, "Telegram не приймає токен бота"),
            (404, r#"{"ok":false,"error_code":404,"description":"Not Found"}"#, "Telegram не приймає токен бота"),
            (400, r#"{"ok":false,"description":"Bad Request: chat not found"}"#, "Чат не знайдено — напишіть боту /start і перевірте Chat ID"),
            (403, r#"{"ok":false,"description":"Forbidden: bot was blocked by the user"}"#, "Бот не може писати в цей чат: його заблоковано або видалено з групи"),
            (409, r#"{"ok":false,"description":"Conflict: can't use getUpdates method while webhook is active"}"#, "У бота ввімкнено webhook — вимкніть його (deleteWebhook)"),
            (429, r#"{"ok":false,"description":"Too Many Requests: retry after 5"}"#, "Telegram: Too Many Requests: retry after 5"),
        ];
        for (status, body, expected) in cases {
            assert_eq!(result(status, body), Err(expected.to_string()), "{status}");
        }
    }

    #[test]
    fn unreadable_response_is_reported_with_its_code() {
        assert_eq!(result(502, "<html>"), Err("Telegram: незрозуміла відповідь (код 502)".to_string()));
    }

    #[test]
    fn reads_bot_username() {
        let me = json!({"id": 1, "is_bot": true, "first_name": "Світло", "username": "svitlo_bot"});
        assert_eq!(bot_username(&me), Some("svitlo_bot".to_string()));
        assert_eq!(bot_username(&json!({})), None);
    }

    #[test]
    fn lists_distinct_chats_newest_first() {
        let updates = json!([
            {"update_id": 1, "message": {"chat": {"id": 42, "type": "private", "first_name": "Олена", "last_name": "К"}}},
            {"update_id": 2, "message": {"chat": {"id": -100, "type": "group", "title": "Сім'я"}}},
            {"update_id": 3, "my_chat_member": {"chat": {"id": -200, "type": "supergroup", "title": "Будинок"}}},
            {"update_id": 4, "channel_post": {"chat": {"id": -300, "type": "channel", "title": "Канал"}}},
            {"update_id": 5, "message": {"chat": {"id": 42, "type": "private", "first_name": "Олена", "last_name": "К"}}},
            {"update_id": 6, "message": {"chat": {"id": 7, "type": "private", "username": "petro"}}},
            {"update_id": 7, "edited_message": {"chat": {"id": 8, "type": "private", "first_name": "Іван"}}},
        ]);
        assert_eq!(
            chats(&updates),
            vec![
                Chat { id: 8, title: "Іван".to_string() },
                Chat { id: 7, title: "@petro".to_string() },
                Chat { id: 42, title: "Олена К".to_string() },
                Chat { id: -300, title: "Канал".to_string() },
                Chat { id: -200, title: "Будинок".to_string() },
                Chat { id: -100, title: "Сім'я".to_string() },
            ]
        );
    }

    #[test]
    fn updates_without_chats_are_skipped() {
        assert_eq!(chats(&json!([{"update_id": 1, "poll": {}}])), vec![]);
        assert_eq!(chats(&json!({})), vec![]);
    }
}
