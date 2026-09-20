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
// A cancellable timer interrupts CPU-bound WASM at the wall-clock deadline.
// Drop always joins it, including traps and instantiation errors.
struct Deadline {
    cancel: std::sync::mpsc::Sender<()>,
    timer: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Deadline {
    fn drop(&mut self) {
        let _ = self.cancel.send(());
        if let Some(timer) = self.timer.take() { let _ = timer.join(); }
    }
}
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

fn fail(message: impl ToString) -> String { json!({"error":message.to_string()}).to_string() }
fn relative(base: &Path, path: &Path) -> PathBuf { if path.is_absolute() { path.into() } else { base.join(path) } }

fn verified_digest(bytes: &[u8], expected: Option<&str>, required: bool, label: &str) -> Result<String, String> {
    if required && expected.is_none() { return Err(format!("sha256 is required for {label}")); }
    if let Some(expected) = expected {
        if expected.len() != 64 || !expected.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c)) {
            return Err(format!("sha256 for {label} must be 64 lowercase hexadecimal characters"));
        }
    }
    let actual = agent_journal::digest(bytes);
    if expected.is_some_and(|expected| expected != actual) { return Err(format!("sha256 mismatch for {label}")); }
    Ok(actual)
}
fn compile(engine: &Engine, path: &Path, expected: Option<&str>, required: bool) -> Result<(InstancePre<Context>, String), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read WASM {}: {e}", path.display()))?;
    let digest = verified_digest(&bytes, expected, required, &format!("WASM {}", path.display()))?;
    let module = Module::from_binary(engine, &bytes).map_err(|e| format!("compile WASM {}: {e}", path.display()))?;
    // WASI is capability-based here: no env, sockets, inherited stdio or implicit
    // directories. A module cannot request a broader grant through its imports.
    for import in module.imports() {
        if import.module() != "wasi_snapshot_preview1" {
            return Err(format!("unsupported host import: {}.{}", import.module(), import.name()));
        }
    }
    if !matches!(module.get_export("_start"), Some(wasmtime::ExternType::Func(ty)) if ty.params().len() == 0 && ty.results().len() == 0) {
        return Err(format!("WASM {} must export _start with no parameters or results", path.display()));
    }
    // Resolve imports and types without a Store or instantiating guest code.
    // Retain this exact linkage for every fresh, isolated invocation.
    let mut linker = Linker::new(engine);
    p1::add_to_linker_sync(&mut linker, |ctx: &mut Context| &mut ctx.wasi).map_err(|e| e.to_string())?;
    let prepared = linker.instantiate_pre(&module).map_err(|e| format!("WASI link validation {}: {e:#}", path.display()))?;
    Ok((prepared, digest))
}
fn load_checks(configs: &mut Vec<CheckConfig>, engine: &Engine, base: &Path, label: &str, required: bool) -> Result<Vec<(String, Guest, Value)>, String> {
    if configs.len() > 16 { return Err(format!("at most 16 {label} checks are supported")); }
    let mut checks = Vec::new();
    let mut check_names = HashSet::new();
    for check in configs {
        if check.name.is_empty() || !check_names.insert(check.name.clone()) || !check.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err(format!("{label} check names must be unique nonempty ASCII identifiers"));
        }
        if !check.parameters.is_object() { return Err(format!("{label} check parameters must be an object")); }
        let mut guest_paths = HashSet::new();
        for mount in &mut check.mounts {
            if !mount.read_only { return Err(format!("{label} check mounts must be read-only")); }
            if mount.guest.is_empty() || !guest_paths.insert(mount.guest.clone()) { return Err(format!("{label} check mount paths must be nonempty and unique")); }
            mount.host = std::fs::canonicalize(relative(base, &mount.host)).map_err(|e| format!("resolve {label} check mount: {e}"))?;
            if !mount.host.is_dir() { return Err(format!("{label} check mount must be a directory")); }
        }
        let (prepared, digest) = compile(engine, &relative(base, &check.wasm), check.sha256.as_deref(), required)?;
        checks.push((check.name.clone(), Guest { prepared, digest, mounts: std::mem::take(&mut check.mounts) }, check.parameters.clone()));
    }
    Ok(checks)
}
/// Settings that must hold before anything is compiled or resolved.
fn validate_settings(config: &AgentConfig, task: &str) -> Result<(), String> {
    if config.version != 1 { return Err("unsupported agent config version (expected 1)".into()); }
    let limits = &config.limits;
    if limits.max_steps == 0 || limits.max_model_calls == 0 || limits.fuel_per_step == 0 || limits.memory_pages == 0 || limits.memory_pages > 65536 || limits.timeout_seconds == 0 || limits.timeout_seconds > 86400 || limits.max_output_tokens == 0 {
        return Err("agent budgets must be positive; memory <= 65536 pages and timeout <= 86400 seconds".into());
    }
    if task.is_empty() || task.len() > MAX_MESSAGE / 2 { return Err("task must be nonempty and at most 512 KiB".into()); }
    if config.model.temperature.is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v)) { return Err("model.temperature must be finite and between 0 and 2".into()); }
    if config.model.name.trim().is_empty() { return Err("model.name is required".into()); }
    validate_endpoint(&config.model.endpoint)
}

