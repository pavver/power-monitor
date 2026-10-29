//! `application/x-www-form-urlencoded`: HTML form posts, Telegram request
//! bodies and the stored configuration all use it.

/// Splits `a=1&b=2` into decoded pairs. Keys without `=` get an empty value.
pub fn parse(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let (key, value) = part.split_once('=').unwrap_or((part, ""));
            (decode(key), decode(value))
        })
        .collect()
}

/// Joins pairs into an encoded body.
pub fn encode(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(key, value)| format!("{}={}", encode_component(key), encode_component(value)))
        .collect::<Vec<_>>()
        .join("&")
}

fn decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match (bytes[i], escaped_byte(&bytes[i..])) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (b'+', _) => {
                out.push(b' ');
                i += 1;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The byte encoded by a `%XX` at the start of `bytes`.
fn escaped_byte(bytes: &[u8]) -> Option<u8> {
    let digits = bytes.get(1..3)?;
    if !digits.iter().all(u8::is_ascii_hexdigit) {
        return None;
    }
    u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()
}

fn encode_component(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'*' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn parses_plain_pairs() {
        assert_eq!(parse("a=1&b=two"), pairs(&[("a", "1"), ("b", "two")]));
    }

    #[test]
    fn decodes_plus_and_percent_escapes() {
        assert_eq!(
            parse("ssid=My+Wi%2DFi&name=%D0%94%D1%96%D0%BC"),
            pairs(&[("ssid", "My Wi-Fi"), ("name", "Дім")])
        );
    }

    #[test]
    fn tolerates_empty_parts_and_missing_values() {
        assert_eq!(parse(""), pairs(&[]));
        assert_eq!(parse("a&&b="), pairs(&[("a", ""), ("b", "")]));
    }

    #[test]
    fn keeps_broken_escapes_literally() {
        assert_eq!(parse("a=100%&b=%zz"), pairs(&[("a", "100%"), ("b", "%zz")]));
        assert_eq!(parse("a=%+1"), pairs(&[("a", "% 1")]));
    }

    #[test]
    fn encodes_reserved_and_non_ascii_characters() {
        assert_eq!(
            encode(&[("text", "a b&c=d+é"), ("id", "-100")]),
            "text=a+b%26c%3Dd%2B%C3%A9&id=-100"
        );
    }

    #[test]
    fn encode_then_parse_returns_the_original() {
        let original = [("k", "Світло з'явилось.\nНе було 5 хв."), ("x", "%+&=")];
        assert_eq!(parse(&encode(&original)), pairs(&original));
    }
}
