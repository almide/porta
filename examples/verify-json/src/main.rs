//! JSON artifact verifier for Porta's completion-check protocol.
//! All filesystem access is confined by the host's read-only WASI preopens.
use serde_json::{json, Value};
use std::{fs::File, io::{self, Read}};
const MAX_BYTES: u64 = 1024 * 1024;

fn bounded(reader: impl Read) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    reader.take(MAX_BYTES + 1).read_to_end(&mut bytes).map_err(|_| "cannot read verification input or artifact")?;
    if bytes.len() as u64 > MAX_BYTES { return Err("verification input or artifact exceeds 1 MiB".into()); }
    Ok(bytes)
}
fn verify() -> Result<bool, String> {
    let request: Value = serde_json::from_slice(&bounded(io::stdin())?).map_err(|_| "invalid verification input JSON")?;
    let parameters = request.get("parameters").ok_or("missing parameters")?;
    let path = parameters.get("path").and_then(Value::as_str).ok_or("missing artifact path")?;
    let expected = parameters.get("expected").ok_or("missing expected JSON value")?;
    let file = File::open(path).map_err(|_| "artifact is missing or unreadable")?;
    let actual: Value = serde_json::from_slice(&bounded(file)?).map_err(|_| "artifact contains invalid JSON")?;
    if &actual != expected { return Ok(false); }
    if let Some(required) = parameters.get("required_tool_calls") {
        let required = required.as_object().ok_or("required_tool_calls must be an object")?;
        let calls = request.get("tool_calls").and_then(Value::as_array).ok_or("missing host tool history")?;
        for (name, minimum) in required {
            let minimum = minimum.as_u64().filter(|n| *n > 0).ok_or("required call counts must be positive integers")?;
            let actual = calls.iter().filter(|call| call.get("name").and_then(Value::as_str) == Some(name)).count() as u64;
            if actual < minimum { return Err(format!("Completion requires at least {minimum} call(s) to {name}; observed {actual}.")); }
        }
    }
    Ok(true)
}
fn main() {
    let verdict = match verify() {
        Ok(true) => json!({"passed":true}),
        Ok(false) => json!({"passed":false,"feedback":"JSON artifact differs from the configured expected value. Correct the artifact before completing."}),
        Err(error) => json!({"passed":false,"feedback":error}),
    };
    println!("{verdict}");
}
