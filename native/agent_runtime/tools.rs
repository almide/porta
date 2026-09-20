//! One tool step: what may refuse it, which of the three kinds of tool runs
//! it, and what the run remembers afterwards.

use super::*;

/// How many violations a rejection reports, and how long each may be. The list
/// travels back through the model's context, so it is bounded: the first few
/// mistakes are what a caller needs to fix, and the rest are noise.
const MAX_REPORTED_VIOLATIONS: usize = 5;
const MAX_VIOLATION_LENGTH: usize = 200;

/// What is wrong with these arguments, in terms of the schema the caller was
/// given. A rejection that only says "invalid" leaves a model guessing, and a
/// guessing model tends to resend exactly what was refused; the broker already
/// knows which properties failed, so it says which.
///
/// An empty list means the arguments are valid. Nothing here reaches the
/// network or the filesystem: the schema was compiled offline at load.
fn schema_violations(validator: &jsonschema::Validator, arguments: &Value) -> Vec<String> {
    validator
        .iter_errors(arguments)
        .take(MAX_REPORTED_VIOLATIONS)
        .map(|error| {
            let path = error.instance_path().to_string();
            let at = if path.is_empty() { "arguments".to_string() } else { path };
            clipped(format!("{at}: {error}"), MAX_VIOLATION_LENGTH)
        })
        .collect()
}

/// Truncates on a character boundary, so a multi-byte name cannot split.
fn clipped(text: String, limit: usize) -> String {
    if text.len() <= limit { return text; }
    let end = (0..=limit).rev().find(|at| text.is_char_boundary(*at)).unwrap_or(0);
    format!("{}…", &text[..end])
}

impl Runtime {
    /// Delegate to a child agent and return its final output. The child shares
    /// this team's budget, so it cannot buy itself more steps.
    pub(super) fn run_delegate(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let task = arguments.get("task").and_then(Value::as_str)
            .filter(|s| !s.is_empty() && s.len() <= MAX_MESSAGE / 2)
            .ok_or("delegation requires a nonempty task of at most 512 KiB")?
            .to_string();
        if arguments.as_object().is_some_and(|a| a.len() != 1) { return Err("delegation accepts only the task field".into()); }
        locked(&self.budget).delegations += 1;
        let child = self.children.get_mut(name).ok_or_else(|| format!("tool not granted: {name}"))?;
        child.reset(&task);
        loop {
            let event = child.tick().map_err(|e| format!("{name}: {e}"))?;
            if event.get("done").and_then(Value::as_bool) == Some(true) {
                return Ok(json!({"ok":event["output"]}));
            }
        }
    }

