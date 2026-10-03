//! The job API over HTTP/1.1: one request per connection, JSON in and out, a
//! bearer token on everything but `/healthz`. Deliberately small — no TLS, no
//! keep-alive — and meant to sit behind the platform's load balancer or on
//! loopback.
//!
//! | Method and path                   | What it does                                  |
//! |-----------------------------------|-----------------------------------------------|
//! | `GET /healthz`                    | liveness, no token                            |
//! | `GET /v1/policy`                  | the policy in force: label, digest, ceilings  |
//! | `POST /v1/runs[?wait=S]`          | submit a job; 202, or the finished record     |
//! | `GET /v1/runs[?limit=N]`          | newest runs first                             |
//! | `GET /v1/runs/{id}[?wait=S]`      | one record, optionally waiting for the end    |
//! | `POST /v1/runs/{id}/cancel`       | stop a running job                            |
//! | `GET /v1/runs/{id}/output/{path}` | a retained output file                        |
//! | `DELETE /v1/runs/{id}`            | remove a finished run's record and output     |

use super::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};

const MAX_HEADER: usize = 16 * 1024;
const MAX_WAIT_S: u64 = 300;
const MAX_CONNECTIONS: usize = 64;

pub(super) struct Request { method: String, path: String, query: HashMap<String, String>, headers: HashMap<String, String>, body: Vec<u8> }
pub(super) struct Response { status: u16, content_type: &'static str, body: Vec<u8> }

fn reply(status: u16, body: Value) -> Response {
    let mut bytes = serde_json::to_vec_pretty(&body).unwrap_or_default();
    bytes.push(b'\n');
    Response { status, content_type: "application/json", body: bytes }
}
fn error(status: u16, code: &str, message: &str) -> Response { reply(status, json!({"error": {"code": code, "message": message}})) }

pub(super) fn serve(service: Arc<Service>, listen: &str, token: Option<String>) -> Result<(), String> {
    let listener = TcpListener::bind(listen).map_err(|e| format!("listen on {listen}: {e}"))?;
    let address = listener.local_addr().map_err(|e| e.to_string())?;
    eprintln!("[porta job-serve] listening on http://{address} policy={} sha256={}{}", service.policy.label, service.policy.sha256,
        if token.is_some() { "" } else { " (no token: loopback only)" });
    let open = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        if open.load(Ordering::SeqCst) >= MAX_CONNECTIONS {
            let _ = write_response(&stream, &error(503, "busy", "too many open connections"));
            continue;
        }
        open.fetch_add(1, Ordering::SeqCst);
        let (service, token, open) = (Arc::clone(&service), token.clone(), Arc::clone(&open));
        std::thread::spawn(move || {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));
            let limit = service.policy.max_request_bytes;
            let response = match read_request(&stream, limit) {
                Ok(request) => handle(&service, token.as_deref(), request),
                Err(response) => response,
            };
            let _ = write_response(&stream, &response);
            open.fetch_sub(1, Ordering::SeqCst);
        });
    }
    Ok(())
}

fn read_request(stream: &TcpStream, max_body: usize) -> Result<Request, Response> {
    let mut reader = BufReader::new(stream.take((MAX_HEADER + max_body) as u64));
    let (method, target, headers) = read_head(&mut reader)?;
    if headers.contains_key("transfer-encoding") { return Err(error(411, "length_required", "send Content-Length; chunked bodies are not accepted")); }
    let length: usize = headers.get("content-length").map(|v| v.parse().unwrap_or(usize::MAX)).unwrap_or(0);
    if length > max_body { return Err(error(413, "request_too_large", &format!("the body is limited to {max_body} bytes"))); }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).map_err(|_| error(400, "bad_request", "body shorter than Content-Length"))?;
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    let query = query.split('&').filter_map(|pair| pair.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
    Ok(Request { method, path: path.to_string(), query, headers, body })
}

type Head = (String, String, HashMap<String, String>);

