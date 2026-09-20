//! The checked HTTP host function.
//!
//! One request, no redirect followed, no proxy variable read and no retry.
//! Whether a request may be made at all is decided by the Almide side
//! against the run's policy before this is ever called.

use crate::json_text::escape_json_text;

/// Execute an HTTP request. Returns JSON response string.
pub fn wt_http_request(method: impl AsRef<str>, url: impl AsRef<str>, headers_json: impl AsRef<str>, body: impl AsRef<str>) -> String {
    let client = match checked_client() {
        Ok(client) => client,
        Err(reason) => return format!("{{\"error\":\"client error: {}\"}}", reason),
    };
    let Some(mut request) = request_for(&client, method.as_ref(), url.as_ref()) else {
        return format!("{{\"error\":\"unsupported method: {}\"}}", method.as_ref());
    };
    request = with_headers(request, headers_json.as_ref());
    if !body.as_ref().is_empty() { request = request.body(body.as_ref().to_string()); }
    match request.send() {
        Ok(response) => encoded_response(response),
        Err(error) => format!("{{\"error\":\"request failed: {}\"}}", error),
    }
}

/// A client that follows no redirect and reads no proxy variable: where a
/// request may go is decided by the caller's policy, never by the environment.
fn checked_client() -> Result<reqwest::blocking::Client, reqwest::Error> {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .timeout(std::time::Duration::from_secs(30))
        .build()
}

/// The builder for a method this host function supports, or nothing.
fn request_for(client: &reqwest::blocking::Client, method: &str, url: &str) -> Option<reqwest::blocking::RequestBuilder> {
    match method {
        "GET" => Some(client.get(url)),
        "POST" => Some(client.post(url)),
        "PUT" => Some(client.put(url)),
        "DELETE" => Some(client.delete(url)),
        "PATCH" => Some(client.patch(url)),
        "HEAD" => Some(client.head(url)),
        _ => None,
    }
}

/// Adds the caller's headers, given as a JSON object of string values. Anything
/// else carries no header, which is how this ABI has always answered.
fn with_headers(mut request: reqwest::blocking::RequestBuilder, headers_json: &str) -> reqwest::blocking::RequestBuilder {
    let Ok(headers) = serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(headers_json) else {
        return request;
    };
    for (name, value) in headers {
        if let Some(text) = value.as_str() { request = request.header(name.as_str(), text); }
    }
    request
}

fn encoded_response(response: reqwest::blocking::Response) -> String {
    let status = response.status().as_u16();
    match response.text() {
        Ok(text) => format!("{{\"status\":{},\"body\":\"{}\"}}", status, escape_json_text(&text)),
        Err(error) => format!("{{\"error\":\"read error: {}\"}}", error),
    }
}