/// The model endpoint may not carry credentials and is plaintext only on loopback.
fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "invalid model endpoint")?;
    let loopback = url.host_str().is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback) || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("model endpoint must use HTTPS (HTTP only on loopback), without credentials, query or fragment".into());
    }
    Ok(())
}

/// A name is unique across every tool and delegated agent in one team.
fn claim_name(names: &mut HashSet<String>, name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') || !names.insert(name.to_string()) {
        return Err(format!("{kind} names must be unique, nonempty ASCII letters, numbers, underscores or hyphens"));
    }
    Ok(())
}

/// Schema validation never reaches the network or the host filesystem.
fn offline_validator(schema: &Value, name: &str) -> Result<jsonschema::Validator, String> {
    jsonschema::options().offline().should_validate_formats(true)
        .should_ignore_unknown_formats(false).build(schema)
        .map_err(|_| format!("tool {name} has an invalid or unresolved input_schema"))
}

/// Resolve every tool mount before the guest runs, so nothing can widen it later.
fn resolve_tool_mounts(tool: &mut ToolConfig, base: &Path) -> Result<(), String> {
    let mut guest_paths = HashSet::new();
    for mount in &mut tool.mounts {
        if mount.guest.is_empty() || !guest_paths.insert(mount.guest.clone()) { return Err("tool mount guest paths must be nonempty and unique".into()); }
        mount.host = std::fs::canonicalize(relative(base, &mount.host)).map_err(|e| format!("resolve tool mount: {e}"))?;
        if !mount.host.is_dir() { return Err("tool mount must be a directory".into()); }
    }
    Ok(())
}

