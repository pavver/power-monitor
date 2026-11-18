//! Reads JSON as it comes off a socket, a byte at a time, keeping only what
//! is asked for: values under unwanted keys are skipped without being stored,
//! so a reply full of long message texts takes no more memory than an empty
//! one. Small on purpose: serde_json's own reader for a stream costs the
//! firmware ~10 KB of flash.

use std::io::Read;

use serde_json::{Map, Value};

/// Deeper nesting is refused rather than recursed into.
const MAX_DEPTH: usize = 32;
/// Longer keys are none we look for; they read as "".
const MAX_KEY: usize = 64;
/// Numbers are short; a longer run of digits is not JSON we expect.
const MAX_NUMBER: usize = 40;

/// The input is not JSON, or ended early.
#[derive(Debug, PartialEq, Eq)]
pub struct Bad;

pub struct Reader<'a> {
    input: &'a mut dyn Read,
    peeked: Option<u8>,
    depth: usize,
}

impl<'a> Reader<'a> {
    pub fn new(input: &'a mut dyn Read) -> Self {
        Self { input, peeked: None, depth: 0 }
    }

    /// Calls `field` with each key of an object; it must read or skip the value.
    pub fn object(&mut self, field: &mut dyn FnMut(&mut Self, &str) -> Result<(), Bad>) -> Result<(), Bad> {
        self.expect(b'{')?;
        self.nested(|r| {
            if r.after_ws()? == b'}' {
                return r.next().map(drop);
            }
            loop {
                r.expect(b'"')?;
                let key = r.string(Some(MAX_KEY))?.unwrap_or_default();
                r.expect(b':')?;
                field(r, &key)?;
                match r.after_ws_next()? {
                    b',' => continue,
                    b'}' => return Ok(()),
                    _ => return Err(Bad),
                }
            }
        })
    }

    /// Calls `item` for each element of an array; it must read or skip it.
    pub fn array(&mut self, item: &mut dyn FnMut(&mut Self) -> Result<(), Bad>) -> Result<(), Bad> {
        self.expect(b'[')?;
        self.nested(|r| {
            if r.after_ws()? == b']' {
                return r.next().map(drop);
            }
            loop {
                item(r)?;
                match r.after_ws_next()? {
                    b',' => continue,
                    b']' => return Ok(()),
                    _ => return Err(Bad),
                }
            }
        })
    }

    /// The next value with only the `kept` keys of its objects, the rest
    /// skipped; a string longer than `max_string` bytes becomes null.
    pub fn value(&mut self, kept: &dyn Fn(&str) -> bool, max_string: usize) -> Result<Value, Bad> {
        Ok(match self.after_ws()? {
            b'{' => {
                let mut object = Map::new();
                self.object(&mut |r, key| {
                    if kept(key) {
                        object.insert(key.to_string(), r.value(kept, max_string)?);
                        Ok(())
                    } else {
                        r.skip()
                    }
                })?;
                Value::Object(object)
            }
            b'[' => {
                let mut items = Vec::new();
                self.array(&mut |r| r.value(kept, max_string).map(|item| items.push(item)))?;
                Value::Array(items)
            }
            b'"' => {
                self.next()?;
                self.string(Some(max_string))?.map_or(Value::Null, Value::String)
            }
            _ => self.scalar()?,
        })
    }

    /// Reads past the next value, storing nothing.
    pub fn skip(&mut self) -> Result<(), Bad> {
        match self.after_ws()? {
            b'{' => self.object(&mut |r, _| r.skip()),
            b'[' => self.array(&mut |r| r.skip()),
            b'"' => {
                self.next()?;
                self.string(None).map(drop)
            }
            _ => self.scalar().map(drop),
        }
    }

    /// Only whitespace is left.
    pub fn end(&mut self) -> Result<(), Bad> {
        match self.skip_ws()? {
            None => Ok(()),
            Some(_) => Err(Bad),
        }
    }

    fn nested(&mut self, read: impl FnOnce(&mut Self) -> Result<(), Bad>) -> Result<(), Bad> {
        if self.depth == MAX_DEPTH {
            return Err(Bad);
        }
        self.depth += 1;
        let result = read(self);
        self.depth -= 1;
        result
    }

    fn scalar(&mut self) -> Result<Value, Bad> {
        match self.after_ws()? {
            b't' => self.word(b"true", Value::Bool(true)),
            b'f' => self.word(b"false", Value::Bool(false)),
            b'n' => self.word(b"null", Value::Null),
            b'-' | b'0'..=b'9' => self.number(),
            _ => Err(Bad),
        }
    }

    fn word(&mut self, word: &[u8], value: Value) -> Result<Value, Bad> {
        for &b in word {
            if self.next()? != b {
                return Err(Bad);
            }
        }
        Ok(value)
    }

