//! Escaping for the replies these host functions assemble by hand.
//!
//! Several host functions answer the guest with a small JSON object built by
//! formatting rather than by serde, and the text they embed comes from a
//! command, a response body or an OS error message. Escaping it here keeps one
//! rule for all of them, including the control characters a raw byte stream can
//! carry, which JSON cannot hold literally.

/// Escapes a string for embedding between double quotes in JSON.
pub fn escape_json_text(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control < ' ' => escaped.push_str(&format!("\\u{:04x}", control as u32)),
            other => escaped.push(other),
        }
    }
    escaped
}