impl Runtime {
    fn open(path: &Path, task: &str) -> Result<Self, String> {
        Self::load(path, task, &[], None, "root", false, false, None)
    }
    fn load(path: &Path, task: &str, chain: &[PathBuf], shared: Option<Arc<Mutex<Budget>>>, actor: &str, defer_credential: bool, inherited_hashes: bool, expected_config: Option<&str>) -> Result<Self, String> {
        let path = std::fs::canonicalize(path).map_err(|e| format!("resolve agent config: {e}"))?;
        if chain.contains(&path) { return Err("cyclic agent delegation configuration".into()); }
        if chain.len() >= 4 { return Err("agent delegation depth exceeds 4".into()); }
        let mut chain = chain.to_vec();
        chain.push(path.clone());
        let source = std::fs::read_to_string(&path).map_err(|e| format!("read agent config: {e}"))?;
        let config_digest = verified_digest(source.as_bytes(), expected_config, inherited_hashes, &format!("agent config {}", path.display()))?;
        let mut config: AgentConfig = toml::from_str(&source).map_err(|e| format!("invalid agent config: {e}"))?;
        validate_settings(&config, task)?;
        let require_hashes = inherited_hashes || config.require_artifact_hashes;
        let limits = &config.limits;
        let budget = shared.unwrap_or_else(|| Arc::new(Mutex::new(Budget {
            max_steps: limits.max_steps, max_model_calls: limits.max_model_calls,
            steps: 0, model_calls: 0, tool_calls: 0, delegations: 0, verification_calls: 0, verification_failures: 0, fuel: 0,
            started: Instant::now(), timeout: Duration::from_secs(limits.timeout_seconds), prior_elapsed: Duration::ZERO,
        })));
        let token = if defer_credential { None } else { config.model.token_env.as_ref().map(|name| {
            std::env::var(name).ok().filter(|s| !s.is_empty()).ok_or_else(|| format!("required model credential environment variable is missing: {name}"))
        }).transpose()? };
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut names = HashSet::new();
        let mut engine_config = Config::new();
        engine_config.consume_fuel(true);
        engine_config.epoch_interruption(true);
        let engine = Engine::new(&engine_config).map_err(|e| e.to_string())?;
        let (prepared, digest) = compile(&engine, &relative(base, &config.agent.wasm), config.agent.sha256.as_deref(), require_hashes)?;
        let agent = Guest { prepared, digest, mounts: vec![] };
        let mut tools = BTreeMap::new();
        let mut schemas = BTreeMap::new();
        for tool in &mut config.tools {
            claim_name(&mut names, &tool.name, "tool")?;
            if !tool.input_schema.is_object() { return Err(format!("tool {} input_schema must be an object", tool.name)); }
            schemas.insert(tool.name.clone(), offline_validator(&tool.input_schema, &tool.name)?);
            resolve_tool_mounts(tool, base)?;
            let (prepared, digest) = compile(&engine, &relative(base, &tool.wasm), tool.sha256.as_deref(), require_hashes)?;
            tools.insert(tool.name.clone(), Guest { prepared, digest, mounts: std::mem::take(&mut tool.mounts) });
        }
        for tool in &config.mcp_tools {
            claim_name(&mut names, &tool.name, "MCP tool")?;
            tool.check()?;
            schemas.insert(tool.name.clone(), offline_validator(&tool.input_schema, &tool.name)?);
        }
        let checks = load_checks(&mut config.completion_checks, &engine, base, "completion", require_hashes)?;
        let before_checks = load_checks(&mut config.before_tool_checks, &engine, base, "before-tool", require_hashes)?;
        let mut children = BTreeMap::new();
        for delegate in &config.agents {
            if delegate.name.is_empty() || !delegate.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
                return Err("invalid delegated agent name".into());
            }
            let name = format!("delegate_{}", delegate.name);
            if !names.insert(name.clone()) { return Err(format!("duplicate tool or agent name: {name}")); }
            // Freeze the complete team at launch, before any guest can change files.
            let child = Self::load(&relative(base, &delegate.config), "pending delegation", &chain, Some(budget.clone()), &format!("{actor}/{name}"), defer_credential, require_hashes, delegate.sha256.as_deref())?;
            children.insert(name, child);
        }
        let mut run = Self { config, engine, agent, tools, schemas, checks, before_checks, operations:vec![], token, event:Value::Null, steps:0, model_calls:0, tool_calls:0, fuel_consumed:0, started:Instant::now(), done:false, children, budget, actor:actor.into(), config_digest, decision:Value::Null, journal:None };
        run.reset(task);
        Ok(run)
    }

    fn identity(&self) -> Value {
        let tools: BTreeMap<_, _> = self.tools.iter().map(|(name, guest)| (name.clone(), json!({"wasm":guest.digest,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()}))).collect();
        let children: BTreeMap<_, _> = self.children.iter().map(|(name, child)| (name.clone(), child.identity())).collect();
        let checks: Vec<_> = self.checks.iter().map(|(name, guest, parameters)| json!({"name":name,"wasm":guest.digest,"parameters":parameters,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()})).collect();
        let before_checks: Vec<_> = self.before_checks.iter().map(|(name, guest, parameters)| json!({"name":name,"wasm":guest.digest,"parameters":parameters,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()})).collect();
        json!({"broker_protocol":5,"before_tool_checks":before_checks,"config":self.config_digest,"agent":self.agent.digest,"tools":tools,"children":children,"checks":checks})
    }
    fn inspection(&self, inherited_pins: bool) -> Value {
        let strict = inherited_pins || self.config.require_artifact_hashes;
        let children: BTreeMap<_, _> = self.children.iter().map(|(name, child)| (name.clone(), child.inspection(strict))).collect();
        let pins: BTreeMap<_, _> = self.config.tools.iter().map(|tool| (tool.name.clone(), tool.sha256.clone())).collect();
        let schemas: BTreeMap<_, _> = self.config.tools.iter().map(|tool| (tool.name.clone(), &tool.input_schema)).collect();
        let checks: BTreeMap<_, _> = self.config.completion_checks.iter().map(|check| (check.name.clone(), check.sha256.clone())).collect();
        let before: BTreeMap<_, _> = self.config.before_tool_checks.iter().map(|check| (check.name.clone(), check.sha256.clone())).collect();
        let delegates: BTreeMap<_, _> = self.config.agents.iter().map(|child| (format!("delegate_{}", child.name), child.sha256.clone())).collect();
        let remote: Vec<_> = self.config.mcp_tools.iter().map(|tool| json!({"name":tool.name,"remote_name":tool.remote_name,"endpoint":tool.endpoint,"token_env":tool.token_env,"input_schema":tool.input_schema})).collect();
        let limits = &self.config.limits;
        let shared = locked(&self.budget);
        let mut report = self.identity();
        report["children"] = json!(children);
        report["strict_artifact_pins"] = json!(strict);
        report["configured_pins"] = json!({"agent":self.config.agent.sha256,"tools":pins,"checks":checks,"before_tool_checks":before,"children":delegates});
        report["model"] = json!({"endpoint":self.config.model.endpoint,"name":self.config.model.name,"token_env":self.config.model.token_env,"temperature":self.config.model.temperature});
        report["local_tool_schemas"] = json!(schemas);
        report["remote_tools"] = json!(remote);
        report["limits"] = json!({"local":{"max_steps":limits.max_steps,"max_model_calls":limits.max_model_calls,"fuel_per_step":limits.fuel_per_step,"memory_pages":limits.memory_pages,"timeout_seconds":limits.timeout_seconds,"max_output_tokens":limits.max_output_tokens},"root":{"max_steps":shared.max_steps,"max_model_calls":shared.max_model_calls,"timeout_seconds":shared.timeout.as_secs()}});
        report
    }
    fn check_journal_path(&self, path: &Path) -> Result<(), String> {
        for guest in self.tools.values().chain(self.checks.iter().chain(self.before_checks.iter()).map(|(_, guest, _)| guest)) {
            if guest.mounts.iter().any(|mount| path.starts_with(&mount.host)) {
                return Err("journal must be outside every tool mount to preserve integrity and conversation isolation".into());
            }
        }
        for child in self.children.values() { child.check_journal_path(path)?; }
        Ok(())
    }
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
    fn execute(&self, guest: &Guest, input: &[u8]) -> Result<(String, u64), String> {
        self.remaining()?;
        if input.len() > MAX_MESSAGE { return Err("guest input exceeds 1 MiB".into()); }
        let mut wasi = WasiCtxBuilder::new();
        wasi.stdin(MemoryInputPipe::new(input.to_vec()));
        let stdout = MemoryOutputPipe::new(MAX_MESSAGE);
        let stderr = MemoryOutputPipe::new(64 * 1024);
        wasi.stdout(stdout.clone()).stderr(stderr);
        for mount in &guest.mounts {
            let (dirs, files) = if mount.read_only { (DirPerms::READ, FilePerms::READ) } else { (DirPerms::all(), FilePerms::all()) };
            wasi.preopened_dir(&mount.host, &mount.guest, dirs, files).map_err(|e| format!("preopen tool directory: {e}"))?;
        }
        let limits = StoreLimitsBuilder::new().memory_size(self.config.limits.memory_pages * 65536).memories(1).tables(4).table_elements(100_000).build();
        let mut store = Store::new(&self.engine, Context { wasi:wasi.build_p1(), limits });
        store.limiter(|ctx| &mut ctx.limits);
        store.set_fuel(self.config.limits.fuel_per_step).map_err(|e| e.to_string())?;
        store.set_epoch_deadline(1);
        let engine = self.engine.clone();
        let remaining = self.remaining()?;
        let (cancel, receiver) = std::sync::mpsc::channel();
        let _deadline = Deadline { cancel, timer: Some(std::thread::spawn(move || {
            if matches!(receiver.recv_timeout(remaining), Err(std::sync::mpsc::RecvTimeoutError::Timeout)) {
                engine.increment_epoch();
            }
        })) };
        let instance = guest.prepared.instantiate(&mut store).map_err(|e| format!("instantiate guest: {e}"))?;
        let start = instance.get_typed_func::<(), ()>(&mut store, "_start").map_err(|e| format!("guest _start: {e}"))?;
        if let Err(e) = start.call(&mut store, ()) {
            if !e.downcast_ref::<wasmtime_wasi::I32Exit>().is_some_and(|e| e.0 == 0) { return Err(format!("guest execution failed: {e:#}")); }
        }
        self.remaining()?;
        let fuel = self.config.limits.fuel_per_step - store.get_fuel().unwrap_or(0);
        let output = String::from_utf8(stdout.contents().to_vec()).map_err(|_| "guest output must be UTF-8")?;
        Ok((output, fuel))
    }
    fn model(&mut self, messages: Vec<Value>) -> Result<Value, String> {
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
        let client = reqwest::blocking::Client::builder().timeout(self.remaining()?.min(Duration::from_secs(30))).redirect(reqwest::redirect::Policy::none()).retry(reqwest::retry::never()).no_proxy().build().map_err(|_| "model HTTP client initialization failed")?;
        let mut request = client.post(&self.config.model.endpoint).header("Content-Type", "application/json").body(body);
        let token = self.token.clone().or_else(|| self.config.model.token_env.as_ref().and_then(|name| std::env::var(name).ok().filter(|s| !s.is_empty())));
        if self.config.model.token_env.is_some() && token.is_none() { return Err("required model credential is missing".into()); }
        if let Some(token) = token { request = request.bearer_auth(token); }
        self.operation_begin("model", journal_request)?;
        let response = request.send().map_err(|_| "model request failed (connection or timeout)")?;
        if !response.status().is_success() { return Err(format!("model returned HTTP {}", response.status().as_u16())); }
        let mut bytes = Vec::new();
        response.take(MAX_MESSAGE as u64 + 1).read_to_end(&mut bytes).map_err(|_| "model response read failed")?;
        if bytes.len() > MAX_MESSAGE { return Err("model response exceeds 1 MiB".into()); }
        self.remaining()?;
        let response: Value = serde_json::from_slice(&bytes).map_err(|_| "model returned invalid JSON")?;
        self.operation_commit(&response, 0)?;
        Ok(response)
    }
    fn verify_checks(&mut self, candidate: &str, proposed: Option<(&str, &Value)>) -> Result<Option<Value>, String> {
        let checks = if proposed.is_some() { &self.before_checks } else { &self.checks };
        let operation = if proposed.is_some() { "before_tool_check" } else { "verification" };
        let input_for = |parameters: &Value| match proposed {
            Some((name, arguments)) => json!({"kind":"before_tool","tool":{"name":name,"arguments":arguments},"parameters":parameters,"tool_calls":self.operations}),
            None => json!({"kind":"verify","candidate":candidate,"parameters":parameters,"tool_calls":self.operations}),
        };
        loop {
            let mut used_cached = false;
            for (name, guest, parameters) in checks {
                if self.steps >= self.config.limits.max_steps { return Err("agent step budget exceeded during verification".into()); }
                {
                    let mut budget = locked(&self.budget);
                    if budget.steps >= budget.max_steps { return Err("team step budget exceeded during verification".into()); }
                    budget.steps += 1;
                    budget.verification_calls += 1;
                }
                self.steps += 1;
                let request = input_for(parameters);
                let mut input = serde_json::to_vec(&request).map_err(|_| "invalid verification input")?;
                input.push(b'\n');
                if input.len() > MAX_MESSAGE { return Err("verification input exceeds 1 MiB".into()); }
                let cached = self.operation_begin(operation, json!({"name":name,"input":request}))?;
                let (result, fuel) = if let Some(recorded) = cached { used_cached = true; recorded } else {
                    let (output, fuel) = self.execute(guest, &input)?;
                    let result: Value = serde_json::from_str(&output).map_err(|_| "completion check must return JSON")?;
                    self.operation_commit(&result, fuel)?;
                    (result, fuel)
                };
                self.fuel_consumed += fuel;
                locked(&self.budget).fuel += fuel;
                let verdict: Verdict = serde_json::from_value(result).map_err(|_| "invalid completion verdict (expected passed boolean and optional feedback string)")?;
                if !verdict.passed {
                    if verdict.feedback.trim().is_empty() { return Err("failed completion check must explain what needs correction".into()); }
                    locked(&self.budget).verification_failures += 1;
                    return Ok(Some(json!({"check":name,"feedback":verdict.feedback})));
                }
            }
            // Consume recorded recheck rounds before making a fresh live check.
            // A historical pass alone cannot certify artifacts after an interruption.
            if used_cached {
                if let (Some(journal), Some((name, _, parameters))) = (&self.journal, checks.first()) {
                    let request = json!({"name":name,"input":input_for(parameters)});
                    let record = json!({"actor":self.actor,"kind":operation,"event":self.event,"decision":self.decision,"request":request});
                    let repeat = {
                        let journal = locked(&journal);
                        journal.continuing_live() || journal.next_matches(&record)
                    };
                    if repeat { continue; }
                }
            }
            return Ok(None);
        }
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
                if !arguments.is_object() { return Err("tool arguments must be an object".into()); }
                if let Some(validator) = self.schemas.get(&name) {
                    if !validator.is_valid(&arguments) {
                        // Pure validation is recomputed during replay. No intent, tool
                        // instantiation, mount access, or side effect occurs here.
                        self.event = json!({"kind":"tool","name":name,"state":state,
                            "result":{"error":{"code":"invalid_tool_arguments",
                            "message":"Arguments do not match the declared input_schema; correct them before retrying."}}});
                        return Ok(json!({"done":false,"event":"tool_rejected","metrics":self.metrics()}));
                    }
                }
                if !self.tools.contains_key(&name) && !self.children.contains_key(&name) && !self.config.mcp_tools.iter().any(|t| t.name == name) {
                    return Err(format!("tool not granted: {name}"));
                }
                if let Some(failure) = self.verify_checks("", Some((&name, &arguments)))? {
                    self.event = json!({"kind":"tool","name":name,"state":state,
                        "result":{"error":{"code":"tool_policy_rejected","check":failure["check"],"message":failure["feedback"]}}});
                    return Ok(json!({"done":false,"event":"tool_rejected","metrics":self.metrics()}));
                }
                let result = if let Some(child) = self.children.get_mut(&name) {
                    let task = arguments.get("task").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() <= MAX_MESSAGE / 2).ok_or("delegation requires a nonempty task of at most 512 KiB")?;
                    if arguments.as_object().is_some_and(|a| a.len() != 1) { return Err("delegation accepts only the task field".into()); }
                    locked(&self.budget).delegations += 1;
                    child.reset(task);
                    loop {
                        let event = child.tick().map_err(|e| format!("{name}: {e}"))?;
                        if event.get("done").and_then(Value::as_bool) == Some(true) {
                            break json!({"ok":event["output"]});
                        }
                    }
                } else if let Some(tool) = self.config.mcp_tools.iter().find(|t| t.name == name) {
                    let request = json!({"name":name,"remote_name":tool.remote_name,"endpoint":tool.endpoint,"arguments":arguments});
                    if let Some((result, _)) = self.operation_cached("mcp_tool", &request)? { result } else {
                        // Missing credentials are preflight failures, not uncertain effects.
                        let token = tool.credential()?;
                        let remaining = self.remaining()?;
                        self.operation_begin("mcp_tool", request)?;
                        let result = agent_mcp::call(tool, &arguments, token, remaining)?;
                        self.remaining()?;
                        self.operation_commit(&result, 0)?;
                        result
                    }
                } else {
                    let tool = self.tools.get(&name).ok_or_else(|| format!("tool not granted: {name}"))?;
                    // The existing tool ABI is a little-endian u32 length plus JSON.
                    let payload = serde_json::to_vec(&json!({"tool":name,"arguments":arguments})).map_err(|e| e.to_string())?;
                    let mut input = (payload.len() as u32).to_le_bytes().to_vec();
                    input.extend_from_slice(&payload);
                    let cached = self.operation_begin("tool", json!({"name":name,"arguments":arguments}))?;
                    let (result, fuel) = if let Some(recorded) = cached { recorded } else {
                        let (output, fuel) = self.execute(tool, &input)?;
                        let result: Value = serde_json::from_str(&output).map_err(|_| "tool output must be JSON")?;
                        self.operation_commit(&result, fuel)?;
                        (result, fuel)
                    };
                    self.fuel_consumed += fuel;
                    locked(&self.budget).fuel += fuel;
                    result
                };
                if !self.checks.is_empty() || !self.before_checks.is_empty() {
                    self.operations.push(json!({"name":name,"arguments":arguments,"result":result}));
                    if serde_json::to_vec(&self.operations).map_err(|_| "invalid verification history")?.len() > MAX_MESSAGE / 2 {
                        return Err("verification tool history exceeds 512 KiB".into());
                    }
                }
                self.tool_calls += 1;
                locked(&self.budget).tool_calls += 1;
                self.event = json!({"kind":"tool","name":name,"result":result,"state":state});
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