    fn number(&mut self) -> Result<Value, Bad> {
        let mut digits = [0u8; MAX_NUMBER];
        let mut len = 0;
        while let Some(b) = self.peek()? {
            if !matches!(b, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') {
                break;
            }
            *digits.get_mut(len).ok_or(Bad)? = b;
            len += 1;
            self.peeked = None;
        }
        let text = std::str::from_utf8(&digits[..len]).map_err(|_| Bad)?;
        if let Ok(n) = text.parse::<i64>() {
            return Ok(Value::from(n));
        }
        if let Ok(n) = text.parse::<u64>() {
            return Ok(Value::from(n));
        }
        // No kept value is a fraction, and parsing one would bring the float
        // parser (some KB) into the firmware: read past it as null.
        let digits = text.strip_prefix('-').unwrap_or(text);
        let number = digits.bytes().next().is_some_and(|b| b.is_ascii_digit()) && !digits.ends_with(['.', 'e', 'E', '-', '+']);
        if number { Ok(Value::Null) } else { Err(Bad) }
    }

    /// The rest of a string after its opening quote: kept when `max` is
    /// given and it fits, else only read past (`None`).
    fn string(&mut self, max: Option<usize>) -> Result<Option<String>, Bad> {
        let mut kept = max.map(|_| Vec::new());
        loop {
            let b = self.next()?;
            let mut utf8 = [0u8; 4];
            let piece: &[u8] = match b {
                b'"' => break,
                b'\\' => match self.next()? {
                    b'u' => self.escaped_char()?.encode_utf8(&mut utf8).as_bytes(),
                    b'n' => b"\n",
                    b't' => b"\t",
                    b'r' => b"\r",
                    b'b' => b"\x08",
                    b'f' => b"\x0c",
                    b @ (b'"' | b'\\' | b'/') => {
                        utf8[0] = b;
                        &utf8[..1]
                    }
                    _ => return Err(Bad),
                },
                _ => {
                    utf8[0] = b;
                    &utf8[..1]
                }
            };
            if let (Some(bytes), Some(max)) = (kept.as_mut(), max) {
                if bytes.len() + piece.len() > max {
                    kept = None;
                } else {
                    bytes.extend_from_slice(piece);
                }
            }
        }
        // Too long to keep is still a string, read to its end.
        kept.map(|bytes| String::from_utf8(bytes).map_err(|_| Bad)).transpose()
    }

