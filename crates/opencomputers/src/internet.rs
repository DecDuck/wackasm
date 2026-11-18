//! HTTP(S) requests and TCP connections through an internet card.
//!
//! ```no_run
//! use std::io::Read;
//! use opencomputers::internet;
//!
//! let mut response = internet::get("https://example.com/")?;
//! println!("{} {}", response.status(), response.message());
//! let mut body = String::new();
//! response.read_to_string(&mut body)?;
//!
//! // Or build the request.
//! let response = internet::Request::new("https://httpbin.org/post")
//!     .method("POST")
//!     .header("Content-Type", "application/json")
//!     .body(r#"{"hello": "world"}"#)
//!     .send()?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```
//!
//! The card does the networking on the server. HTTP and TCP have to be
//! enabled in OpenComputers' config (they are by default), which also limits
//! which hosts can be reached; by default, local and private addresses are
//! blocked. Reads come in chunks of at most 2 KiB (`maxReadBuffer`), one per
//! server tick, so expect about 40 KB/s.
//!
//! [`Response`] and [`TcpStream`] are `std::io` streams. Dropping one closes
//! the connection and releases it on the host.

use std::io;

use crate::{
    component::{self, Address, Component},
    components::Internet,
    error::{Error, Result},
    value::{Handle, Value},
};

/// The most a single `read` returns (OpenComputers' default `maxReadBuffer`).
const CHUNK: i64 = 2048;

/// The internet card to use: `card`, or the first one.
fn card(card: Option<&Address>) -> Result<Internet> {
    match card {
        Some(address) => Ok(Internet::from_address(address.clone())),
        None => Internet::first().ok_or_else(|| Error::Failed("no internet card".into())),
    }
}

/// A request or socket object on the host.
#[derive(Debug)]
struct Connection {
    handle: Handle,
    eof: bool,
}

impl Connection {
    fn call(&self, method: &str, args: Vec<Value>) -> Result<Vec<Value>> {
        component::invoke_value(self.handle, method, args)
    }

    /// Waits until the connection is up; errors if it failed. Each try is an
    /// indirect call, so this waits a tick per try without using fuel.
    fn finish_connect(&self) -> Result<()> {
        loop {
            let reply = self.call("finishConnect", Vec::new())?;
            match reply.first() {
                Some(Value::Bool(true)) => return Ok(()),
                Some(Value::Bool(false)) => {}
                // `nil, reason`
                _ => {
                    let reason = reply.get(1).and_then(Value::as_str).unwrap_or("connection failed");
                    return Err(Error::Failed(reason.to_owned()));
                }
            }
        }
    }

    /// The next chunk of data, waiting for one; empty at the end.
    fn read_chunk(&mut self, max: usize) -> io::Result<Vec<u8>> {
        loop {
            if let Some(chunk) = self.read_once(max)? {
                return Ok(chunk);
            }
        }
    }

    /// One `read` call (a tick): `None` if nothing has arrived yet, empty at
    /// the end.
    fn read_once(&mut self, max: usize) -> io::Result<Option<Vec<u8>>> {
        if self.eof {
            return Ok(Some(Vec::new()));
        }
        let n = (max as i64).clamp(1, CHUNK);
        let reply = self.call("read", vec![Value::from(n)]).map_err(io::Error::other)?;
        match reply.first() {
            Some(Value::String(bytes)) if !bytes.0.is_empty() => Ok(Some(bytes.0.clone())),
            Some(Value::String(_)) => Ok(None),
            None | Some(Value::Nil) => {
                self.eof = true;
                match reply.get(1).and_then(Value::as_str) {
                    Some(reason) => Err(io::Error::other(reason.to_owned())),
                    None => Ok(Some(Vec::new())),
                }
            }
            Some(other) => Err(io::Error::other(format!("unexpected read result {other:?}"))),
        }
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self.call("close", Vec::new());
        component::release_value(self.handle);
    }
}

/// Bytes left over from a chunk that didn't fit the caller's buffer.
#[derive(Debug, Default)]
struct Pending {
    data: Vec<u8>,
    pos: usize,
}

fn read_buffered(conn: &mut Connection, pending: &mut Pending, buf: &mut [u8]) -> io::Result<usize> {
    if buf.is_empty() {
        return Ok(0);
    }
    if pending.pos >= pending.data.len() {
        pending.data = conn.read_chunk(buf.len())?;
        pending.pos = 0;
    }
    let n = (pending.data.len() - pending.pos).min(buf.len());
    buf[..n].copy_from_slice(&pending.data[pending.pos..pending.pos + n]);
    pending.pos += n;
    Ok(n)
}

/// An HTTP request to send.
#[derive(Clone, Debug)]
pub struct Request {
    url: String,
    method: Option<String>,
    headers: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    card: Option<Address>,
}

impl Request {
    /// A `GET` of `url` (`http://` or `https://`).
    pub fn new(url: impl Into<String>) -> Request {
        Request { url: url.into(), method: None, headers: Vec::new(), body: None, card: None }
    }

