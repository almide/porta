//! Capability broker for guest-driven agents. The decision loop is a WASM program;
//! this module owns credentials, registered tools, and non-negotiable run budgets.
use crate::locking::locked;
use serde::Deserialize;
use crate::agent_journal::{self, Journal};
use crate::agent_mcp;
use serde_json::{json, Value};
use std::{collections::{BTreeMap, HashSet}, io::Read, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicU64, Ordering}}, time::{Duration, Instant}};
use wasmtime::{Config, Engine, InstancePre, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::{WasiCtxBuilder, p1::{self, WasiP1Ctx}, p2::pipe::{MemoryInputPipe, MemoryOutputPipe}, filesystem::{DirPerms, FilePerms}};

mod config;
mod ffi;
mod guest;
mod inspect;
mod loading;
mod model;
mod tools;
mod verification;
use config::*;
use loading::Descent;
pub use ffi::*;

const MAX_MESSAGE: usize = 1024 * 1024;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentConfig {
    version: u32,
    #[serde(default)] require_artifact_hashes: bool,
    agent: Agent,
    model: Model,
    #[serde(default)] limits: Limits,
    #[serde(default)] tools: Vec<ToolConfig>,
    #[serde(default)] agents: Vec<Delegate>,
    #[serde(default)] mcp_tools: Vec<agent_mcp::Tool>,
    #[serde(default)] completion_checks: Vec<CheckConfig>,
    #[serde(default)] before_tool_checks: Vec<CheckConfig>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Agent { wasm: PathBuf, #[serde(default)] sha256: Option<String>, #[serde(default)] instruction: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model { endpoint: String, name: String, #[serde(default)] token_env: Option<String>, #[serde(default)] temperature: Option<f64> }
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Limits { max_steps: u32, max_model_calls: u32, fuel_per_step: u64, memory_pages: usize, timeout_seconds: u64, max_output_tokens: u32 }
impl Default for Limits {
    fn default() -> Self { Self { max_steps: 64, max_model_calls: 16, fuel_per_step: 50_000_000, memory_pages: 1024, timeout_seconds: 120, max_output_tokens: 2048 } }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolConfig {
    name: String, wasm: PathBuf, #[serde(default)] sha256: Option<String>, #[serde(default)] description: String,
    #[serde(default = "object_schema")] input_schema: Value,
    #[serde(default)] mounts: Vec<Mount>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Delegate { name: String, config: PathBuf, #[serde(default)] sha256: Option<String>, #[serde(default)] description: String }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckConfig {
    name: String, wasm: PathBuf, #[serde(default)] sha256: Option<String>,
    #[serde(default = "empty_object")] parameters: Value,
    #[serde(default)] mounts: Vec<Mount>,
}
fn empty_object() -> Value { json!({}) }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Verdict { passed: bool, #[serde(default)] feedback: String }

fn object_schema() -> Value { json!({"type":"object", "properties":{}}) }
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mount { host: PathBuf, guest: String, #[serde(default = "read_only")] read_only: bool }
fn read_only() -> bool { true }

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Action {
    Model { messages: Vec<Value>, #[serde(default)] state: Value },
    Tool { name: String, arguments: Value, #[serde(default)] state: Value },
    Done { output: String, #[serde(default)] state: Value },
    Fail { error: String },
}
struct Guest { prepared: InstancePre<Context>, mounts: Vec<Mount>, digest: String }
struct Context { wasi: WasiP1Ctx, limits: StoreLimits }
struct Budget {
    max_steps: u32, max_model_calls: u32, steps: u32, model_calls: u32, tool_calls: u32,
    delegations: u32, verification_calls: u32, verification_failures: u32, fuel: u64, started: Instant, timeout: Duration, prior_elapsed: Duration,
}
struct Runtime {
    config: AgentConfig, engine: Engine, agent: Guest, tools: BTreeMap<String, Guest>,
    schemas: BTreeMap<String, jsonschema::Validator>,
    checks: Vec<(String, Guest, Value)>, before_checks: Vec<(String, Guest, Value)>, operations: Vec<Value>,
    token: Option<String>, event: Value, steps: u32, model_calls: u32, tool_calls: u32,
    fuel_consumed: u64, started: Instant, done: bool,
    children: BTreeMap<String, Runtime>, budget: Arc<Mutex<Budget>>,
    actor: String, config_digest: String, decision: Value, journal: Option<Arc<Mutex<Journal>>>,
}
static RUNS: Mutex<BTreeMap<u64, Runtime>> = Mutex::new(BTreeMap::new());
static NEXT_ID: AtomicU64 = AtomicU64::new(1);


impl Runtime {
    fn attach_journal(&mut self, journal: Arc<Mutex<Journal>>) {
        self.journal = Some(journal.clone());
        for child in self.children.values_mut() { child.attach_journal(journal.clone()); }
    }
    fn operation_cached(&self, kind: &str, request: &Value) -> Result<Option<(Value, u64)>, String> {
        match &self.journal {
            Some(journal) => locked(&journal).cached(&json!({"actor":self.actor,"kind":kind,"event":self.event,"decision":self.decision,"request":request})),
            None => Ok(None),
        }
    }
    fn operation_begin(&self, kind: &str, request: Value) -> Result<Option<(Value, u64)>, String> {
        match &self.journal {
            Some(journal) => locked(&journal).begin(json!({"actor":self.actor,"kind":kind,"event":self.event,"decision":self.decision,"request":request})),
            None => Ok(None),
        }
    }
    fn operation_commit(&self, response: &Value, fuel: u64) -> Result<(), String> {
        if let Some(journal) = &self.journal {
            let elapsed = self.metrics()["elapsed_ms"].as_u64().ok_or("invalid elapsed time")?;
            locked(&journal).commit(response, fuel, elapsed)?;
        }
        Ok(())
    }

    fn definitions(&self) -> Vec<Value> {
        let mut definitions: Vec<Value> = self.config.tools.iter().map(|t| json!({"type":"function", "function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})).collect();
        definitions.extend(self.config.mcp_tools.iter().map(|t| json!({"type":"function", "function":{"name":t.name,"description":t.description,"parameters":t.input_schema}})));
        definitions.extend(self.config.agents.iter().map(|a| json!({"type":"function","function":{"name":format!("delegate_{}", a.name),"description":a.description,"parameters":{"type":"object","properties":{"task":{"type":"string"}},"required":["task"],"additionalProperties":false}}})));
        definitions
    }
    fn reset(&mut self, task: &str) {
        self.event = json!({"kind":"start", "task":task, "instruction":self.config.agent.instruction, "tools":self.definitions()});
        self.operations.clear();
        self.steps = 0;
        self.model_calls = 0;
        self.tool_calls = 0;
        self.fuel_consumed = 0;
        self.started = Instant::now();
        self.done = false;
    }
    fn remaining(&self) -> Result<Duration, String> {
        let budget = locked(&self.budget);
        let global = budget.timeout.checked_sub(budget.prior_elapsed + budget.started.elapsed()).unwrap_or_default();
        let local = Duration::from_secs(self.config.limits.timeout_seconds).checked_sub(self.started.elapsed()).unwrap_or_default();
        let remaining = global.min(local);
        if remaining.is_zero() { Err("agent deadline exceeded".into()) } else { Ok(remaining) }
    }

    fn tick(&mut self) -> Result<Value, String> {
        if self.done { return Err("agent run already completed".into()); }
        if self.steps >= self.config.limits.max_steps { return Err("agent step budget exceeded".into()); }
        {
            let mut budget = locked(&self.budget);
            if budget.steps >= budget.max_steps { return Err("team step budget exceeded".into()); }
            budget.steps += 1;
        }
        let input = serde_json::to_vec(&self.event).map_err(|e| e.to_string())?;
        let mut line = input;
        line.push(b'\n');
        let (output, fuel) = self.execute(&self.agent, &line)?;
        self.steps += 1;
        self.fuel_consumed += fuel;
        locked(&self.budget).fuel += fuel;
        self.decision = serde_json::from_str(&output).map_err(|e| format!("invalid guest action: {e}"))?;
        let action: Action = serde_json::from_str(&output).map_err(|e| format!("invalid guest action: {e}"))?;
        let kind;
        match action {
            Action::Done { output, state } => {
                if let Some(failure) = self.verify_checks(&output, None)? {
                    self.event = json!({"kind":"verification_failed","candidate":output,"failure":failure,"state":state});
                    return Ok(json!({"done":false,"event":"verification_failed","metrics":self.metrics()}));
                }
                self.done = true;
                return Ok(json!({"done":true,"output":output,"metrics":self.metrics()}));
            }
            Action::Fail { error } => return Err(format!("agent failed: {error}")),
            Action::Model { messages, state } => {
                let response = self.model(messages)?;
                self.event = json!({"kind":"model","response":response,"state":state});
                kind = "model";
            }
            Action::Tool { name, arguments, state } => {
                if let Some(reply) = self.handle_tool(name, arguments, state)? { return Ok(reply); }
                kind = "tool";
            }
        }
        Ok(json!({"done":false,"event":kind,"metrics":self.metrics()}))
    }
    fn metrics(&self) -> Value {
        let budget = locked(&self.budget);
        json!({"steps":budget.steps,"model_calls":budget.model_calls,"tool_calls":budget.tool_calls,"delegations":budget.delegations,"verification_calls":budget.verification_calls,"verification_failures":budget.verification_failures,"fuel":budget.fuel,"elapsed_ms":(budget.prior_elapsed + budget.started.elapsed()).as_millis()})
    }
}

