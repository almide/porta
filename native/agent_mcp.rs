//! Explicitly granted remote MCP calls. No discovery-driven grants, redirects,
//! inherited proxies, reconnects, or automatic retries of tool effects.
use serde::Deserialize;
use serde_json::{json, Value};
use std::{io::{BufRead, BufReader, Read}, time::{Duration, Instant}};
use reqwest::blocking::{Client, Response};

const MAX_BYTES: u64 = 1024 * 1024;
const VERSION: &str = "2025-11-25";
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    pub name: String,
    pub endpoint: String,
    pub remote_name: String,
    #[serde(default)] pub description: String,
    pub input_schema: Value,
    #[serde(default)] pub token_env: Option<String>,
}

/// A remote endpoint carries no credential of its own — the host supplies that
/// — and is plaintext only when it is loopback.
fn check_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "invalid MCP endpoint")?;
    let local = url.host_str().is_some_and(|h| matches!(h, "localhost" | "127.0.0.1" | "[::1]"));
    if !(url.scheme() == "https" || url.scheme() == "http" && local)
        || !url.username().is_empty() || url.password().is_some()
        || url.query().is_some() || url.fragment().is_some() {
        return Err("MCP endpoint requires HTTPS (HTTP only on loopback), without credentials, query or fragment".into());
    }
    Ok(())
}

impl Tool {
    pub fn check(&self) -> Result<(), String> {
        check_endpoint(&self.endpoint)?;
        if self.remote_name.is_empty() || self.remote_name.len() > 128 || !self.input_schema.is_object() {
            return Err("MCP remote_name and object input_schema are required".into());
        }
        Ok(())
    }
    pub fn credential(&self) -> Result<Option<String>, String> {
        self.token_env.as_ref().map(|name| std::env::var(name).ok().filter(|v| !v.is_empty())
            .ok_or_else(|| "required MCP credential is missing".to_string())).transpose()
    }
}

struct Connection<'a> {
    tool: &'a Tool, client: Client, token: Option<String>, session: Option<String>,
    version: String, deadline: Instant,
}
impl Connection<'_> {
    fn remaining(&self) -> Result<Duration, String> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { Err("MCP deadline exceeded".into()) } else { Ok(remaining.min(Duration::from_secs(30))) }
    }
    fn request(&self, method: reqwest::Method) -> Result<reqwest::blocking::RequestBuilder, String> {
        let mut request = self.client.request(method, &self.tool.endpoint).timeout(self.remaining()?)
            .header("Accept", "application/json, text/event-stream")
            .header("MCP-Protocol-Version", &self.version);
        if let Some(token) = &self.token { request = request.bearer_auth(token); }
        if let Some(session) = &self.session { request = request.header("MCP-Session-Id", session); }
        Ok(request)
    }
    fn post(&self, message: &Value) -> Result<Response, String> {
        let body = serde_json::to_vec(message).map_err(|_| "invalid MCP request")?;
        if body.len() as u64 > MAX_BYTES { return Err("MCP request exceeds 1 MiB".into()); }
        let response = self.request(reqwest::Method::POST)?.header("Content-Type", "application/json")
            .body(body).send().map_err(|_| "MCP request failed (connection or timeout)")?;
        if !response.status().is_success() { return Err(format!("MCP returned HTTP {}", response.status().as_u16())); }
        Ok(response)
    }
    fn rpc(&mut self, id: u64, method: &str, params: Value) -> Result<Value, String> {
        let response = self.post(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        if method == "initialize" {
            if let Some(header) = response.headers().get("MCP-Session-Id") {
                let session = header.to_str().map_err(|_| "invalid MCP session header")?;
                if session.is_empty() || session.len() > 1024 || !session.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
                    return Err("invalid MCP session header".into());
                }
                self.session = Some(session.into());
            }
        }
        read_response(response, id)
    }
    fn close(&self) {
        // Best effort only; never retry the completed tools/call to close a session.
        if self.session.is_some() {
            if let Ok(request) = self.request(reqwest::Method::DELETE) {
                let _ = request.timeout(self.remaining().unwrap_or(Duration::from_millis(1)).min(Duration::from_secs(1))).send();
            }
        }
    }
}

fn envelope(message: Value, id: u64) -> Result<Option<Value>, String> {
    if message.get("jsonrpc").and_then(Value::as_str) != Some("2.0") { return Err("invalid MCP JSON-RPC version".into()); }
    if message.get("method").is_some() {
        // No sampling, roots, elicitation, or other server-initiated capabilities
        // were advertised. Notifications carry no authority and are ignored.
        if message.get("id").is_some() { return Err("MCP server requested an ungranted client capability".into()); }
        return Ok(None);
    }
    if message.get("id").and_then(Value::as_u64) != Some(id) { return Err("MCP response id mismatch".into()); }
    match (message.get("result"), message.get("error")) {
        (Some(result), None) if result.is_object() => Ok(Some(result.clone())),
        (None, Some(_)) => Err("MCP returned a JSON-RPC error".into()),
        _ => Err("invalid MCP response envelope".into()),
    }
}

