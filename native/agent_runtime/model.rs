//! The outbound model call: one request, one bounded reply, no retry.

use super::*;

impl Runtime {
    pub(super) fn model(&mut self, messages: Vec<Value>) -> Result<Value, String> {
        if self.model_calls >= self.config.limits.max_model_calls { return Err("model call budget exceeded".into()); }
        if messages.is_empty() || messages.iter().any(|m| !m.is_object() || m.get("role").and_then(Value::as_str).is_none()) { return Err("model messages must be a nonempty list of messages with roles".into()); }
        let tools = self.definitions();
        let mut body = json!({"model":self.config.model.name,"messages":messages,"max_tokens":self.config.limits.max_output_tokens,"stream":false});
        if !tools.is_empty() { body["tools"] = json!(tools); }
        if let Some(temperature) = self.config.model.temperature { body["temperature"] = json!(temperature); }
        let body = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        if body.len() > MAX_MESSAGE { return Err("model request exceeds 1 MiB".into()); }
        {
            let mut budget = locked(&self.budget);
            if budget.model_calls >= budget.max_model_calls { return Err("team model call budget exceeded".into()); }
            budget.model_calls += 1;
        }
        self.model_calls += 1;
        let journal_request: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        if let Some((response, _)) = self.operation_cached("model", &journal_request)? {
            return Ok(response);
        }
        let request = self.model_request(body)?;
        self.operation_begin("model", journal_request)?;
        let response = Self::read_model_response(request)?;
        self.remaining()?;
        self.operation_commit(&response, 0)?;
        Ok(response)
    }

    /// The outbound model request: no proxy, no redirects, no retry, and the
    /// credential the host holds rather than anything the guest supplied.
    pub(super) fn model_request(&self, body: Vec<u8>) -> Result<reqwest::blocking::RequestBuilder, String> {
        let client = reqwest::blocking::Client::builder()
            .timeout(self.remaining()?.min(Duration::from_secs(30)))
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .no_proxy().build().map_err(|_| "model HTTP client initialization failed")?;
        let request = client.post(&self.config.model.endpoint)
            .header("Content-Type", "application/json").body(body);
        let token = self.token.clone().or_else(|| self.config.model.token_env.as_ref()
            .and_then(|name| std::env::var(name).ok().filter(|s| !s.is_empty())));
        match (self.config.model.token_env.is_some(), token) {
            (true, None) => Err("required model credential is missing".into()),
            (_, Some(token)) => Ok(request.bearer_auth(token)),
            (false, None) => Ok(request),
        }
    }

    /// Send the request and read a bounded JSON reply.
    pub(super) fn read_model_response(request: reqwest::blocking::RequestBuilder) -> Result<Value, String> {
        let response = request.send().map_err(|_| "model request failed (connection or timeout)")?;
        if !response.status().is_success() {
            return Err(format!("model returned HTTP {}", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        response.take(MAX_MESSAGE as u64 + 1).read_to_end(&mut bytes).map_err(|_| "model response read failed")?;
        if bytes.len() > MAX_MESSAGE { return Err("model response exceeds 1 MiB".into()); }
        serde_json::from_slice(&bytes).map_err(|_| "model returned invalid JSON".into())
    }
}
