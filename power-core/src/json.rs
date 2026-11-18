//! Escaping JSON string contents and counting output length.

pub mod read;

use std::fmt;

/// Writes JSON string contents without surrounding quotes.
pub struct Escaped<'a, W: fmt::Write>(pub &'a mut W);

impl<'a, W: fmt::Write> fmt::Write for Escaped<'a, W> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let mut start = 0;
        for (i, &b) in bytes.iter().enumerate() {
            let escaped = match b {
                b'"' => "\\\"",
                b'\\' => "\\\\",
                b'\n' => "\\n",
                b'\r' => "\\r",
                b'\t' => "\\t",
                b if b < 0x20 => {
                    if i > start {
                        self.0.write_str(&s[start..i])?;
                    }
                    const HEX: &[u8; 16] = b"0123456789abcdef";
                    let escape = [
                        b'\\',
                        b'u',
                        b'0',
                        b'0',
                        HEX[(b >> 4) as usize],
                        HEX[(b & 0x0f) as usize],
                    ];
                    // SAFETY: All bytes in escape are ASCII, valid UTF-8.
                    let escape_str = std::str::from_utf8(&escape).unwrap();
                    self.0.write_str(escape_str)?;
                    start = i + 1;
                    continue;
                }
                _ => continue,
            };
            if i > start {
                self.0.write_str(&s[start..i])?;
            }
            self.0.write_str(escaped)?;
            start = i + 1;
        }
        if start < s.len() {
            self.0.write_str(&s[start..])?;
        }
        Ok(())
    }
}

/// Counts bytes written into it.
pub struct Count(pub usize);

impl fmt::Write for Count {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 += s.len();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn escape(s: &str) -> String {
        let mut out = String::new();
        let mut escaped = Escaped(&mut out);
        fmt::Write::write_str(&mut escaped, s).unwrap();
        out
    }

    #[test]
    fn escaping_table() {
        assert_eq!(escape("\""), "\\\"");
        assert_eq!(escape("\\"), "\\\\");
        assert_eq!(escape("\n"), "\\n");
        assert_eq!(escape("\r"), "\\r");
        assert_eq!(escape("\t"), "\\t");
        assert_eq!(escape("\x00"), "\\u0000");
        assert_eq!(escape("\x01"), "\\u0001");
        assert_eq!(escape("\x1f"), "\\u001f");
        assert_eq!(escape("a\x01b\"c"), "a\\u0001b\\\"c");
        assert_eq!(escape("plain text 123"), "plain text 123");
        assert_eq!(escape("Слава Україні! 🇺🇦 🔋"), "Слава Україні! 🇺🇦 🔋");
    }

    #[test]
    fn round_trip_with_all_characters() {
        let original = "\x00\x01\x07\t\n\r\x1f \"hello\\world\" Привіт! ⚡🔋";
        let escaped = escape(original);
        let wrapped = format!("\"{escaped}\"");
        let decoded: String = serde_json::from_str(&wrapped).unwrap();
        assert_eq!(decoded, original);
    }

    #[test]
    fn counts_bytes() {
        let mut c = Count(0);
        fmt::Write::write_str(&mut c, "hello").unwrap();
        fmt::Write::write_str(&mut c, " світ").unwrap();
        assert_eq!(c.0, "hello світ".len());
    }
}