// SSE permits LF, CRLF, and lone CR line endings. Read through a buffered
// reader so delimiters and UTF-8 characters may cross network chunk boundaries.
fn sse_line(reader: &mut impl BufRead, total: &mut u64, skip_lf: &mut bool) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    loop {
        let mut byte = [0];
        if reader.read(&mut byte).map_err(|_| "MCP stream read failed (connection or timeout)")? == 0 { return Ok(None); }
        *total += 1;
        if *total > MAX_BYTES { return Err("MCP response exceeds 1 MiB".into()); }
        if *skip_lf {
            *skip_lf = false;
            if byte[0] == b'\n' { continue; }
        }
        if byte[0] == b'\r' || byte[0] == b'\n' {
            *skip_lf = byte[0] == b'\r';
            return String::from_utf8(bytes).map(Some).map_err(|_| "MCP stream must be UTF-8".into());
        }
        bytes.push(byte[0]);
    }
}

fn read_response(response: Response, id: u64) -> Result<Value, String> {
    let mime = response.headers().get("Content-Type").and_then(|h| h.to_str().ok())
        .unwrap_or("").split(';').next().unwrap_or("").trim().to_ascii_lowercase();
    match mime.as_str() {
        "application/json" => read_json_result(response, id),
        "text/event-stream" => read_streamed_result(response, id),
        _ => Err("unsupported MCP response content type".into()),
    }
}

/// A whole JSON reply, bounded so an endless body cannot exhaust this process.
fn read_json_result(response: Response, id: u64) -> Result<Value, String> {
    let mut bytes = Vec::new();
    response.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "MCP response read failed (connection or timeout)")?;
    if bytes.len() as u64 > MAX_BYTES { return Err("MCP response exceeds 1 MiB".into()); }
    let message = serde_json::from_slice(&bytes).map_err(|_| "MCP returned invalid JSON")?;
    envelope(message, id)?.ok_or_else(|| "MCP response did not contain a result".into())
}

/// The first result an SSE reply carries. A stream that ends without one leaves
/// the outcome uncertain, which is reported rather than retried.
fn read_streamed_result(response: Response, id: u64) -> Result<Value, String> {
    let mut reader = BufReader::new(response.take(MAX_BYTES + 1));
    let mut total = 0;
    let mut events = 0;
    let mut skip_lf = false;
    let mut first_line = true;
    let mut data = String::new();
    loop {
        let line = sse_line(&mut reader, &mut total, &mut skip_lf)?
            .ok_or("MCP stream ended without a result; operation will not be retried")?;
        let line = if first_line { first_line = false; line.strip_prefix('\u{feff}').unwrap_or(&line) } else { &line };
        if !line.is_empty() {
            append_field(&mut data, line);
            continue;
        }
        events += 1;
        if events > 128 { return Err("MCP stream exceeds 128 events".into()); }
        if let Some(result) = event_result(&data, id)? { return Ok(result); }
        data.clear();
    }
}

/// Folds one field line into the event being assembled. Fields the protocol
/// defines but this client does not use are ignored.
fn append_field(data: &mut String, line: &str) {
    if let Some(value) = line.strip_prefix("data:") {
        if !data.is_empty() { data.push('\n'); }
        data.push_str(value.strip_prefix(' ').unwrap_or(value));
    } else if line == "data" {
        data.push('\n');
    }
}

/// The result an assembled event carries, if it carries one at all.
fn event_result(data: &str, id: u64) -> Result<Option<Value>, String> {
    if data.trim().is_empty() { return Ok(None); }
    let message = serde_json::from_str(data).map_err(|_| "MCP stream contains invalid JSON")?;
    envelope(message, id)
}

pub fn call(tool: &Tool, arguments: &Value, token: Option<String>, timeout: Duration) -> Result<Value, String> {
    let client = Client::builder().redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never()).no_proxy()
        .build().map_err(|_| "MCP HTTP client initialization failed")?;
    let mut connection = Connection { tool, client, token, session: None, version: VERSION.into(), deadline: Instant::now() + timeout };
    let result = (|| {
        let init = connection.rpc(1, "initialize", json!({"protocolVersion":VERSION,"capabilities":{},"clientInfo":{"name":"porta","version":"0.4.0"}}))?;
        let version = init.get("protocolVersion").and_then(Value::as_str).ok_or("MCP initialize missing protocolVersion")?;
        if !matches!(version, "2025-11-25" | "2025-06-18" | "2025-03-26") { return Err("unsupported MCP protocol version".into()); }
        if !init.get("capabilities").and_then(|v| v.get("tools")).is_some_and(Value::is_object) { return Err("MCP server does not advertise tools".into()); }
        connection.version = version.into();
        let ack = connection.post(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))?;
        if ack.status().as_u16() != 202 { return Err("MCP initialized notification requires HTTP 202".into()); }
        let result = connection.rpc(2, "tools/call", json!({"name":tool.remote_name,"arguments":arguments}))?;
        if !result.get("content").is_some_and(Value::is_array)
            || result.get("isError").is_some_and(|v| !v.is_boolean())
            || result.get("structuredContent").is_some_and(|v| !v.is_object()) {
            return Err("invalid MCP tool result".into());
        }
        Ok(result)
    })();
    connection.close();
    result
}