fn register(run: Runtime) -> String {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    locked(&RUNS).insert(id, run);
    json!({"handle":id}).to_string()
}
fn recorded(path: &Path, task: &str, journal_path: &Path) -> Result<Runtime, String> {
    let mut run = Runtime::open(path, task)?;
    let parent = journal_path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
    let planned = std::fs::canonicalize(parent).map_err(|e| format!("resolve journal directory: {e}"))?.join(journal_path.file_name().ok_or("journal filename required")?);
    run.check_journal_path(&planned)?;
    let fingerprint = agent_journal::digest(run.identity().to_string().as_bytes());
    let journal = Journal::create(journal_path, &fingerprint, task)?;
    run.attach_journal(Arc::new(Mutex::new(journal)));
    Ok(run)
}
fn resumed(path: &Path, journal_path: &Path, replay: bool) -> Result<Runtime, String> {
    let journal = Journal::load(journal_path, replay)?;
    let mut run = Runtime::load(path, &journal.task, &[], None, "root", true, false, None)?;
    run.check_journal_path(&journal.path)?;
    if journal.fingerprint != agent_journal::digest(run.identity().to_string().as_bytes()) {
        return Err("journal fingerprint mismatch: configuration, WASM artifacts or mount bindings changed".into());
    }
    if !replay { locked(&run.budget).prior_elapsed = Duration::from_millis(journal.elapsed_ms); }
    run.attach_journal(Arc::new(Mutex::new(journal)));
    Ok(run)
}
pub fn agent_open_record(path: impl AsRef<str>, task: impl AsRef<str>, journal: impl AsRef<str>) -> String {
    match recorded(Path::new(path.as_ref()), task.as_ref(), Path::new(journal.as_ref())) { Ok(run) => register(run), Err(e) => fail(e) }
}
pub fn agent_open_resume(path: impl AsRef<str>, journal: impl AsRef<str>, replay: bool) -> String {
    match resumed(Path::new(path.as_ref()), Path::new(journal.as_ref()), replay) { Ok(run) => register(run), Err(e) => fail(e) }
}

