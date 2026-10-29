//! Optional password for the web page. Only a salted hash is stored; a
//! successful login opens a session identified by a random cookie.

use std::collections::VecDeque;
use std::fmt;

use sha2::{Digest, Sha256};

use crate::form;

const HASH_ROUNDS: u32 = 2_000;
const COOKIE: &str = "session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    Length,
    Mismatch,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            AuthError::Length => "Пароль має бути від 4 до 64 символів.",
            AuthError::Mismatch => "Паролі не збігаються.",
        })
    }
}

/// A password the user typed twice when setting it.
#[derive(Debug, PartialEq, Eq)]
pub struct NewPassword(String);

impl NewPassword {
    /// Reads `password` and `confirm`.
    pub fn from_form(body: &str) -> Result<Self, AuthError> {
        let fields = form::parse(body);
        let password = form::field(&fields, "password");
        if !(4..=64).contains(&password.chars().count()) {
            return Err(AuthError::Length);
        }
        if password != form::field(&fields, "confirm") {
            return Err(AuthError::Mismatch);
        }
        Ok(Self(password.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PasswordHash {
    salt: [u8; 16],
    hash: [u8; 32],
}

impl PasswordHash {
    pub fn new(password: &NewPassword, salt: [u8; 16]) -> Self {
        Self { salt, hash: digest(&salt, &password.0) }
    }

    pub fn verify(&self, attempt: &str) -> bool {
        let candidate = digest(&self.salt, attempt);
        // Compare every byte so timing does not reveal how much matched.
        candidate.iter().zip(&self.hash).fold(0, |diff, (a, b)| diff | (a ^ b)) == 0
    }

    /// `salt$hash` in hex.
    pub fn to_stored(&self) -> String {
        format!("{}${}", hex(&self.salt), hex(&self.hash))
    }

    pub fn from_stored(stored: &str) -> Option<Self> {
        let (salt, hash) = stored.split_once('$')?;
        Some(Self { salt: unhex(salt)?, hash: unhex(hash)? })
    }
}

/// Open sessions; the oldest is dropped when there are too many.
pub struct Sessions {
    tokens: VecDeque<String>,
    capacity: usize,
}

impl Sessions {
    pub fn new(capacity: usize) -> Self {
        Self { tokens: VecDeque::with_capacity(capacity), capacity }
    }

    /// Opens a session and returns its `Set-Cookie` value.
    pub fn open(&mut self, random: [u8; 16]) -> String {
        if self.tokens.len() == self.capacity {
            self.tokens.pop_front();
        }
        let token = hex(&random);
        self.tokens.push_back(token.clone());
        format!("{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict")
    }

    /// Whether the request's `Cookie` header carries an open session.
    pub fn is_open(&self, cookie_header: Option<&str>) -> bool {
        session_token(cookie_header).is_some_and(|token| self.tokens.iter().any(|t| t == token))
    }

    pub fn close(&mut self, cookie_header: Option<&str>) {
        if let Some(token) = session_token(cookie_header) {
            self.tokens.retain(|t| t != token);
        }
    }

    pub fn close_all(&mut self) {
        self.tokens.clear();
    }
}

/// `Set-Cookie` value that removes the session cookie.
pub fn expired_cookie() -> String {
    format!("{COOKIE}=; Path=/; Max-Age=0")
}

fn session_token(cookie_header: Option<&str>) -> Option<&str> {
    cookie_header?
        .split(';')
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value)
}

/// Iterated SHA-256 of salt and password; slows down guessing a stolen hash.
fn digest(salt: &[u8; 16], password: &str) -> [u8; 32] {
    let mut hash: [u8; 32] = Sha256::new().chain_update(salt).chain_update(password).finalize().into();
    for _ in 1..HASH_ROUNDS {
        hash = Sha256::new().chain_update(salt).chain_update(hash).finalize().into();
    }
    hash
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex<const N: usize>(text: &str) -> Option<[u8; N]> {
    if text.len() != N * 2 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut out = [0u8; N];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SALT: [u8; 16] = [7; 16];

    fn new_password(password: &str) -> NewPassword {
        NewPassword::from_form(&form::encode(&[("password", password), ("confirm", password)])).unwrap()
    }

    fn cookie_of(set_cookie: &str) -> String {
        set_cookie.split(';').next().unwrap().to_string()
    }

    #[test]
    fn new_password_must_be_typed_twice_alike() {
        let body = form::encode(&[("password", "secret1"), ("confirm", "secret2")]);
        assert_eq!(NewPassword::from_form(&body), Err(AuthError::Mismatch));
    }

    #[test]
    fn new_password_length_is_checked() {
        for bad in ["", "abc", &"я".repeat(65)] {
            let body = form::encode(&[("password", bad), ("confirm", bad)]);
            assert_eq!(NewPassword::from_form(&body), Err(AuthError::Length), "{bad}");
        }
        assert!(NewPassword::from_form(&form::encode(&[("password", "abcd"), ("confirm", "abcd")])).is_ok());
    }

    #[test]
    fn hash_verifies_only_the_right_password() {
        let hash = PasswordHash::new(&new_password("секрет"), SALT);
        assert!(hash.verify("секрет"));
        assert!(!hash.verify("секрет "));
        assert!(!hash.verify(""));
    }

    #[test]
    fn salt_changes_the_hash() {
        let a = PasswordHash::new(&new_password("secret"), SALT);
        let b = PasswordHash::new(&new_password("secret"), [8; 16]);
        assert_ne!(a.to_stored(), b.to_stored());
    }

    #[test]
    fn stored_form_round_trips_and_hides_the_password() {
        let hash = PasswordHash::new(&new_password("secret"), SALT);
        let stored = hash.to_stored();
        assert!(!stored.contains("secret"));
        assert_eq!(stored.len(), 32 + 1 + 64);
        assert_eq!(PasswordHash::from_stored(&stored), Some(hash));
    }

    #[test]
    fn broken_stored_value_is_rejected() {
        for bad in ["", "abc", "zz$zz", &format!("{}${}", "0".repeat(32), "0".repeat(63))] {
            assert_eq!(PasswordHash::from_stored(bad), None, "{bad}");
        }
    }

    #[test]
    fn opened_session_is_recognised_from_the_cookie_header() {
        let mut sessions = Sessions::new(4);
        let set_cookie = sessions.open([1; 16]);
        assert!(set_cookie.starts_with("session="));
        assert!(set_cookie.contains("HttpOnly"));
        let header = format!("theme=dark; {}", cookie_of(&set_cookie));
        assert!(sessions.is_open(Some(&header)));
        assert!(!sessions.is_open(Some("session=0101")));
        assert!(!sessions.is_open(Some("other=1")));
        assert!(!sessions.is_open(None));
    }

    #[test]
    fn closing_ends_one_or_all_sessions() {
        let mut sessions = Sessions::new(4);
        let a = cookie_of(&sessions.open([1; 16]));
        let b = cookie_of(&sessions.open([2; 16]));
        sessions.close(Some(&a));
        assert!(!sessions.is_open(Some(&a)));
        assert!(sessions.is_open(Some(&b)));
        sessions.close_all();
        assert!(!sessions.is_open(Some(&b)));
    }

    #[test]
    fn too_many_sessions_drop_the_oldest() {
        let mut sessions = Sessions::new(2);
        let first = cookie_of(&sessions.open([1; 16]));
        sessions.open([2; 16]);
        let third = cookie_of(&sessions.open([3; 16]));
        assert!(!sessions.is_open(Some(&first)));
        assert!(sessions.is_open(Some(&third)));
    }
}
