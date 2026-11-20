//! Telegram Bot API: request shapes and response parsing.

use std::fmt;
use std::io::Read;

use serde::Serialize;
use serde_json::Value;

use crate::json::read::{Bad, Reader};
use crate::settings::{BotToken, Destination};

/// A chat (or a topic of a forum group) that wrote to the bot, offered as
/// the report destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Chat {
    pub id: i64,
    pub thread_id: Option<i64>,
    pub title: String,
}

/// The URL for `sendMessage`.
pub fn send_message_url(token: &BotToken) -> String {
    method_url(token, "sendMessage")
}

/// The URL for `editMessageText`.
pub fn edit_message_url(token: &BotToken) -> String {
    method_url(token, "editMessageText")
}

/// The URL for `deleteMessage`.
pub fn delete_message_url(token: &BotToken) -> String {
    method_url(token, "deleteMessage")
}

/// The `sendMessage` body as JSON: `{"chat_id":…,"message_thread_id":…,"text":"…"}`.
pub fn write_send_message(to: Destination, text: &str, out: &mut impl fmt::Write) -> fmt::Result {
    write_send_message_with(to, |w| w.write_str(text), out)
}

/// Streams a `sendMessage` body into `out`, writing the text via `write_text` through JSON escaping.
pub fn write_send_message_with(
    to: Destination,
    write_text: impl FnOnce(&mut dyn fmt::Write) -> fmt::Result,
    out: &mut impl fmt::Write,
) -> fmt::Result {
    out.write_str("{\"chat_id\":")?;
    write!(out, "{}", to.chat_id)?;
    if let Some(thread_id) = to.thread_id {
        out.write_str(",\"message_thread_id\":")?;
        write!(out, "{thread_id}")?;
    }
    out.write_str(",\"text\":\"")?;
    write_text(&mut crate::json::Escaped(out))?;
    out.write_str("\"}")
}

/// The `editMessageText` body as JSON: `{"chat_id":…,"message_id":…,"text":"…"}`.
pub fn write_edit_message(to: Destination, message_id: i64, text: &str, out: &mut impl fmt::Write) -> fmt::Result {
    write_edit_message_with(to, message_id, |w| w.write_str(text), out)
}

/// Streams an `editMessageText` body into `out`, writing the text via `write_text` through JSON escaping.
pub fn write_edit_message_with(
    to: Destination,
    message_id: i64,
    write_text: impl FnOnce(&mut dyn fmt::Write) -> fmt::Result,
    out: &mut impl fmt::Write,
) -> fmt::Result {
    out.write_str("{\"chat_id\":")?;
    write!(out, "{}", to.chat_id)?;
    out.write_str(",\"message_id\":")?;
    write!(out, "{message_id}")?;
    out.write_str(",\"text\":\"")?;
    write_text(&mut crate::json::Escaped(out))?;
    out.write_str("\"}")
}

/// The `deleteMessage` body as JSON: `{"chat_id":…,"message_id":…}`.
pub fn write_delete_message(to: Destination, message_id: i64, out: &mut impl fmt::Write) -> fmt::Result {
    out.write_str("{\"chat_id\":")?;
    write!(out, "{}", to.chat_id)?;
    out.write_str(",\"message_id\":")?;
    write!(out, "{message_id}")?;
    out.write_str("}")
}

/// The URL for `sendChatAction`.
pub fn chat_action_url(token: &BotToken) -> String {
    method_url(token, "sendChatAction")
}