/// Verify and summarize a stopped journal without loading any executable artifacts.
pub fn agent_journal_inspect(path: impl AsRef<str>) -> String {
    match Journal::inspect(Path::new(path.as_ref())) {
        Ok(report) => report.to_string(),
        Err(error) => fail(error),
    }
}

/// Load and compile the complete team without executing guest code or resolving credentials.
pub fn agent_check(path: impl AsRef<str>) -> String {
    match Runtime::load(Path::new(path.as_ref()), "configuration check", &[], None, "root", true, false, None) {
        Ok(run) => json!({"valid":true,"fingerprint":agent_journal::digest(run.identity().to_string().as_bytes()),"team":run.inspection(false)}).to_string(),
        Err(error) => fail(error),
    }
}

pub fn agent_open(path: impl AsRef<str>, task: impl AsRef<str>) -> String {
    match Runtime::open(Path::new(path.as_ref()), task.as_ref()) {
        Ok(run) => register(run),
        Err(e) => fail(e),
    }
}
pub fn agent_step(handle: i64) -> String {
    let mut runs = locked(&RUNS);
    match runs.get_mut(&(handle as u64)) {
        Some(run) => match run.tick() {
            Ok(value) => {
                if value["done"].as_bool().unwrap_or(false) {
                    if let Some(journal) = &run.journal {
                        if let Err(error) = locked(&journal).finish(value["output"].as_str().unwrap_or(""), value["metrics"]["elapsed_ms"].as_u64().unwrap_or(0)) { return fail(error); }
                    }
                }
                value.to_string()
            }
            Err(e) => fail(e),
        },
        None => fail("unknown agent run"),
    }
}
pub fn agent_close(handle: i64) -> i64 { if locked(&RUNS).remove(&(handle as u64)).is_some() { 0 } else { -1 } }

pub fn agent_checkpoint(handle: i64) -> String {
    let runs = locked(&RUNS);
    let Some(run) = runs.get(&(handle as u64)) else { return fail("unknown agent run") };
    let Some(journal) = &run.journal else { return fail("checkpoint requires --record") };
    let metrics = run.metrics();
    let saved = locked(&journal).checkpoint(metrics["elapsed_ms"].as_u64().unwrap_or(0));
    match saved {
        Ok(()) => json!({"paused":true,"metrics":metrics}).to_string(),
        Err(e) => fail(e),
    }
}
