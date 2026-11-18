//! Calls to the server's admin API.

use serde_json::Value;

/// What a call came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Reply {
    Ok(Value),
    /// The admin has to log in (again).
    Login,
    /// The server's reason, or the link's; `None` when nobody gave one.
    Failed(Option<String>),
}

impl Reply {
    /// The answer with `status` and JSON `body`.
    pub fn of(status: u16, body: Value) -> Self {
        match status {
            200 => Reply::Ok(body),
            401 if body.get("login").and_then(Value::as_bool) == Some(true) => Reply::Login,
            _ => Reply::Failed(
                body.get("error")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            ),
        }
    }
}

pub async fn get(path: &str) -> Reply {
    let request = gloo_net::http::Request::get(path).cache(web_sys::RequestCache::NoStore);
    answer(request.send().await).await
}

pub async fn post(path: &str, params: &[(&str, &str)]) -> Reply {
    let request = gloo_net::http::Request::post(path)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(form(params));
    match request {
        Ok(request) => answer(request.send().await).await,
        Err(error) => Reply::Failed(Some(error.to_string())),
    }
}

async fn answer(sent: Result<gloo_net::http::Response, gloo_net::Error>) -> Reply {
    let Ok(response) = sent else {
        return Reply::Failed(Some("Немає зв'язку із сервером".to_string()));
    };
    match response.json().await {
        Ok(body) => Reply::of(response.status(), body),
        Err(_) => Reply::Failed(Some("Сервер не відповів".to_string())),
    }
}

/// `params` as `application/x-www-form-urlencoded`.
pub fn form(params: &[(&str, &str)]) -> String {
    params
        .iter()
        .map(|(key, value)| format!("{}={}", url_encode(key), url_encode(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(b as char)
            }
            b' ' => encoded.push('+'),
            _ => encoded.push_str(&format!("%{b:02X}")),
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn replies_are_read_by_status_and_body() {
        assert_eq!(
            Reply::of(200, json!({"id": "x"})),
            Reply::Ok(json!({"id": "x"}))
        );
        assert_eq!(Reply::of(401, json!({"login": true})), Reply::Login);
        assert_eq!(Reply::of(401, json!({})), Reply::Failed(None));
        assert_eq!(
            Reply::of(404, json!({"error": "Немає"})),
            Reply::Failed(Some("Немає".into()))
        );
        assert_eq!(Reply::of(500, json!(null)), Reply::Failed(None));
    }

    #[test]
    fn forms_are_url_encoded() {
        assert_eq!(form(&[]), "");
        assert_eq!(form(&[("on", "1")]), "on=1");
        assert_eq!(
            form(&[("a b", "Дім&x"), ("c", "~._-")]),
            "a+b=%D0%94%D1%96%D0%BC%26x&c=~._-"
        );
    }
}