/// The `sendChatAction` body: "typing" in the destination, which fails as
/// sending would (unknown chat, bot not in it, no such topic) yet leaves no
/// message behind.
pub fn write_chat_action(to: Destination, out: &mut impl fmt::Write) -> fmt::Result {
    write!(out, "{{\"chat_id\":{}", to.chat_id)?;
    if let Some(thread_id) = to.thread_id {
        write!(out, ",\"message_thread_id\":{thread_id}")?;
    }
    out.write_str(",\"action\":\"typing\"}")
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

/// Keys kept from a reply: what `getMe`, `getUpdates`, `sendMessage` and an error need.
/// Everything else — message texts, entities, photos — is skipped as it
/// is read and never held in memory.
const KEPT: [&str; 22] = [
    "ok", "description", "result", "username", "message", "edited_message", "channel_post", "my_chat_member",
    "chat_member", "chat", "id", "title", "first_name", "last_name", "is_forum", "is_topic_message",
    "message_thread_id", "forum_topic_created", "forum_topic_edited", "name", "reply_to_message",
    "message_id",
];

/// The outcome of editing or deleting a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Gone,
}

/// Classifies the outcome of an `editMessageText` call.
pub fn edit_outcome(result: Result<Value, String>) -> Result<Outcome, String> {
    match result {
        Ok(_) => Ok(Outcome::Done),
        Err(err) if err.contains("message is not modified") => Ok(Outcome::Done),
        Err(err) if is_gone(&err) => Ok(Outcome::Gone),
        Err(err) => Err(err),
    }
}

/// Classifies the outcome of a `deleteMessage` call.
pub fn delete_outcome(result: Result<Value, String>) -> Result<Outcome, String> {
    match result {
        Ok(_) => Ok(Outcome::Done),
        Err(err) if is_gone(&err) => Ok(Outcome::Gone),
        Err(err) => Err(err),
    }
}

fn is_gone(err: &str) -> bool {
    err.contains("message to edit not found")
        || err.contains("message to delete not found")
        || err.contains("message can't be edited")
        || err.contains("message can't be deleted")
}

/// Another program polls the same bot and takes its updates first, so the
/// device never sees the messages written to the bot.
pub const BOT_BUSY: &str = "Цього бота вже читає інша програма чи пристрій і забирає повідомлення першою. \
     Вимкніть її, поки налаштовуєте, або створіть для пристрою окремого бота";

/// Longer strings are dropped: no kept value is near it (names and titles
/// are at most 128 characters).
const MAX_KEPT_STRING: usize = 1024;

/// Reads a Bot API reply from `reader` as it comes: its `result`, with only
/// the kept keys, or a message for the user.
pub fn read_reply(status: u16, reader: &mut dyn Read) -> Result<Value, String> {
    let mut value = Value::Null;
    read_with(status, reader, &mut |r| r.value(&kept, MAX_KEPT_STRING).map(|result| value = result))?;
    Ok(value)
}

/// Distinct chats from a `getUpdates` reply, newest first, read one update
/// at a time. Each topic of a forum group that wrote to the bot is a
/// destination of its own.
pub fn read_chats(status: u16, reader: &mut dyn Read) -> Result<Vec<Chat>, String> {
    let mut found = Found::default();
    read_with(status, reader, &mut |r| r.array(&mut |r| r.value(&kept, MAX_KEPT_STRING).map(|update| found.add(&update))))?;
    Ok(found.chats())
}

/// The bot's `@username` from a `getMe` result.
pub fn bot_username(me: &Value) -> Result<String, String> {
    me["username"].as_str().map(str::to_string).ok_or_else(|| "Telegram не повернув ім'я бота".to_string())
}

/// The `message_id` from a `sendMessage` result.
pub fn message_id(result: &Value) -> Result<i64, String> {
    result
        .as_i64()
        .or_else(|| result["message_id"].as_i64())
        .ok_or_else(|| "Telegram не повернув id повідомлення".to_string())
}

fn kept(key: &str) -> bool {
    KEPT.contains(&key)
}

