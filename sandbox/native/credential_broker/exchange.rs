//! One intercepted request, carried across: the command's TLS answered with
//! the run's certificate for the host, the head read and rewritten, the
//! request sent on over porta's own TLS to the real server, and the response
//! carried back.
//!
//! Each intercepted connection carries one request: porta asks the server for
//! `Connection: close` and closes the client's side after the response, so a
//! client sends its next request on a fresh connection. That costs a
//! handshake per request to a bound host and buys a proxy that never has to
//! find where one response ends and the next begins.

use super::{rewrite_head, Broker, Rewrite, Target};
use crate::proxy_audit::{audit_log, Decision};
use crate::proxy_egress::dial;
use rustls::pki_types::ServerName;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

type ClientSide = BufReader<rustls::StreamOwned<rustls::ServerConnection, TcpStream>>;

impl Broker {
    /// Takes over a CONNECT to a bound host: answers it, speaks TLS to the
    /// command as the host, and carries one request on to the real server
    /// with the credential put on.
    pub(crate) fn intercept(&self, mut client: TcpStream, audit: &Option<String>, at: Target) {
        let config = match self.leaf_for(at.host) {
            Ok(config) => config,
            Err(reason) => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n");
                return audit_log(audit, &Decision::new(at.host, at.port, "error", reason));
            }
        };
        let _ = client.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n");
        let Ok(connection) = rustls::ServerConnection::new(config) else { return };
        let _ = client.set_read_timeout(Some(Duration::from_secs(120)));
        let mut reader = BufReader::new(rustls::StreamOwned::new(connection, client));
        if let Some((request_line, headers)) = read_head(&mut reader) {
            self.answer(&mut reader, audit, at, &format!("{}\r\n{}\r\n", request_line, headers.join("\r\n")));
        }
        close(reader.get_mut());
    }

    /// The request refused, or sent on with its credentials put on, and the
    /// decision recorded either way.
    fn answer(&self, reader: &mut ClientSide, audit: &Option<String>, at: Target, head: &str) {
        let request_target = head.split_whitespace().nth(1).unwrap_or("/").to_string();
        let path = request_target.split(['?', '#']).next().unwrap_or("/").to_string();
        let record = |verdict: &'static str, reason: String| audit_log(audit, &Decision::new(at.host, at.port, verdict, reason));
        match rewrite_head(&self.credentials, head, at, &request_target) {
            Rewrite::Refuse(name) => {
                let _ = reader.get_mut().write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                record("deny", format!("credential {name} is not bound to {}{path}: the request was not sent", at.host));
            }
            Rewrite::Send(head, used) => {
                if used.is_empty() {
                    record("allow", format!("opened for its credentials; {path} carried none"));
                }
                for name in &used {
                    record("substitute", format!("credential {name} put on for {path}"));
                }
                if let Err(reason) = self.forward(reader, &head, at) {
                    let _ = reader.get_mut().write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    record("error", reason);
                }
            }
        }
    }

    /// Sends the rewritten head and the body after it to the real server, then
    /// the whole response back.
    fn forward(&self, client: &mut ClientSide, head: &str, at: Target) -> Result<(), String> {
        let upstream = dial(at.host, at.port).map_err(|failure| match failure {
            crate::proxy_egress::Dial::Blocked(reason) | crate::proxy_egress::Dial::Failed(reason) => reason,
        })?;
        let _ = upstream.set_read_timeout(Some(Duration::from_secs(300)));
        let name = ServerName::try_from(at.host.to_string()).map_err(|e| format!("{}: {e}", at.host))?;
        let connection = rustls::ClientConnection::new(self.upstream.clone(), name).map_err(|e| format!("upstream TLS: {e}"))?;
        let mut server = rustls::StreamOwned::new(connection, upstream);
        let (lines, framing) = outgoing_head(head);
        server.write_all(lines.as_bytes()).map_err(|e| format!("upstream TLS: {e}"))?;
        copy_body(client, &mut server, framing).map_err(|e| format!("request body: {e}"))?;
        server.flush().map_err(|e| format!("upstream: {e}"))?;
        copy_response(&mut server, client.get_mut())
    }
}

