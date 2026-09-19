//! Require a pass from the most recent actual MCP tool call, not agent state.
use serde_json::{json, Value};
use std::io::{self, Read};

fn check(request: &Value) -> Result<(), String> {
    let name = request.pointer("/parameters/tool").and_then(Value::as_str)
        .filter(|s| !s.is_empty()).ok_or("configure parameters.tool")?;
    let call = request.get("tool_calls").and_then(Value::as_array).and_then(|calls| calls.last())
        .ok_or_else(|| format!("Call {name} to verify the task before completing."))?;
    if call.get("name").and_then(Value::as_str) != Some(name) {
        return Err(format!("Call {name} after your last tool operation before completing."));
    }
    let result = call.get("result").ok_or("verification tool has no recorded result")?;
    if result.get("isError").is_some_and(|v| v != &Value::Bool(false)) {
        return Err(format!("{name} returned an error; obtain a successful verification result."));
    }
    // FastMCP wraps a string return as {"result": "<JSON>"} while the
    // text content contains the JSON itself. Normalize only that exact shape.
    let wrapped = result.get("structuredContent").and_then(Value::as_object)
        .filter(|object| object.len() == 1)
        .and_then(|object| object.get("result")).and_then(Value::as_str)
        .map(serde_json::from_str::<Value>).transpose()
        .map_err(|_| "verification tool structured result is not JSON")?;
    let structured = wrapped.as_ref().or_else(|| result.get("structuredContent"));
    let content = result.get("content").and_then(Value::as_array).ok_or("verification tool must return MCP content")?;
    let text = if content.is_empty() { None } else {
        if content.len() != 1 || content[0].get("type").and_then(Value::as_str) != Some("text") {
            return Err("verification tool must return one unambiguous JSON text result".into());
        }
        let text = content[0].get("text").and_then(Value::as_str).ok_or("missing verification result text")?;
        Some(serde_json::from_str::<Value>(text).map_err(|_| "verification tool result is not JSON")?)
    };
    let verdict = match (structured, text.as_ref()) {
        (Some(a), Some(b)) if a != b => return Err("verification tool returned conflicting structured and text results".into()),
        (Some(value), _) | (None, Some(value)) => value,
        _ => return Err("verification tool returned no verdict".into()),
    };
    if verdict.get("passed") != Some(&Value::Bool(true)) {
        return Err(format!("{name} did not pass. Correct the reported failures, then call it again: {verdict}"));
    }
    Ok(())
}

fn main() {
    let mut bytes = Vec::new();
    let result = io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)
        .map_err(|_| "cannot read verification request".to_string())
        .and_then(|_| if bytes.len() > 1024 * 1024 { Err("verification request exceeds 1 MiB".into()) } else { Ok(()) })
        .and_then(|_| serde_json::from_slice(&bytes).map_err(|_| "invalid verification request".to_string()))
        .and_then(|request| check(&request));
    let result = match result {
        Ok(()) => json!({"passed":true}),
        Err(feedback) => json!({"passed":false,"feedback":feedback}),
    };
    println!("{result}");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(result: Value) -> Value {
        json!({"parameters":{"tool":"verify_task"},"tool_calls":[{"name":"verify_task","result":result}]})
    }
    fn text(value: Value) -> Value { json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}) }
    #[test]
    fn accepts_actual_unambiguous_pass() {
        assert!(check(&request(text(json!({"passed":true})))).is_ok());
        assert!(check(&request(json!({"content":[],"structuredContent":{"passed":true}}))).is_ok());
    }
    #[test]
    fn accepts_matching_sdk_string_wrapper_only() {
        let mut result = text(json!({"passed":true}));
        result["structuredContent"] = json!({"result":"{\"passed\":true}"});
        assert!(check(&request(result.clone())).is_ok());
        result["structuredContent"] = json!({"result":"{\"passed\":false}"});
        assert!(check(&request(result.clone())).is_err());
        result["structuredContent"] = json!({"result":"invalid"});
        assert!(check(&request(result.clone())).is_err());
        result["structuredContent"] = json!({"result":"{\"passed\":true}","extra":true});
        assert!(check(&request(result)).is_err());
    }
    #[test]
    fn rejects_missing_failed_and_stale_evidence() {
        assert!(check(&json!({"parameters":{"tool":"verify_task"},"state":{"passed":true}})).is_err());
        for verdict in [json!({"passed":false}), json!({"passed":"true"}), json!({"ok":true})] {
            assert!(check(&request(text(verdict))).is_err());
        }
        let mut stale = request(text(json!({"passed":true})));
        stale["tool_calls"].as_array_mut().unwrap().push(json!({"name":"write_file","result":{"ok":true}}));
        assert!(check(&stale).is_err());
    }
    #[test]
    fn rejects_errors_and_conflicting_representations() {
        let mut error = text(json!({"passed":true}));
        error["isError"] = json!(true);
        assert!(check(&request(error)).is_err());
        let mut conflict = text(json!({"passed":false}));
        conflict["structuredContent"] = json!({"passed":true});
        assert!(check(&request(conflict)).is_err());
        assert!(check(&request(json!({"content":[{"type":"text","text":"not JSON"}]}))).is_err());
    }
}
