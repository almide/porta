//! Example operator policy: require a completed prerequisite before a tool.
use serde_json::{json, Value};
use std::io::{self, Read};
fn check(request: &Value) -> Result<(), String> {
    if request["kind"] != "before_tool" { return Err("expected before_tool request".into()); }
    let p = &request["parameters"];
    let target = p["tool"].as_str().filter(|s| !s.is_empty()).ok_or("configure parameters.tool")?;
    let required = p["requires"].as_str().filter(|s| !s.is_empty()).ok_or("configure parameters.requires")?;
    if request["tool"]["name"] != target { return Ok(()); }
    let history = request["tool_calls"].as_array().ok_or("missing host history")?;
    if history.iter().any(|call| {
        let result = &call["result"];
        call["name"] == required && result.is_object()
            && result.get("error").is_none() && result.get("err").is_none()
            && result.get("isError").is_none_or(|v| v == false)
            && p.get("requires_arguments").is_none_or(|v| v == &call["arguments"])
            && p.get("requires_result").is_none_or(|v| v == result)
    }) { Ok(()) } else {
        Err(format!("Before calling {target}, call {required} with the configured arguments and obtain the required result. The blocked operation has not executed."))
    }
}
fn main() {
    let mut bytes = Vec::new();
    let result = io::stdin().take(1024 * 1024 + 1).read_to_end(&mut bytes)
        .map_err(|_| "cannot read policy input".to_string())
        .and_then(|_| if bytes.len() > 1024 * 1024 { Err("policy input exceeds 1 MiB".into()) } else { Ok(()) })
        .and_then(|_| serde_json::from_slice(&bytes).map_err(|_| "invalid policy input JSON".into()))
        .and_then(|request| check(&request));
    println!("{}", match result { Ok(()) => json!({"passed":true}), Err(feedback) => json!({"passed":false,"feedback":feedback}) });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn requires_actual_matching_nonerror_evidence() {
        let mut r = json!({"kind":"before_tool","tool":{"name":"write"},"parameters":{"tool":"write","requires":"test","requires_arguments":{"path":"x"}},"tool_calls":[],"state":{"passed":true}});
        assert!(check(&r).is_err());
        r["tool_calls"] = json!([{"name":"test","arguments":{"path":"other"},"result":{"ok":true}}]);
        assert!(check(&r).is_err());
        r["tool_calls"][0]["arguments"] = json!({"path":"x"});
        assert!(check(&r).is_ok());
        for result in [json!({"error":"failed"}), json!({"err":"failed"}), json!({"isError":true})] {
            r["tool_calls"][0]["result"] = result;
            assert!(check(&r).is_err());
        }
        r["tool"]["name"] = json!("read");
        assert!(check(&r).is_ok());
    }
    #[test]
    fn optional_exact_result_is_enforced() {
        let mut r = json!({"kind":"before_tool","tool":{"name":"write"},"parameters":{"tool":"write","requires":"test","requires_result":{"passed":true}},"tool_calls":[{"name":"test","result":{"passed":false}}]});
        assert!(check(&r).is_err());
        r["tool_calls"][0]["result"] = json!({"passed":true});
        assert!(check(&r).is_ok());
    }
}