/// `{"ok":…,"description":…,"result":…}`, `result` read by `read_result`.
fn read_with(
    status: u16,
    input: &mut dyn Read,
    read_result: &mut dyn FnMut(&mut Reader) -> Result<(), Bad>,
) -> Result<(), String> {
    let unclear = || format!("Telegram: незрозуміла відповідь (код {status})");
    let (mut ok, mut description, mut has_result) = (false, String::new(), false);
    let mut reader = Reader::new(input);
    reader
        .object(&mut |r, key| match key {
            "ok" => r.value(&kept, 0).map(|value| ok = value == Value::Bool(true)),
            "description" => r.value(&kept, MAX_KEPT_STRING).map(|value| {
                description = value.as_str().unwrap_or_default().to_string();
            }),
            "result" => {
                has_result = true;
                read_result(r)
            }
            _ => r.skip(),
        })
        .and_then(|()| reader.end())
        .map_err(|_| unclear())?;
    if ok {
        return if has_result { Ok(()) } else { Err(unclear()) };
    }
    Err(match status {
        401 | 404 => "Telegram не приймає токен бота".to_string(),
        400 if description.contains("message thread not found") => {
            "Гілку форуму не знайдено — можливо, її видалено. Оберіть чат заново".to_string()
        }
        400 if description.contains("chat not found") => {
            "Чат не знайдено — напишіть боту /start і перевірте Chat ID".to_string()
        }
        403 => "Бот не може писати в цей чат: його заблоковано або видалено з групи".to_string(),
        409 if description.contains("other getUpdates request") => BOT_BUSY.to_string(),
        409 => "У бота ввімкнено webhook — вимкніть його (deleteWebhook)".to_string(),
        _ => format!("Telegram: {description}"),
    })
}

/// Chats and topic names seen so far, the latest of each last.
#[derive(Default)]
struct Found {
    /// (chat id, thread id, chat title, is a forum)
    chats: Vec<(i64, Option<i64>, String, bool)>,
    topics: Vec<((i64, i64), String)>,
}

impl Found {
    fn add(&mut self, update: &Value) {
        const SOURCES: [&str; 5] = ["message", "edited_message", "channel_post", "my_chat_member", "chat_member"];
        self.add_topic_names(&update["message"]);
        let Some(source) = SOURCES.iter().map(|key| &update[*key]).find(|source| source["chat"].is_object()) else {
            return;
        };
        let chat = &source["chat"];
        let Some(id) = chat["id"].as_i64() else { return };
        let thread_id = topic(source);
        self.chats.retain(|(known, thread, _, _)| (*known, *thread) != (id, thread_id));
        self.chats.push((id, thread_id, chat_title(chat), chat["is_forum"].as_bool() == Some(true)));
    }

    /// Topic names from creation and renaming service messages, also as the
    /// message a topic message replies to.
    fn add_topic_names(&mut self, message: &Value) {
        let Some(chat_id) = message["chat"]["id"].as_i64() else { return };
        for service in [&message["reply_to_message"], message] {
            let name = ["forum_topic_created", "forum_topic_edited"].iter().find_map(|kind| service[*kind]["name"].as_str());
            if let (Some(thread), Some(name)) = (service["message_thread_id"].as_i64(), name) {
                let key = (chat_id, thread);
                self.topics.retain(|(known, _)| *known != key);
                self.topics.push((key, name.to_string()));
            }
        }
    }

    fn chats(self) -> Vec<Chat> {
        let topics = self.topics;
        let title = |id: i64, thread: Option<i64>, chat: String, forum: bool| match (thread, forum) {
            (Some(thread), _) => {
                let name = topics.iter().find(|(key, _)| *key == (id, thread)).map(|(_, name)| name.clone());
                format!("{chat} › {}", name.unwrap_or_else(|| format!("гілка {thread}")))
            }
            (None, true) => format!("{chat} › General"),
            (None, false) => chat,
        };
        let chats = self.chats.into_iter().rev();
        chats.map(|(id, thread_id, chat, forum)| Chat { id, thread_id, title: title(id, thread_id, chat, forum) }).collect()
    }
}

