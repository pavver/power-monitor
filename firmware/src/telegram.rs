//! HTTPS calls to the Telegram Bot API. Nothing is held whole in memory:
//! a request body is written as it is made, in small pieces, and a reply is
//! parsed as it is read, keeping only the fields asked for.

use std::fmt;
use std::io::BufReader;
use std::time::Duration;

use embedded_svc::http::client::{Client, Response};
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use power_core::json::Count;
use power_core::settings::{Bot, BotToken, Destination};
use power_core::telegram::{self, Chat};
pub use power_core::telegram::Outcome;

/// Pieces a body is written and a reply is read in.
const PIECE: usize = 256;

type HttpClient = Client<EspHttpConnection>;

pub fn send(bot: &Bot, text: &str) -> Result<i64, String> {
    send_to(bot.token(), bot.destination(), text)
}

#[allow(dead_code)]
pub fn send_with(bot: &Bot, write_text: impl Fn(&mut dyn fmt::Write) -> fmt::Result) -> Result<i64, String> {
    send_to_with(bot.token(), bot.destination(), write_text)
}

pub fn send_to(token: &BotToken, to: Destination, text: &str) -> Result<i64, String> {
    send_to_with(token, to, |out| out.write_str(text))
}

pub fn send_to_with(
    token: &BotToken,
    to: Destination,
    write_text: impl Fn(&mut dyn fmt::Write) -> fmt::Result,
) -> Result<i64, String> {
    let body = |out: &mut dyn fmt::Write| telegram::write_send_message_with(to, &write_text, &mut Dyn(out));
    let reply = post(&telegram::send_message_url(token), body)?;
    telegram::message_id(&reply)
}

#[allow(dead_code)]
pub fn edit(bot: &Bot, message_id: i64, text: &str) -> Result<Outcome, String> {
    edit_with(bot, message_id, |out| out.write_str(text))
}

#[allow(dead_code)]
pub fn edit_with(
    bot: &Bot,
    message_id: i64,
    write_text: impl Fn(&mut dyn fmt::Write) -> fmt::Result,
) -> Result<Outcome, String> {
    let body = |out: &mut dyn fmt::Write| {
        telegram::write_edit_message_with(bot.destination(), message_id, &write_text, &mut Dyn(out))
    };
    let reply = post(&telegram::edit_message_url(bot.token()), body);
    telegram::edit_outcome(reply)
}

#[allow(dead_code)]
pub fn delete(bot: &Bot, message_id: i64) -> Result<Outcome, String> {
    let body = |out: &mut dyn fmt::Write| {
        telegram::write_delete_message(bot.destination(), message_id, &mut Dyn(out))
    };
    let reply = post(&telegram::delete_message_url(bot.token()), body);
    telegram::delete_outcome(reply)
}

/// The bot's `@username`, proving the token works.
pub fn bot_username(token: &BotToken) -> Result<String, String> {
    get(&telegram::get_me(token), telegram::read_reply).and_then(|me| telegram::bot_username(&me))
}

pub fn chats(token: &BotToken) -> Result<Vec<Chat>, String> {
    get(&telegram::get_updates(token), telegram::read_chats)
}

fn connect() -> Result<HttpClient, String> {
    let connection = EspHttpConnection::new(&Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    })
    .map_err(unreachable)?;
    Ok(Client::wrap(connection))
}

/// How a reply is read: `telegram::read_reply` or `telegram::read_chats`,
/// from a small buffer filled off the socket.
type ReadReply<T> = fn(u16, &mut dyn std::io::Read) -> Result<T, String>;

fn get<T>(url: &str, read_reply: ReadReply<T>) -> Result<T, String> {
    let mut client = connect()?;
    let response = client.get(url).and_then(|request| request.submit()).map_err(unreachable)?;
    read(response, read_reply)
}

/// POSTs the JSON `body`, written twice: once to count it, once to send it.
fn post(url: &str, body: impl Fn(&mut dyn fmt::Write) -> fmt::Result) -> Result<serde_json::Value, String> {
    let mut count = Count(0);
    body(&mut count).map_err(|_| "Telegram: запит не сформовано".to_string())?;
    let length = count.0.to_string();
    let headers = [("content-type", "application/json"), ("content-length", length.as_str())];
    let mut client = connect()?;
    let response = submit(&mut client, url, &headers, body).map_err(unreachable)?;
    read(response, telegram::read_reply)
}

fn submit<'a>(
    client: &'a mut HttpClient,
    url: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: impl Fn(&mut dyn fmt::Write) -> fmt::Result,
) -> anyhow::Result<Response<&'a mut EspHttpConnection>> {
    let mut request = client.post(url, headers)?;
    let mut pieces = Pieces { out: &mut request, buf: [0; PIECE], len: 0, error: None };
    if body(&mut pieces).is_err() || pieces.flush().is_err() {
        return Err(pieces.error.unwrap_or_else(|| anyhow::anyhow!("запит не сформовано")));
    }
    request.flush()?;
    Ok(request.submit()?)
}

/// Parses the reply as it comes off the socket.
fn read<T>(mut response: Response<&mut EspHttpConnection>, read_reply: ReadReply<T>) -> Result<T, String> {
    let status = response.status();
    read_reply(status, &mut BufReader::with_capacity(PIECE, Stream(&mut response)))
}

fn unreachable(error: impl fmt::Display) -> String {
    format!("немає зв'язку з Telegram ({error})")
}

/// A reply as `std::io::Read`, for serde_json.
struct Stream<'a, R>(&'a mut R);

impl<R: Read> std::io::Read for Stream<'_, R>
where
    R::Error: fmt::Debug,
{
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(buf).map_err(|e| std::io::Error::other(format!("{e:?}")))
    }
}

/// Collects text into `PIECE`-byte pieces and writes each to `out`.
struct Pieces<'a, W> {
    out: &'a mut W,
    buf: [u8; PIECE],
    len: usize,
    error: Option<anyhow::Error>,
}

impl<W: Write> Pieces<'_, W>
where
    W::Error: fmt::Debug,
{
    fn flush(&mut self) -> fmt::Result {
        if self.len > 0 {
            if let Err(e) = self.out.write_all(&self.buf[..self.len]) {
                self.error = Some(anyhow::anyhow!("{e:?}"));
                return Err(fmt::Error);
            }
            self.len = 0;
        }
        Ok(())
    }
}

impl<W: Write> fmt::Write for Pieces<'_, W>
where
    W::Error: fmt::Debug,
{
    fn write_str(&mut self, text: &str) -> fmt::Result {
        // Bytes, not a `str`: a piece may end inside a letter.
        let mut bytes = text.as_bytes();
        while !bytes.is_empty() {
            if self.len == PIECE {
                self.flush()?;
            }
            let take = (PIECE - self.len).min(bytes.len());
            self.buf[self.len..self.len + take].copy_from_slice(&bytes[..take]);
            self.len += take;
            bytes = &bytes[take..];
        }
        Ok(())
    }
}

/// `&mut dyn fmt::Write` where a sized writer is wanted.
struct Dyn<'a>(&'a mut dyn fmt::Write);

impl fmt::Write for Dyn<'_> {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        self.0.write_str(text)
    }
}
