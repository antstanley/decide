//! A hand-rolled HTTP/1.1 server for the transport tests.
//!
//! No mock framework and no HTTP server dependency: `std::net::TcpListener` and a thread
//! are the whole thing, which keeps the manifest small and means the client is exercised
//! against real bytes on a real socket.
//!
//! Two properties matter:
//!
//! - **The assertion is on the request the server received**, recorded in
//!   [`FakeServer::seen`], not only on what the client parsed. "The body contained the
//!   criteria I gave" and "the client says it sent them" are different claims.
//! - **A server with no replies left answers `500`**, so a client that retries when it
//!   should not fails loudly instead of hanging.
//!
//! Every item here is used by *some* test crate but not by all of them, and each crate
//! compiles this module separately, so the unused ones are allowed rather than duplicated.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]

use std::collections::BTreeMap;
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// One canned response, in order.
#[derive(Debug, Clone)]
pub struct Reply {
    /// The status line's code.
    pub status: u16,
    /// Extra headers, in addition to `Content-Length` and `Connection: close`.
    pub headers: Vec<(String, String)>,
    /// The body, verbatim.
    pub body: String,
}

impl Reply {
    /// A `200` with a JSON body.
    pub fn json(body: &str) -> Self {
        Self {
            status: 200,
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
            body: body.to_string(),
        }
    }

    /// A status with a body.
    pub fn status(status: u16, body: &str) -> Self {
        Self {
            status,
            headers: Vec::new(),
            body: body.to_string(),
        }
    }

    /// Add one header, for a `Retry-After` or the like.
    #[must_use]
    pub fn with_header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }
}

/// One request the server saw, recorded before the reply is written.
#[derive(Debug, Clone)]
pub struct Request {
    /// `GET`, `POST`, and the like.
    pub method: String,
    /// The path, without the query string.
    pub path: String,
    /// The headers, lower-cased by name.
    pub headers: BTreeMap<String, String>,
    /// The body, read to `Content-Length`.
    pub body: String,
}

impl Request {
    /// One header, by lower-case name.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).map(String::as_str)
    }
}

/// What the server does with each connection.
enum Behaviour {
    /// Answer the Nth request with the Nth reply; `500` when they run out.
    Replies(Vec<Reply>),
    /// Accept and never answer, which is what a timeout test needs.
    Silent,
}

/// A server that records every request it sees and answers from a script.
pub struct FakeServer {
    addr: SocketAddr,
    seen: Arc<Mutex<Vec<Request>>>,
}

impl FakeServer {
    /// Start a server that answers the first N requests from `replies`.
    #[must_use]
    pub fn start(replies: Vec<Reply>) -> Self {
        Self::start_with(Behaviour::Replies(replies))
    }

    /// Start a server that accepts connections and never answers.
    #[must_use]
    pub fn start_silent() -> Self {
        Self::start_with(Behaviour::Silent)
    }

    fn start_with(behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        // Published after the bind, so no test races the port.
        let addr = listener.local_addr().expect("read the bound address");
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);

        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let request = read_request(&stream);
                let index = {
                    let mut seen = recorder.lock().expect("the recorder is not poisoned");
                    seen.push(request);
                    seen.len().saturating_sub(1)
                };
                match &behaviour {
                    Behaviour::Replies(replies) => {
                        let reply = replies
                            .get(index)
                            .cloned()
                            .unwrap_or_else(|| Reply::status(500, "no reply was scripted"));
                        write_reply(&mut stream, &reply);
                    }
                    Behaviour::Silent => {
                        // Hold the connection open with no answer, so the client's own
                        // timeout is what ends the attempt.
                        thread::sleep(Duration::from_secs(30));
                    }
                }
            }
        });

        Self { addr, seen }
    }

    /// The address the server is listening on.
    #[must_use]
    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// The base URL to hand the client, with no trailing slash.
    #[must_use]
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Every request seen so far, in order.
    #[must_use]
    pub fn seen(&self) -> Vec<Request> {
        self.seen
            .lock()
            .expect("the recorder is not poisoned")
            .clone()
    }

    /// How many requests have been seen so far.
    #[must_use]
    pub fn count(&self) -> usize {
        self.seen
            .lock()
            .expect("the recorder is not poisoned")
            .len()
    }
}

/// Read one request: the request line, the headers, and `Content-Length` bytes of body.
fn read_request(stream: &TcpStream) -> Request {
    let mut reader = BufReader::new(stream.try_clone().expect("clone the accepted socket"));

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .expect("read the request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).expect("read a header line");
        if read == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }

    let length: usize = headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut bytes = vec![0_u8; length];
    if length > 0 {
        reader.read_exact(&mut bytes).expect("read the body");
    }

    Request {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&bytes).into_owned(),
    }
}

/// Write one reply and close, which is what makes the next attempt a new connection.
fn write_reply(stream: &mut TcpStream, reply: &Reply) {
    let mut head = vec![
        format!("HTTP/1.1 {} {}\r\n", reply.status, reason(reply.status)),
        format!("Content-Length: {}\r\n", reply.body.len()),
        "Connection: close\r\n".to_string(),
    ];
    for (name, value) in &reply.headers {
        head.push(format!("{name}: {value}\r\n"));
    }
    head.push("\r\n".to_string());

    let mut bytes = head.join("").into_bytes();
    bytes.extend_from_slice(reply.body.as_bytes());
    let _ = stream.write_all(&bytes);
    let _ = stream.flush();
}

/// The reason phrase for the statuses these tests use.
const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        408 => "Request Timeout",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        529 => "Overloaded",
        _ => "Unknown",
    }
}
