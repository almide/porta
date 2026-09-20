//! Pure computation inside a fresh WASI instance. No script I/O functions.
use rhai::{Array, Dynamic, Engine, INT, ImmutableString, Map, Scope};
use rust_decimal::Decimal;
use serde_json::{Value, json};
use std::io::{self, Read};
use std::str::FromStr;

const MAX_FRAME: usize = 256 * 1024;
const MAX_JSON: usize = 128 * 1024;
const MAX_SCRIPT: usize = 16 * 1024;
const MAX_NODES: usize = 10_000;
const MAX_DEPTH: usize = 32;

fn visit(depth: usize, nodes: &mut usize) -> Result<(), String> {
    *nodes += 1;
    if depth > MAX_DEPTH || *nodes > MAX_NODES {
        return Err("JSON exceeds depth or node limit".into());
    }
    Ok(())
}

fn to_script(value: Value, depth: usize, nodes: &mut usize) -> Result<Dynamic, String> {
    visit(depth, nodes)?;
    match value {
        Value::Null => Ok(Dynamic::UNIT),
        Value::Bool(v) => Ok(v.into()),
        Value::String(v) => string_to_script(v),
        Value::Number(v) => number_to_script(v),
        Value::Array(values) => array_to_script(values, depth, nodes),
        Value::Object(values) => object_to_script(values, depth, nodes),
    }
}

fn string_to_script(text: String) -> Result<Dynamic, String> {
    if text.len() > 64 * 1024 {
        return Err("input string exceeds 64 KiB".into());
    }
    Ok(text.into())
}

/// Integers stay integers; everything else becomes an exact decimal, so a
/// value that would only survive as a rounded float is refused instead.
fn number_to_script(number: serde_json::Number) -> Result<Dynamic, String> {
    if let Some(integer) = number.as_i64() {
        return Ok(integer.into());
    }
    let text = number.to_string();
    let decimal = match text.split_once(['e', 'E']) {
        // from_scientific alone accepts a rounded mantissa. Reject that loss
        // before applying its checked exponent conversion.
        Some((base, _)) => Decimal::from_str_exact(base).and_then(|_| Decimal::from_scientific(&text)),
        None => Decimal::from_str_exact(&text),
    }
    .map_err(|_| "JSON number exceeds supported decimal precision or range; preserve it as a string if it is not used in arithmetic")?;
    Ok(Dynamic::from(decimal))
}

fn array_to_script(values: Vec<Value>, depth: usize, nodes: &mut usize) -> Result<Dynamic, String> {
    if values.len() > 4096 {
        return Err("input array exceeds 4096 elements".into());
    }
    let values: Result<Array, String> = values
        .into_iter()
        .map(|v| to_script(v, depth + 1, nodes))
        .collect();
    Ok(values?.into())
}

fn object_to_script(values: serde_json::Map<String, Value>, depth: usize, nodes: &mut usize) -> Result<Dynamic, String> {
    if values.len() > 1024 {
        return Err("input object exceeds 1024 properties".into());
    }
    let values: Result<Map, String> = values
        .into_iter()
        .map(|(k, v)| Ok((k.into(), to_script(v, depth + 1, nodes)?)))
        .collect();
    Ok(values?.into())
}

fn to_json(value: Dynamic, depth: usize, nodes: &mut usize) -> Result<Value, String> {
    visit(depth, nodes)?;
    if value.is_unit() {
        Ok(Value::Null)
    } else if value.is::<bool>() {
        Ok(Value::Bool(value.cast()))
    } else if value.is::<INT>() {
        Ok(Value::Number(value.cast::<INT>().into()))
    } else if value.is::<Decimal>() {
        let text = value.cast::<Decimal>().to_string();
        Ok(Value::Number(
            serde_json::Number::from_str(&text).map_err(|_| "invalid decimal result")?,
        ))
    } else if value.is::<ImmutableString>() {
        Ok(Value::String(value.cast::<ImmutableString>().into()))
    } else if value.is::<char>() {
        Ok(Value::String(value.cast::<char>().to_string()))
    } else if value.is::<Array>() {
        value
            .cast::<Array>()
            .into_iter()
            .map(|v| to_json(v, depth + 1, nodes))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    } else if value.is::<Map>() {
        value
            .cast::<Map>()
            .into_iter()
            .map(|(k, v)| Ok((k.into(), to_json(v, depth + 1, nodes)?)))
            .collect::<Result<serde_json::Map<_, _>, String>>()
            .map(Value::Object)
    } else {
        Err("script result is not JSON-compatible".into())
    }
}