    /// Call a granted remote MCP tool, replaying a recorded reply when the
    /// journal holds one. A missing credential is a preflight failure, not an
    /// uncertain effect, so it never leaves a pending intent.
    pub(super) fn call_remote_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let tool = self.config.mcp_tools.iter().find(|t| t.name == name)
            .ok_or_else(|| format!("tool not granted: {name}"))?;
        let request = json!({"name":name,"remote_name":tool.remote_name,"endpoint":tool.endpoint,"arguments":arguments});
        if let Some((result, _)) = self.operation_cached("mcp_tool", &request)? { return Ok(result); }
        let token = tool.credential()?;
        let remaining = self.remaining()?;
        self.operation_begin("mcp_tool", request)?;
        let result = agent_mcp::call(tool, arguments, token, remaining)?;
        self.remaining()?;
        self.operation_commit(&result, 0)?;
        Ok(result)
    }

    /// Run one granted tool, delegated agent or remote MCP call, and record the
    /// event the guest sees next. Returns the reply for a rejected call, or None
    /// when the tool ran and the loop should continue.
    pub(super) fn handle_tool(&mut self, name: String, arguments: Value, state: Value) -> Result<Option<Value>, String> {
        if let Some(rejection) = self.reject_tool_call(&name, &arguments, &state)? {
            return Ok(Some(rejection));
        }
        let result = self.invoke_tool(&name, &arguments)?;
        self.record_operation(&name, &arguments, &result)?;
        self.tool_calls += 1;
        locked(&self.budget).tool_calls += 1;
        self.event = json!({"kind":"tool","name":name,"result":result,"state":state});
        Ok(None)
    }
    /// Everything that can refuse a call before any effect reaches a tool: the
    /// argument shape, the declared schema, the grant, and the pre-tool checks.
    pub(super) fn reject_tool_call(&mut self, name: &str, arguments: &Value, state: &Value) -> Result<Option<Value>, String> {
        if !arguments.is_object() { return Err("tool arguments must be an object".into()); }
        let violations = match self.schemas.get(name) {
            Some(validator) => schema_violations(validator, arguments),
            None => Vec::new(),
        };
        if !violations.is_empty() {
            // Pure validation is recomputed during replay. No intent, tool
            // instantiation, mount access, or side effect occurs here.
            return Ok(Some(self.refuse(name, state, json!({"code":"invalid_tool_arguments",
                "message":"Arguments do not match the declared input_schema; correct them and retry.",
                "violations":violations}))));
        }
        if !self.is_granted(name) { return Err(format!("tool not granted: {name}")); }
        match self.verify_checks("", Some((name, arguments)))? {
            Some(failure) => Ok(Some(self.refuse(name, state, json!({"code":"tool_policy_rejected",
                "check":failure["check"],"message":failure["feedback"]})))),
            None => Ok(None),
        }
    }
    /// Reports a refusal to the guest as a tool error and ends the step.
    pub(super) fn refuse(&mut self, name: &str, state: &Value, error: Value) -> Value {
        self.event = json!({"kind":"tool","name":name,"state":state,"result":{"error":error}});
        json!({"done":false,"event":"tool_rejected","metrics":self.metrics()})
    }
    pub(super) fn is_remote(&self, name: &str) -> bool {
        self.config.mcp_tools.iter().any(|tool| tool.name == name)
    }
    pub(super) fn is_granted(&self, name: &str) -> bool {
        self.tools.contains_key(name) || self.children.contains_key(name) || self.is_remote(name)
    }
    /// Runs a granted tool, whichever of the three kinds it is.
    pub(super) fn invoke_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        if self.children.contains_key(name) { return self.run_delegate(name, arguments); }
        if self.is_remote(name) { return self.call_remote_tool(name, arguments); }
        self.run_local_tool(name, arguments)
    }
    /// Calls a scoped WASM tool, replaying a recorded result when the journal
    /// already holds one so a resumed run cannot repeat a completed effect.
    pub(super) fn run_local_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        let tool = self.tools.get(name).ok_or_else(|| format!("tool not granted: {name}"))?;
        // The existing tool ABI is a little-endian u32 length plus JSON.
        let payload = serde_json::to_vec(&json!({"tool":name,"arguments":arguments})).map_err(|e| e.to_string())?;
        let mut input = (payload.len() as u32).to_le_bytes().to_vec();
        input.extend_from_slice(&payload);
        let cached = self.operation_begin("tool", json!({"name":name,"arguments":arguments}))?;
        let (result, fuel) = match cached {
            Some(recorded) => recorded,
            None => {
                let (output, fuel) = self.execute(tool, &input)?;
                let result: Value = serde_json::from_str(&output).map_err(|_| "tool output must be JSON")?;
                self.operation_commit(&result, fuel)?;
                (result, fuel)
            }
        };
        self.fuel_consumed += fuel;
        locked(&self.budget).fuel += fuel;
        Ok(result)
    }
    /// Checks read the tool history, so it is kept only when a check exists to
    /// read it and bounded so it cannot outgrow one message.
    pub(super) fn record_operation(&mut self, name: &str, arguments: &Value, result: &Value) -> Result<(), String> {
        if self.checks.is_empty() && self.before_checks.is_empty() { return Ok(()); }
        self.operations.push(json!({"name":name,"arguments":arguments,"result":result}));
        if serde_json::to_vec(&self.operations).map_err(|_| "invalid verification history")?.len() > MAX_MESSAGE / 2 {
            return Err("verification tool history exceeds 512 KiB".into());
        }
        Ok(())
    }
}