    /// `\uXXXX`, with the second half of a surrogate pair if one follows.
    fn escaped_char(&mut self) -> Result<char, Bad> {
        let high = self.hex4()?;
        if !(0xD800..0xDC00).contains(&high) {
            return char::from_u32(high).ok_or(Bad);
        }
        if self.next()? != b'\\' || self.next()? != b'u' {
            return Err(Bad);
        }
        let low = self.hex4()?;
        if !(0xDC00..0xE000).contains(&low) {
            return Err(Bad);
        }
        char::from_u32(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)).ok_or(Bad)
    }

    fn hex4(&mut self) -> Result<u32, Bad> {
        let mut code = 0;
        for _ in 0..4 {
            code = code * 16 + char::from(self.next()?).to_digit(16).ok_or(Bad)?;
        }
        Ok(code)
    }

    fn expect(&mut self, b: u8) -> Result<(), Bad> {
        if self.after_ws_next()? == b {
            Ok(())
        } else {
            Err(Bad)
        }
    }

    /// The next byte that is not whitespace, left unread.
    fn after_ws(&mut self) -> Result<u8, Bad> {
        self.skip_ws()?.ok_or(Bad)
    }

    fn after_ws_next(&mut self) -> Result<u8, Bad> {
        self.after_ws()?;
        self.next()
    }

    fn skip_ws(&mut self) -> Result<Option<u8>, Bad> {
        while let Some(b) = self.peek()? {
            if !matches!(b, b' ' | b'\n' | b'\r' | b'\t') {
                return Ok(Some(b));
            }
            self.peeked = None;
        }
        Ok(None)
    }

    fn peek(&mut self) -> Result<Option<u8>, Bad> {
        if self.peeked.is_none() {
            let mut byte = [0u8];
            loop {
                match self.input.read(&mut byte) {
                    Ok(0) => return Ok(None),
                    Ok(_) => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => return Err(Bad),
                }
            }
            self.peeked = Some(byte[0]);
        }
        Ok(self.peeked)
    }

    fn next(&mut self) -> Result<u8, Bad> {
        let b = self.peek()?.ok_or(Bad)?;
        self.peeked = None;
        Ok(b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEPT: [&str; 3] = ["id", "name", "chat"];

    fn kept(key: &str) -> bool {
        KEPT.contains(&key)
    }

    fn read(json: &str, max: usize) -> Result<Value, Bad> {
        let mut bytes = json.as_bytes();
        let mut reader = Reader::new(&mut bytes);
        let value = reader.value(&kept, max)?;
        reader.end()?;
        Ok(value)
    }

    /// What `read` should give: serde_json's reading with the same keys kept.
    fn pruned(value: Value, max: usize) -> Value {
        match value {
            Value::Object(object) => Value::Object(
                object.into_iter().filter(|(key, _)| kept(key)).map(|(key, value)| (key, pruned(value, max))).collect(),
            ),
            Value::Array(items) => Value::Array(items.into_iter().map(|item| pruned(item, max)).collect()),
            Value::String(text) if text.len() > max => Value::Null,
            Value::Number(n) if n.is_f64() => Value::Null,
            other => other,
        }
    }

    #[test]
    fn keeps_only_the_kept_keys() {
        let json = r#" {"id": -5, "text": "довгий текст {\"id\":1}", "chat": {"name": "Дім", "x": [1, {"id": 2}]},
            "entities": [{"type": "bold"}], "ok": true, "n": null, "f": 1.5e3, "big": 18446744073709551615, "id2": 0.5} "#;
        assert_eq!(read(json, 64), Ok(serde_json::json!({"id": -5, "chat": {"name": "Дім"}})));
        // A fraction is read past as null: no kept value is one.
        assert_eq!(read(r#"{"id": -0.5e-3}"#, 64), Ok(serde_json::json!({"id": null})));
        assert_eq!(read(r#"{"id": 1.}"#, 64), Err(Bad));
    }

    #[test]
    fn reads_escapes_and_surrogate_pairs() {
        let json = r#"{"name": "a\"b\\c\/d\n\tЖ🔋🔋"}"#;
        assert_eq!(read(json, 64), Ok(serde_json::json!({"name": "a\"b\\c/d\n\tЖ🔋🔋"})));
        // The lowest and highest pairs, and a second half that is not one.
        assert_eq!(read(r#""\ud800\udc00\udbff\udfff""#, 64), Ok(Value::from("\u{10000}\u{10FFFF}")));
        assert_eq!(read(r#""\ud83d\u0041""#, 64), Err(Bad));
        assert_eq!(read(r#""\ud83d\ue000""#, 64), Err(Bad));
    }

    #[test]
    fn a_string_too_long_to_keep_is_null_and_the_rest_still_reads() {
        assert_eq!(read(r#"{"name": "абв", "id": 1}"#, 5), Ok(serde_json::json!({"name": null, "id": 1})));
        assert_eq!(read(r#"{"name": "аб", "id": 1}"#, 4), Ok(serde_json::json!({"name": "аб", "id": 1})));
    }

    #[test]
    fn refuses_what_is_not_json() {
        for bad in ["", "<html>", "{", r#"{"id" 1}"#, r#"{"id":1,}"#, "[1 2]", r#""\x""#, r#""\ud83d""#, "tru", "{} x", "--"] {
            assert_eq!(read(bad, 64), Err(Bad), "{bad}");
        }
        assert_eq!(read(&("[".repeat(MAX_DEPTH + 1) + &"]".repeat(MAX_DEPTH + 1)), 64), Err(Bad));
        assert!(read(&("[".repeat(MAX_DEPTH) + &"]".repeat(MAX_DEPTH)), 64).is_ok());
    }

    #[test]
    fn reads_like_serde_json_on_random_documents() {
        let mut seed: u64 = 0x2545F4914F6CDD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..3000 {
            let value = random_value(&mut next, 0);
            let json = serde_json::to_string_pretty(&value).unwrap();
            assert_eq!(read(&json, 8), Ok(pruned(value, 8)), "{json}");
        }
    }

    fn random_value(next: &mut impl FnMut() -> u64, depth: usize) -> Value {
        const KEYS: [&str; 6] = ["id", "name", "chat", "text", "x", "ідентифікатор"];
        const STRINGS: [&str; 7] = ["", "a", "Дім", "\"\\/\n\t\u{1}", "🔋 заряд", "довгий текст повідомлення", "x"];
        match next() % if depth > 4 { 5 } else { 7 } {
            0 => Value::Null,
            1 => Value::Bool(next().is_multiple_of(2)),
            2 => Value::from(next() as i64 >> (next() % 64)),
            3 => Value::from((next() % 100_000) as f64 / 7.0),
            4 => Value::from(STRINGS[(next() % STRINGS.len() as u64) as usize]),
            5 => Value::Array((0..next() % 4).map(|_| random_value(next, depth + 1)).collect()),
            _ => Value::Object(
                (0..next() % 5)
                    .map(|_| (KEYS[(next() % KEYS.len() as u64) as usize].to_string(), random_value(next, depth + 1)))
                    .collect(),
            ),
        }
    }
}