    /// The HTTP method. Without one it's `GET`, or `POST` if there's a body.
    pub fn method(mut self, method: impl Into<String>) -> Request {
        self.method = Some(method.into());
        self
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Request {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn body(mut self, body: impl Into<Vec<u8>>) -> Request {
        self.body = Some(body.into());
        self
    }

    /// Use this internet card instead of the first one.
    pub fn card(mut self, card: impl Into<Address>) -> Request {
        self.card = Some(card.into());
        self
    }

    /// Sends the request and waits for the status line and headers. Any
    /// status, 404 included, is a `Response`; errors are for requests that
    /// failed (unknown host, refused, blocked by the config, ...).
    pub fn send(&self) -> Result<Response> {
        let card = card(self.card.as_ref())?;
        let headers = (!self.headers.is_empty()).then(|| {
            Value::Map(self.headers.iter().map(|(k, v)| (Value::from(k.as_str()), Value::from(v.as_str()))).collect())
        });
        let args = vec![
            Value::from(self.url.as_str()),
            self.body.as_deref().map_or(Value::Nil, Value::text),
            headers.unwrap_or(Value::Nil),
            self.method.as_deref().map_or(Value::Nil, Value::from),
        ];
        let reply = card.invoke("request", args)?;
        let handle = match reply.first() {
            Some(Value::Handle(handle)) => *handle,
            _ => {
                let reason = reply.get(1).and_then(Value::as_str).unwrap_or("request failed");
                return Err(Error::Failed(reason.to_owned()));
            }
        };
        let conn = Connection { handle, eof: false };
        conn.finish_connect()?;
        // The response line is there once connected, but allow for a lag.
        loop {
            let reply = conn.call("response", Vec::new())?;
            if let Some(status) = reply.first().and_then(Value::as_i64) {
                let message = reply.get(1).and_then(Value::as_str).unwrap_or_default().to_owned();
                let headers = reply.get(2).map(parse_headers).unwrap_or_default();
                return Ok(Response { status: status as u16, message, headers, conn, pending: Pending::default() });
            }
            // `response` is a direct call: once this tick's call budget is
            // spent, the machine sleeps until the next one.
        }
    }
}

/// `{name = {value, ...}}`, skipping Java's null key for the status line.
fn parse_headers(value: &Value) -> Vec<(String, String)> {
    let mut headers = Vec::new();
    for (name, values) in value.as_map().unwrap_or_default() {
        let Some(name) = name.as_str() else { continue };
        match values {
            Value::String(_) => headers.push((name.to_owned(), values.as_str().unwrap_or_default().to_owned())),
            _ => {
                for v in values.as_array().unwrap_or_default() {
                    headers.push((name.to_owned(), v.as_str().unwrap_or_default().to_owned()));
                }
            }
        }
    }
    headers
}

/// A shortcut for `Request::new(url).send()`.
pub fn get(url: impl Into<String>) -> Result<Response> {
    Request::new(url).send()
}

/// An HTTP response. Reading it reads the body.
#[derive(Debug)]
pub struct Response {
    status: u16,
    message: String,
    headers: Vec<(String, String)>,
    conn: Connection,
    pending: Pending,
}

impl Response {
    /// The status code, such as 200.
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The reason phrase, such as `OK`.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Every header, in the order the server sent them (repeated names
    /// appear once per value).
    pub fn headers(&self) -> &[(String, String)] {
        &self.headers
    }

    /// The first value of the header `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

impl io::Read for Response {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_buffered(&mut self.conn, &mut self.pending, buf)
    }
}

/// A TCP connection.
#[derive(Debug)]
pub struct TcpStream {
    conn: Connection,
    pending: Pending,
}

impl TcpStream {
    /// Connects to `host` on `port` with the first internet card, and waits
    /// until the connection is up.
    pub fn connect(host: &str, port: u16) -> Result<TcpStream> {
        TcpStream::connect_with(&card(None)?, host, port)
    }

    pub fn connect_with(card: &Internet, host: &str, port: u16) -> Result<TcpStream> {
        let handle = card.connect(host, Some(i64::from(port).into()))?;
        let conn = Connection { handle, eof: false };
        conn.finish_connect()?;
        Ok(TcpStream { conn, pending: Pending::default() })
    }

    /// Reads what has arrived without waiting for more: `Ok(None)` if
    /// nothing has, `Ok(Some(0))` once the other side has closed. Each call
    /// takes a server tick.
    pub fn try_read(&mut self, buf: &mut [u8]) -> io::Result<Option<usize>> {
        if self.pending.pos >= self.pending.data.len() {
            match self.conn.read_once(buf.len())? {
                Some(chunk) => self.pending = Pending { data: chunk, pos: 0 },
                None => return Ok(None),
            }
        }
        read_buffered(&mut self.conn, &mut self.pending, buf).map(Some)
    }
}

impl io::Read for TcpStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        read_buffered(&mut self.conn, &mut self.pending, buf)
    }
}

impl io::Write for TcpStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            let reply = self.conn.call("write", vec![Value::text(buf)]).map_err(io::Error::other)?;
            match reply.first().and_then(Value::as_i64) {
                // 0: the socket isn't writable yet; `write` is indirect, so
                // trying again waits a tick.
                Some(0) => {}
                Some(n) => return Ok(n as usize),
                None => {
                    let reason = reply.get(1).and_then(Value::as_str).unwrap_or("write failed");
                    return Err(io::Error::other(reason.to_owned()));
                }
            }
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
