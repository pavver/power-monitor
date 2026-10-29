//! Telegram Bot API request for `sendMessage`.

use crate::form;

pub struct Request {
    pub url: String,
    /// `application/x-www-form-urlencoded` body.
    pub body: String,
}

pub fn send_message(bot_token: &str, chat_id: i64, text: &str) -> Request {
    Request {
        url: format!("https://api.telegram.org/bot{bot_token}/sendMessage"),
        body: form::encode(&[("chat_id", &chat_id.to_string()), ("text", text)]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_a_send_message_request() {
        let request = send_message("1:abc", -100, "Світло є & ok");
        assert_eq!(request.url, "https://api.telegram.org/bot1:abc/sendMessage");
        assert_eq!(
            form::parse(&request.body),
            vec![
                ("chat_id".to_string(), "-100".to_string()),
                ("text".to_string(), "Світло є & ok".to_string()),
            ]
        );
    }
}