/// The request line and headers, refused past 16 KiB.
fn read_head(reader: &mut impl BufRead) -> Result<Head, Response> {
    let unreadable = |_| error(400, "bad_request", "unreadable request head");
    let mut line = String::new();
    reader.read_line(&mut line).map_err(unreadable)?;
    let mut parts = line.trim_end().splitn(3, ' ');
    let (method, target) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
    if method.is_empty() || !target.starts_with('/') { return Err(error(400, "bad_request", "malformed request line")); }
    let mut headers = HashMap::new();
    let mut seen = line.len();
    loop {
        let mut header = String::new();
        let n = reader.read_line(&mut header).map_err(unreadable)?;
        seen += n;
        if seen > MAX_HEADER { return Err(error(431, "headers_too_large", "request headers are too large")); }
        let Some((name, value)) = header.trim_end().split_once(':') else { break };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    Ok((method, target, headers))
}

const REASONS: [(u16, &str); 17] = [
    (200, "OK"), (202, "Accepted"), (204, "No Content"), (400, "Bad Request"), (401, "Unauthorized"),
    (403, "Forbidden"), (404, "Not Found"), (405, "Method Not Allowed"), (409, "Conflict"), (411, "Length Required"),
    (413, "Payload Too Large"), (415, "Unsupported Media Type"), (429, "Too Many Requests"),
    (431, "Request Header Fields Too Large"), (500, "Internal Server Error"), (503, "Service Unavailable"), (0, "Error"),
];

fn write_response(mut stream: &TcpStream, response: &Response) -> std::io::Result<()> {
    let reason = REASONS.iter().find(|(code, _)| *code == response.status || *code == 0).map(|(_, r)| *r).unwrap_or("Error");
    write!(stream, "HTTP/1.1 {} {reason}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        response.status, response.content_type, response.body.len())?;
    stream.write_all(&response.body)?;
    stream.flush()
}

/// A token comparison whose time does not depend on where the strings differ.
fn token_matches(expected: &str, given: &str) -> bool {
    let (a, b) = (expected.as_bytes(), given.as_bytes());
    let mut diff = a.len() ^ b.len();
    for (i, byte) in a.iter().enumerate() { diff |= (byte ^ b.get(i).copied().unwrap_or(0)) as usize; }
    diff == 0
}

fn wait_of(query: &HashMap<String, String>) -> Duration {
    Duration::from_secs(query.get("wait").and_then(|v| v.parse().ok()).unwrap_or(0u64).min(MAX_WAIT_S))
}

fn handle(service: &Arc<Service>, token: Option<&str>, request: Request) -> Response {
    if request.path == "/healthz" { return reply(200, json!({"ok": true})); }
    let given = request.headers.get("authorization").and_then(|v| v.strip_prefix("Bearer ")).unwrap_or("");
    if token.is_some_and(|token| !token_matches(token, given)) { return error(401, "unauthorized", "a valid bearer token is required"); }
    let path = request.path.trim_matches('/').to_string();
    let segments: Vec<&str> = path.split('/').collect();
    match (request.method.as_str(), segments.as_slice()) {
        ("GET", ["v1", rest @ ..]) => read(service, &request, rest),
        ("POST", ["v1", "runs"]) => submit(service, &request),
        ("POST", ["v1", "runs", id, "cancel"]) => cancel(service, id),
        ("DELETE", ["v1", "runs", id]) => delete(service, id),
        (_, ["v1", ..]) => error(405, "method_not_allowed", "see the API reference for the supported methods"),
        _ => error(404, "not_found", "no such endpoint"),
    }
}

/// Every GET under `/v1`.
fn read(service: &Service, request: &Request, path: &[&str]) -> Response {
    match path {
        ["policy"] => reply(200, service.policy.summary()),
        ["runs"] => {
            let limit = request.query.get("limit").and_then(|v| v.parse().ok()).unwrap_or(50usize).min(1000);
            reply(200, json!({"runs": service.list(limit)}))
        }
        ["runs", id] => service.record(id, wait_of(&request.query)).map(|r| reply(200, r)).unwrap_or_else(|| error(404, "not_found", "no such run")),
        ["runs", id, "output", rest @ ..] => output_file(service, id, &rest.join("/")),
        _ => error(404, "not_found", "no such endpoint"),
    }
}

fn submit(service: &Arc<Service>, request: &Request) -> Response {
    if request.headers.get("content-type").is_some_and(|t| !t.starts_with("application/json")) {
        return error(415, "unsupported_media_type", "send the job as application/json");
    }
    let record = match service.submit(&request.body) { Ok(record) => record, Err(rejected) => return reply(rejected.status, rejected.record) };
    let waited = wait_of(&request.query);
    if waited.is_zero() { return reply(202, record); }
    let latest = service.record(record["run_id"].as_str().unwrap_or_default(), waited).unwrap_or(record);
    reply(if latest["status"] == "finished" { 200 } else { 202 }, latest)
}

fn cancel(service: &Service, id: &str) -> Response {
    if service.stop(id, "cancelled") { return reply(202, json!({"run_id": id, "status": "stopping"})); }
    match service.record(id, Duration::ZERO) {
        Some(_) => error(409, "not_running", "the run has already finished"),
        None => error(404, "not_found", "no such run"),
    }
}

fn delete(service: &Service, id: &str) -> Response {
    match service.delete(id) {
        Ok(()) => Response { status: 204, content_type: "application/json", body: Vec::new() },
        Err(409) => error(409, "running", "cancel the run before deleting it"),
        Err(404) => error(404, "not_found", "no such run"),
        Err(_) => error(500, "delete_failed", "the record could not be removed"),
    }
}

/// Serves only a path the record lists as a retained regular file, so the
/// request path never reaches the filesystem on its own.
fn output_file(service: &Service, id: &str, path: &str) -> Response {
    let Some(record) = service.record(id, Duration::ZERO) else { return error(404, "not_found", "no such run") };
    let listed = record["output_files"].as_array().is_some_and(|files| files.iter().any(|f| f["path"] == path && f.get("sha256").is_some()));
    if !listed || !service.policy.retain_output { return error(404, "not_found", "no such retained output file"); }
    match std::fs::read(service.policy.records.join(id).join("output").join(path)) {
        Ok(body) => Response { status: 200, content_type: "application/octet-stream", body },
        Err(_) => error(404, "not_found", "no such retained output file"),
    }
}