fn compute(request: Value) -> Result<Value, String> {
    if request.get("tool").and_then(Value::as_str) != Some("compute") {
        return Err("unknown tool; expected compute".into());
    }
    let args = request
        .get("arguments")
        .and_then(Value::as_object)
        .ok_or("arguments must be an object")?;
    if args.keys().any(|k| k != "script" && k != "input_json") {
        return Err("unknown compute argument".into());
    }
    let script = args
        .get("script")
        .and_then(Value::as_str)
        .ok_or("script must be a string")?;
    let input = args
        .get("input_json")
        .and_then(Value::as_str)
        .ok_or("input_json must be a JSON string")?;
    if script.is_empty() || script.len() > MAX_SCRIPT || input.len() > MAX_JSON {
        return Err("script or input_json exceeds size limits".into());
    }
    let input: Value =
        serde_json::from_str(input).map_err(|e| format!("invalid input_json: {e}"))?;
    let input = to_script(input, 0, &mut 0)?;
    let mut engine = Engine::new();
    engine.set_max_operations(100_000);
    engine.set_max_call_levels(32);
    engine.set_max_expr_depths(32, 16);
    engine.set_max_variables(256);
    engine.set_max_functions(64);
    engine.set_max_string_size(64 * 1024);
    engine.set_max_array_size(4096);
    engine.set_max_map_size(1024);
    // Print/debug must not corrupt the single JSON tool response.
    engine.on_print(|_| {});
    engine.on_debug(|_, _, _| {});
    let mut scope = Scope::new();
    scope.push("input", input);
    let result = engine
        .eval_with_scope::<Dynamic>(&mut scope, script)
        .map_err(|e| format!("script failed: {e}"))?;
    let result = to_json(result, 0, &mut 0)?;
    let encoded = serde_json::to_string(&result).map_err(|_| "cannot encode result")?;
    if encoded.len() > MAX_JSON {
        return Err("result exceeds 128 KiB".into());
    }
    // Carry JSON as text through the broker so it cannot round decimal values.
    Ok(json!({"ok":true,"json":encoded}))
}

fn read_request(mut reader: impl Read) -> Result<Value, String> {
    let mut header = [0_u8; 4];
    reader
        .read_exact(&mut header)
        .map_err(|_| "missing tool frame header")?;
    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_FRAME {
        return Err("tool frame exceeds 256 KiB".into());
    }
    let mut bytes = vec![0_u8; length];
    reader
        .read_exact(&mut bytes)
        .map_err(|_| "incomplete tool frame")?;
    serde_json::from_slice(&bytes).map_err(|_| "invalid tool request JSON".into())
}

fn main() {
    let result = read_request(io::stdin()).and_then(compute).unwrap_or_else(
        |error| json!({"ok":false,"error":{"code":"compute_failed","message":error}}),
    );
    println!("{result}");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn run(script: &str, input: &str) -> Result<Value, String> {
        compute(json!({"tool":"compute","arguments":{"script":script,"input_json":input}}))
    }
    #[test]
    fn exact_decimal_and_large_integer_transport() {
        assert_eq!(run("0.1 + 0.2", "null").unwrap()["json"], "0.3");
        assert_eq!(run("input / 1000.0", "2500").unwrap()["json"], "2.5");
        assert_eq!(run("input", "1.25e3").unwrap()["json"], "1250");
        assert_eq!(
            run("input.a + input.b", r#"{"a":0.1,"b":0.2}"#).unwrap()["json"],
            "0.3"
        );
        assert_eq!(
            run("input", "9007199254740993").unwrap()["json"],
            "9007199254740993"
        );
        assert!(run("input", "0.12345678901234567890123456789").is_err());
        assert!(run("input", "0.12345678901234567890123456789e1").is_err());
        assert!(run("input", "1e-29").is_err());
    }
    #[test]
    fn transform_without_changing_nested_data() {
        let result = run(
            "input.new_name = input.remove(\"old_name\"); input",
            r#"{"old_name":7,"nested":{"old_name":8},"label":"日本語"}"#,
        )
        .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(result["json"].as_str().unwrap()).unwrap(),
            json!({"new_name":7,"nested":{"old_name":8},"label":"日本語"})
        );
    }
    #[test]
    fn bounded_and_pure() {
        for script in [
            "loop {}",
            "fn f() { f() } f()",
            "read_file(\"/etc/passwd\")",
            "env(\"HOME\")",
            "timestamp()",
            "import \"evil\" as x;",
            "1 / 0",
            "9223372036854775807 + 1",
        ] {
            assert!(run(script, "null").is_err(), "{script}");
        }
        assert_eq!(
            run("print(\"noise\"); debug(42); 2 + 3", "null").unwrap()["json"],
            "5"
        );
        assert!(run("[0] * 100000", "null").is_err());
    }
    #[test]
    fn invalid_frames_and_input() {
        assert!(read_request(&[1_u8, 0][..]).is_err());
        assert!(read_request(&(MAX_FRAME as u32 + 1).to_le_bytes()[..]).is_err());
        assert!(run("input", "{").is_err());
        assert!(run(&" ".repeat(MAX_SCRIPT + 1), "null").is_err());
    }
}