/// The forum topic a message was written in. Reply chains in ordinary groups
/// also carry a thread id, so only topic messages count.
fn topic(message: &Value) -> Option<i64> {
    if message["is_topic_message"].as_bool() != Some(true) {
        return None;
    }
    message["message_thread_id"].as_i64()
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
    use serde_json::Value;

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    fn token() -> BotToken {
        BotToken::parse(TOKEN).unwrap()
    }

    #[test]
    fn builds_requests() {
        assert_eq!(send_message_url(&token()), format!("https://api.telegram.org/bot{TOKEN}/sendMessage"));
        assert_eq!(edit_message_url(&token()), format!("https://api.telegram.org/bot{TOKEN}/editMessageText"));
        assert_eq!(delete_message_url(&token()), format!("https://api.telegram.org/bot{TOKEN}/deleteMessage"));
        assert_eq!(get_me(&token()), format!("https://api.telegram.org/bot{TOKEN}/getMe"));
        assert_eq!(get_updates(&token()), format!("https://api.telegram.org/bot{TOKEN}/getUpdates"));

        let to = Destination { chat_id: -100, thread_id: None };
        let mut out = String::new();
        let mut count = crate::json::Count(0);
        let text = "Світло є & ok \"quoted\" \\ \n newline";
        write_send_message(to, text, &mut out).unwrap();
        write_send_message(to, text, &mut count).unwrap();
        assert_eq!(count.0, out.len());

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["chat_id"], -100);
        assert_eq!(parsed.get("message_thread_id"), None);
        assert_eq!(parsed["text"], text);
    }

    #[test]
    fn checks_a_destination_without_a_message() {
        assert_eq!(chat_action_url(&token()), format!("https://api.telegram.org/bot{TOKEN}/sendChatAction"));
        let mut out = String::new();
        write_chat_action(Destination { chat_id: -100, thread_id: Some(77) }, &mut out).unwrap();
        assert_eq!(out, r#"{"chat_id":-100,"message_thread_id":77,"action":"typing"}"#);
        out.clear();
        write_chat_action(Destination { chat_id: 5, thread_id: None }, &mut out).unwrap();
        assert_eq!(out, r#"{"chat_id":5,"action":"typing"}"#);
    }

    #[test]
    fn builds_edit_and_delete_requests() {
        let to = Destination { chat_id: -100, thread_id: Some(77) };
        let mut out = String::new();
        let mut count = crate::json::Count(0);
        let text = "hi \"quoted\" \\ \n 🇺🇦";
        write_edit_message(to, 42, text, &mut out).unwrap();
        write_edit_message(to, 42, text, &mut count).unwrap();
        assert_eq!(count.0, out.len());

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["chat_id"], -100);
        assert_eq!(parsed["message_id"], 42);
        assert_eq!(parsed["text"], text);
        assert_eq!(parsed.get("message_thread_id"), None);
        assert_eq!(out, "{\"chat_id\":-100,\"message_id\":42,\"text\":\"hi \\\"quoted\\\" \\\\ \\n 🇺🇦\"}");

        let mut del_out = String::new();
        let mut del_count = crate::json::Count(0);
        write_delete_message(to, 42, &mut del_out).unwrap();
        write_delete_message(to, 42, &mut del_count).unwrap();
        assert_eq!(del_count.0, del_out.len());

        let del_parsed: Value = serde_json::from_str(&del_out).unwrap();
        assert_eq!(del_parsed["chat_id"], -100);
        assert_eq!(del_parsed["message_id"], 42);
        assert_eq!(del_parsed.get("message_thread_id"), None);
        assert_eq!(del_out, "{\"chat_id\":-100,\"message_id\":42}");
    }

    #[test]
    fn sends_into_a_forum_topic() {
        let to = Destination { chat_id: -100, thread_id: Some(77) };
        let mut out = String::new();
        let mut count = crate::json::Count(0);
        let text = "hi \t \\ \" \n 🇺🇦";
        write_send_message(to, text, &mut out).unwrap();
        write_send_message(to, text, &mut count).unwrap();
        assert_eq!(count.0, out.len());

        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["chat_id"], -100);
        assert_eq!(parsed["message_thread_id"], 77);
        assert_eq!(parsed["text"], text);
    }

    #[test]
    fn successful_call_gives_its_result() {
        let reply = br#"{"ok":true,"result":{"id":1}}"#;
        let val: Value = read_reply(200, &mut reply.as_slice()).unwrap();
        assert_eq!(val["id"], 1);

        // What is not kept is skipped, not read into memory.
        let sent = read_reply(200, &mut br#"{"ok":true,"result":{"message_id":42,"text":"ignored"}}"#.as_slice()).unwrap();
        assert_eq!(sent, serde_json::json!({"message_id": 42}));

        let ignored = read_reply(200, &mut br#"{"ok":true,"result":{"other":42,"text":"ignored"}}"#.as_slice()).unwrap();
        assert_eq!(ignored, serde_json::json!({}));
    }

    #[test]
    fn reads_message_id_from_send_message_reply() {
        let json = r#"{"ok":true,"result":{"message_id":123456,"from":{"id":99,"is_bot":true,"first_name":"Bot"},"chat":{"id":-100,"title":"Group","type":"supergroup"},"date":1700000000,"text":"Power on"}}"#;
        let result = read_reply(200, &mut json.as_bytes()).unwrap();
        assert_eq!(message_id(&result), Ok(123456));

        let missing = serde_json::json!({"id": 1});
        assert_eq!(message_id(&missing), Err("Telegram не повернув id повідомлення".to_string()));
    }

    #[test]
    fn classifies_edit_and_delete_outcomes() {
        // Edit success
        let ok_edit = read_reply(200, &mut br#"{"ok":true,"result":{"message_id":42}}"#.as_slice());
        assert_eq!(edit_outcome(ok_edit), Ok(Outcome::Done));

        // Edit not modified is success
        let not_modified = read_reply(400, &mut br#"{"ok":false,"error_code":400,"description":"Bad Request: message is not modified: specified new message content and reply markup are exactly the same as a current content and reply markup of the message"}"#.as_slice());
        assert_eq!(edit_outcome(not_modified), Ok(Outcome::Done));

        // Edit message to edit not found -> Gone
        let edit_not_found = read_reply(400, &mut br#"{"ok":false,"error_code":400,"description":"Bad Request: message to edit not found"}"#.as_slice());
        assert_eq!(edit_outcome(edit_not_found), Ok(Outcome::Gone));

        // Edit can't be edited -> Gone
        let cant_edit = read_reply(400, &mut br#"{"ok":false,"error_code":400,"description":"Bad Request: message can't be edited"}"#.as_slice());
        assert_eq!(edit_outcome(cant_edit), Ok(Outcome::Gone));

        // Edit other error -> Err
        let edit_blocked = read_reply(403, &mut br#"{"ok":false,"error_code":403,"description":"Forbidden: bot was blocked by the user"}"#.as_slice());
        assert_eq!(edit_outcome(edit_blocked), Err("Бот не може писати в цей чат: його заблоковано або видалено з групи".to_string()));

        // Delete success
        let ok_del = read_reply(200, &mut br#"{"ok":true,"result":true}"#.as_slice());
        assert_eq!(delete_outcome(ok_del), Ok(Outcome::Done));

        // Delete message to delete not found -> Gone
        let del_not_found = read_reply(400, &mut br#"{"ok":false,"error_code":400,"description":"Bad Request: message to delete not found"}"#.as_slice());
        assert_eq!(delete_outcome(del_not_found), Ok(Outcome::Gone));

        // Delete can't be deleted -> Gone
        let cant_delete = read_reply(400, &mut br#"{"ok":false,"error_code":400,"description":"Bad Request: message can't be deleted"}"#.as_slice());
        assert_eq!(delete_outcome(cant_delete), Ok(Outcome::Gone));

        // Delete other error -> Err
        let del_unauthorized = read_reply(401, &mut br#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#.as_slice());
        assert_eq!(delete_outcome(del_unauthorized), Err("Telegram не приймає токен бота".to_string()));
    }

    #[test]
    fn errors_are_explained_to_the_user() {
        let cases = [
            (401, r#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#, "Telegram не приймає токен бота"),
            (404, r#"{"ok":false,"error_code":404,"description":"Not Found"}"#, "Telegram не приймає токен бота"),
            (400, r#"{"ok":false,"description":"Bad Request: chat not found"}"#, "Чат не знайдено — напишіть боту /start і перевірте Chat ID"),
            (403, r#"{"ok":false,"description":"Forbidden: bot was blocked by the user"}"#, "Бот не може писати в цей чат: його заблоковано або видалено з групи"),
            (409, r#"{"ok":false,"description":"Conflict: can't use getUpdates method while webhook is active"}"#, "У бота ввімкнено webhook — вимкніть його (deleteWebhook)"),
            (409, r#"{"ok":false,"error_code":409,"description":"Conflict: terminated by other getUpdates request; make sure that only one bot instance is running"}"#, BOT_BUSY),
            (429, r#"{"ok":false,"description":"Too Many Requests: retry after 5"}"#, "Telegram: Too Many Requests: retry after 5"),
        ];
        for (status, body, expected) in cases {
            assert_eq!(read_reply(status, &mut body.as_bytes()), Err(expected.to_string()), "{status}");
        }
    }

    #[test]
    fn unreadable_response_is_reported_with_its_code() {
        assert_eq!(read_reply(502, &mut b"<html>".as_slice()), Err("Telegram: незрозуміла відповідь (код 502)".to_string()));
    }

    #[test]
    fn ok_with_missing_result_is_unreadable() {
        assert_eq!(read_reply(200, &mut br#"{"ok":true}"#.as_slice()), Err("Telegram: незрозуміла відповідь (код 200)".to_string()));
    }

    #[test]
    fn reads_bot_username() {
        let ok = r#"{"ok":true,"result":{"id": 1, "is_bot": true, "first_name": "Світло", "username": "svitlo_bot"}}"#;
        assert_eq!(read_reply(200, &mut ok.as_bytes()).and_then(|me| bot_username(&me)), Ok("svitlo_bot".to_string()));

        let missing = br#"{"ok":true,"result":{"id": 1, "is_bot": true}}"#;
        assert_eq!(read_reply(200, &mut missing.as_slice()).and_then(|me| bot_username(&me)), Err("Telegram не повернув ім'я бота".to_string()));

        let err = br#"{"ok":false,"error_code":401,"description":"Unauthorized"}"#;
        assert_eq!(read_reply(401, &mut err.as_slice()).and_then(|me| bot_username(&me)), Err("Telegram не приймає токен бота".to_string()));
    }

    #[test]
    fn lists_distinct_chats_newest_first() {
        let json = r#"{"ok":true,"result":[
            {"update_id": 1, "message": {"chat": {"id": 42, "type": "private", "first_name": "Олена", "last_name": "К"}}},
            {"update_id": 2, "message": {"chat": {"id": -100, "type": "group", "title": "Сім'я"}}},
            {"update_id": 3, "my_chat_member": {"chat": {"id": -200, "type": "supergroup", "title": "Будинок"}}},
            {"update_id": 4, "channel_post": {"chat": {"id": -300, "type": "channel", "title": "Канал"}}},
            {"update_id": 5, "message": {"chat": {"id": 42, "type": "private", "first_name": "Олена", "last_name": "К"}}},
            {"update_id": 6, "message": {"chat": {"id": 7, "type": "private", "username": "petro"}}},
            {"update_id": 7, "edited_message": {"chat": {"id": 8, "type": "private", "first_name": "Іван"}}}
        ]}"#;
        assert_eq!(
            read_chats(200, &mut json.as_bytes()),
            Ok(vec![
                Chat { id: 8, thread_id: None, title: "Іван".to_string() },
                Chat { id: 7, thread_id: None, title: "@petro".to_string() },
                Chat { id: 42, thread_id: None, title: "Олена К".to_string() },
                Chat { id: -300, thread_id: None, title: "Канал".to_string() },
                Chat { id: -200, thread_id: None, title: "Будинок".to_string() },
                Chat { id: -100, thread_id: None, title: "Сім'я".to_string() },
            ])
        );
    }

    #[test]
    fn forum_topics_are_separate_destinations() {
        let json = r#"{"ok":true,"result":[
            {"update_id": 1, "message": {"chat": {"id": -500, "type": "supergroup", "title": "Будинок", "is_forum": true}, "message_thread_id": 7, "is_topic_message": true,
                "reply_to_message": {"message_thread_id": 7, "forum_topic_created": {"name": "Світло"}}}},
            {"update_id": 2, "message": {"chat": {"id": -500, "type": "supergroup", "title": "Будинок", "is_forum": true}}},
            {"update_id": 3, "message": {"chat": {"id": -500, "type": "supergroup", "title": "Будинок", "is_forum": true}, "message_thread_id": 9, "is_topic_message": true}},
            {"update_id": 4, "message": {"chat": {"id": -100, "type": "supergroup", "title": "Сім'я"},
                "message_thread_id": 3, "reply_to_message": {"message_id": 3}}},
            {"update_id": 5, "message": {"chat": {"id": -500, "type": "supergroup", "title": "Будинок", "is_forum": true}, "message_thread_id": 7, "is_topic_message": true}}
        ]}"#;
        assert_eq!(
            read_chats(200, &mut json.as_bytes()),
            Ok(vec![
                Chat { id: -500, thread_id: Some(7), title: "Будинок › Світло".to_string() },
                Chat { id: -100, thread_id: None, title: "Сім'я".to_string() },
                Chat { id: -500, thread_id: Some(9), title: "Будинок › гілка 9".to_string() },
                Chat { id: -500, thread_id: None, title: "Будинок › General".to_string() },
            ])
        );
    }

    #[test]
    fn topic_names_belong_to_their_forum() {
        let json = r#"{"ok":true,"result":[
            {"update_id": 1, "message": {"chat": {"id": -500, "type": "supergroup", "title": "Будинок", "is_forum": true}, "message_thread_id": 7, "is_topic_message": true,
                "reply_to_message": {"message_thread_id": 7, "forum_topic_created": {"name": "Світло"}}}},
            {"update_id": 2, "message": {"chat": {"id": -600, "type": "supergroup", "title": "Робота", "is_forum": true}, "message_thread_id": 7, "is_topic_message": true}}
        ]}"#;
        let chats = read_chats(200, &mut json.as_bytes()).unwrap();
        assert_eq!(chats[0].title, "Робота › гілка 7");
    }

    #[test]
    fn deleted_topic_is_explained() {
        assert_eq!(
            read_reply(400, &mut br#"{"ok":false,"description":"Bad Request: message thread not found"}"#.as_slice()),
            Err("Гілку форуму не знайдено — можливо, її видалено. Оберіть чат заново".to_string())
        );
    }

    #[test]
    fn updates_without_chats_are_skipped() {
        assert_eq!(read_chats(200, &mut br#"{"ok":true,"result":[{"update_id": 1, "poll": {}}]}"#.as_slice()), Ok(vec![]));
        assert_eq!(read_chats(200, &mut br#"{"ok":true,"result":[]}"#.as_slice()), Ok(vec![]));
    }

    #[test]
    fn a_topic_edit_without_a_name_does_not_spoil_the_list() {
        // Changing only a topic's icon sends `forum_topic_edited` without `name`.
        let json = r#"{"ok":true,"result":[{"update_id":1,"message":{"chat":{"id":-5,"title":"Дім","is_forum":true},
            "is_topic_message":true,"message_thread_id":3,"forum_topic_edited":{"icon_custom_emoji_id":"1"}}}]}"#;
        let chats = read_chats(200, &mut json.as_bytes()).unwrap();
        assert_eq!(chats, vec![Chat { id: -5, thread_id: Some(3), title: "Дім › гілка 3".to_string() }]);
    }
}