/// The response, to the end of the server's stream.
fn copy_response<R: Read, W: Write>(server: &mut R, client: &mut W) -> Result<(), String> {
    let mut buffer = [0u8; 16384];
    loop {
        match server.read(&mut buffer) {
            Ok(0) => return Ok(()),
            Ok(n) => client.write_all(&buffer[..n]).map_err(|e| format!("client: {e}"))?,
            // A server that closes without close_notify has still sent the
            // whole response: it was asked for `Connection: close`.
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(e) => return Err(format!("upstream read: {e}")),
        }
    }
}

/// How the request's body is framed, from its headers.
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum Framing {
    None,
    Length(u64),
    Chunked,
}

/// The head as it goes to the server: hop-by-hop headers dropped, `Expect`
/// dropped (the body follows at once; a client waiting on 100 Continue sends
/// it after its own short wait), and `Connection: close` asked for.
pub(crate) fn outgoing_head(head: &str) -> (String, Framing) {
    let mut lines = head.split("\r\n").filter(|l| !l.is_empty());
    let mut out = format!("{}\r\n", lines.next().unwrap_or(""));
    let mut framing = Framing::None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else { continue };
        let name = name.trim().to_ascii_lowercase();
        if matches!(name.as_str(), "connection" | "proxy-connection" | "keep-alive" | "expect" | "proxy-authorization") {
            continue;
        }
        framing = framing_after(framing, &name, value);
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    (out, framing)
}

/// Chunked wins over a length, as HTTP/1.1 says it must.
fn framing_after(framing: Framing, name: &str, value: &str) -> Framing {
    match name {
        "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => Framing::Chunked,
        "content-length" if framing != Framing::Chunked => Framing::Length(value.trim().parse().unwrap_or(0)),
        _ => framing,
    }
}

fn copy_body<R: BufRead, W: Write>(from: &mut R, to: &mut W, framing: Framing) -> std::io::Result<()> {
    match framing {
        Framing::None => Ok(()),
        Framing::Length(n) => {
            let copied = std::io::copy(&mut from.take(n), to)?;
            if copied < n { Err(ended_early("body shorter than Content-Length")) } else { Ok(()) }
        }
        Framing::Chunked => copy_chunks(from, to),
    }
}

fn ended_early(what: &str) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::UnexpectedEof, what.to_string())
}

/// Chunks as they come, through the last one and its trailers.
fn copy_chunks<R: BufRead, W: Write>(from: &mut R, to: &mut W) -> std::io::Result<()> {
    loop {
        let mut size_line = String::new();
        if from.read_line(&mut size_line)? == 0 {
            return Err(ended_early("chunked body ended early"));
        }
        to.write_all(size_line.as_bytes())?;
        let size = u64::from_str_radix(size_line.trim().split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 {
            return copy_trailers(from, to);
        }
        std::io::copy(&mut from.take(size + 2), to)?;
    }
}

/// Trailers, then the blank line that ends them.
fn copy_trailers<R: BufRead, W: Write>(from: &mut R, to: &mut W) -> std::io::Result<()> {
    loop {
        let mut line = String::new();
        if from.read_line(&mut line)? == 0 {
            return Ok(());
        }
        to.write_all(line.as_bytes())?;
        if line == "\r\n" || line == "\n" {
            return Ok(());
        }
    }
}

/// The request line and headers, up to 64 KiB of them.
fn read_head<R: BufRead>(reader: &mut R) -> Option<(String, Vec<String>)> {
    let mut request_line = String::new();
    if reader.read_line(&mut request_line).ok()? == 0 {
        return None;
    }
    let mut headers = Vec::new();
    let mut total = request_line.len();
    loop {
        let mut line = String::new();
        let n = reader.read_line(&mut line).ok().filter(|n| *n > 0)?;
        if line == "\r\n" || line == "\n" {
            return Some((request_line.trim_end().to_string(), headers));
        }
        total += n;
        if total > 65536 {
            return None;
        }
        headers.push(line.trim_end().to_string());
    }
}

fn close<S: Read + Write>(stream: &mut rustls::StreamOwned<rustls::ServerConnection, S>) {
    stream.conn.send_close_notify();
    let _ = stream.flush();
}
