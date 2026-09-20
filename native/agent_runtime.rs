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
        name_check(check, &mut check_names, label)?;
        resolve_check_mounts(&mut check.mounts, base, label)?;
        let (prepared, digest) = compile(engine, &relative(base, &check.wasm), check.sha256.as_deref(), required)?;
        checks.push((check.name.clone(), Guest { prepared, digest, mounts: std::mem::take(&mut check.mounts) }, check.parameters.clone()));
    }
    Ok(checks)
}
/// A check is addressed by name in verdicts and journal records, so the name has
/// to be a unique identifier before anything else about the check is read.
fn name_check(check: &CheckConfig, taken: &mut HashSet<String>, label: &str) -> Result<(), String> {
    if check.name.is_empty() || !taken.insert(check.name.clone()) || !check.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("{label} check names must be unique nonempty ASCII identifiers"));
    }
    if !check.parameters.is_object() { return Err(format!("{label} check parameters must be an object")); }
    Ok(())
}
/// Checks read the artifacts they judge and never write them, so every mount is
/// resolved to a real directory here and refused unless it is read-only.
fn resolve_check_mounts(mounts: &mut [Mount], base: &Path, label: &str) -> Result<(), String> {
    let mut guest_paths = HashSet::new();
    for mount in mounts {
        if !mount.read_only { return Err(format!("{label} check mounts must be read-only")); }
        if mount.guest.is_empty() || !guest_paths.insert(mount.guest.clone()) { return Err(format!("{label} check mount paths must be nonempty and unique")); }
        mount.host = std::fs::canonicalize(relative(base, &mount.host)).map_err(|e| format!("resolve {label} check mount: {e}"))?;
        if !mount.host.is_dir() { return Err(format!("{label} check mount must be a directory")); }
    }
    Ok(())
}
/// Settings that must hold before anything is compiled or resolved.
fn validate_settings(config: &AgentConfig, task: &str) -> Result<(), String> {
    if config.version != 1 { return Err("unsupported agent config version (expected 1)".into()); }
    if task.is_empty() || task.len() > MAX_MESSAGE / 2 { return Err("task must be nonempty and at most 512 KiB".into()); }
    validate_limits(&config.limits)?;
    validate_model(&config.model)
}

/// Every budget is positive and bounded; an unbounded run is not a run.
fn validate_limits(limits: &Limits) -> Result<(), String> {
    let positive = limits.max_steps > 0 && limits.max_model_calls > 0 && limits.fuel_per_step > 0
        && limits.memory_pages > 0 && limits.timeout_seconds > 0 && limits.max_output_tokens > 0;
    let bounded = limits.memory_pages <= 65536 && limits.timeout_seconds <= 86400;
    if positive && bounded { return Ok(()); }
    Err("agent budgets must be positive; memory <= 65536 pages and timeout <= 86400 seconds".into())
}

fn validate_model(model: &Model) -> Result<(), String> {
    if model.temperature.is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v)) {
        return Err("model.temperature must be finite and between 0 and 2".into());
    }
    if model.name.trim().is_empty() { return Err("model.name is required".into()); }
    validate_endpoint(&model.endpoint)
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
        let request = self.model_request(body)?;
        self.operation_begin("model", journal_request)?;
        let response = Self::read_model_response(request)?;
        self.remaining()?;
        self.operation_commit(&response, 0)?;
        Ok(response)
    }

    /// The outbound model request: no proxy, no redirects, no retry, and the
    /// credential the host holds rather than anything the guest supplied.
    fn model_request(&self, body: Vec<u8>) -> Result<reqwest::blocking::RequestBuilder, String> {
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
    fn read_model_response(request: reqwest::blocking::RequestBuilder) -> Result<Value, String> {
        let response = request.send().map_err(|_| "model request failed (connection or timeout)")?;
        if !response.status().is_success() {
            return Err(format!("model returned HTTP {}", response.status().as_u16()));
        }
        let mut bytes = Vec::new();
        response.take(MAX_MESSAGE as u64 + 1).read_to_end(&mut bytes).map_err(|_| "model response read failed")?;
        if bytes.len() > MAX_MESSAGE { return Err("model response exceeds 1 MiB".into()); }
        serde_json::from_slice(&bytes).map_err(|_| "model returned invalid JSON".into())
    }
    /// Charge one verification step against the team budget before running it.
    fn charge_verification_step(&self) -> Result<(), String> {
        let mut budget = locked(&self.budget);
        if budget.steps >= budget.max_steps { return Err("team step budget exceeded during verification".into()); }
        budget.steps += 1;
        budget.verification_calls += 1;
        Ok(())
    }

    /// Run one check, replaying a recorded result when the journal holds one.
    /// Reports the verdict, the fuel it cost, and whether it came from the journal.
    fn run_check(&self, name: &str, guest: &Guest, request: &Value, operation: &str)
        -> Result<(Verdict, u64, bool), String> {
        let mut input = serde_json::to_vec(request).map_err(|_| "invalid verification input")?;
        input.push(b'\n');
        if input.len() > MAX_MESSAGE { return Err("verification input exceeds 1 MiB".into()); }
        let cached = self.operation_begin(operation, json!({"name":name,"input":request}))?;
        let (result, fuel, replayed) = match cached {
            Some((result, fuel)) => (result, fuel, true),
            None => {
                let (output, fuel) = self.execute(guest, &input)?;
                let result: Value = serde_json::from_str(&output).map_err(|_| "completion check must return JSON")?;
                self.operation_commit(&result, fuel)?;
                (result, fuel, false)
            }
        };
        let verdict: Verdict = serde_json::from_value(result)
            .map_err(|_| "invalid completion verdict (expected passed boolean and optional feedback string)")?;
        Ok((verdict, fuel, replayed))
    }

    /// A replayed pass alone cannot certify current artifacts, so recorded
    /// recheck rounds are consumed before a fresh live check is accepted.
    fn journal_expects_recheck(&self, name: &str, input: &Value, operation: &str) -> bool {
        let Some(journal) = &self.journal else { return false };
        let request = json!({"name":name,"input":input});
        let record = json!({"actor":self.actor,"kind":operation,"event":self.event,"decision":self.decision,"request":request});
        let journal = locked(journal);
        journal.continuing_live() || journal.next_matches(&record)
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
                self.charge_verification_step()?;
                self.steps += 1;
                let (verdict, fuel, cached) = self.run_check(name, guest, &input_for(parameters), operation)?;
                used_cached |= cached;
                self.fuel_consumed += fuel;
                locked(&self.budget).fuel += fuel;
                if let Some(failure) = self.refused_by(name, verdict)? { return Ok(Some(failure)); }
            }
            let recheck = used_cached && checks.first().is_some_and(|(name, _, parameters)| {
                self.journal_expects_recheck(name, &input_for(parameters), operation)
            });
            if !recheck { return Ok(None); }
        }
    }
    /// The failure a verdict reports, if it failed. A refusal that explains
    /// nothing is itself an error: the guest would be stopped without being
    /// told what to correct.
    fn refused_by(&self, name: &str, verdict: Verdict) -> Result<Option<Value>, String> {
        if verdict.passed { return Ok(None); }
        if verdict.feedback.trim().is_empty() { return Err("failed completion check must explain what needs correction".into()); }
        locked(&self.budget).verification_failures += 1;
        Ok(Some(json!({"check":name,"feedback":verdict.feedback})))
    }
    /// Delegate to a child agent and return its final output. The child shares
    /// this team's budget, so it cannot buy itself more steps.
    fn run_delegate(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
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
    fn call_remote_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
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
    fn handle_tool(&mut self, name: String, arguments: Value, state: Value) -> Result<Option<Value>, String> {
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
    fn reject_tool_call(&mut self, name: &str, arguments: &Value, state: &Value) -> Result<Option<Value>, String> {
        if !arguments.is_object() { return Err("tool arguments must be an object".into()); }
        if self.schemas.get(name).is_some_and(|validator| !validator.is_valid(arguments)) {
            // Pure validation is recomputed during replay. No intent, tool
            // instantiation, mount access, or side effect occurs here.
            return Ok(Some(self.refuse(name, state, json!({"code":"invalid_tool_arguments",
                "message":"Arguments do not match the declared input_schema; correct them before retrying."}))));
        }
        if !self.is_granted(name) { return Err(format!("tool not granted: {name}")); }
        match self.verify_checks("", Some((name, arguments)))? {
            Some(failure) => Ok(Some(self.refuse(name, state, json!({"code":"tool_policy_rejected",
                "check":failure["check"],"message":failure["feedback"]})))),
            None => Ok(None),
        }
    }
    /// Reports a refusal to the guest as a tool error and ends the step.
    fn refuse(&mut self, name: &str, state: &Value, error: Value) -> Value {
        self.event = json!({"kind":"tool","name":name,"state":state,"result":{"error":error}});
        json!({"done":false,"event":"tool_rejected","metrics":self.metrics()})
    }
    fn is_remote(&self, name: &str) -> bool {
        self.config.mcp_tools.iter().any(|tool| tool.name == name)
    }
    fn is_granted(&self, name: &str) -> bool {
        self.tools.contains_key(name) || self.children.contains_key(name) || self.is_remote(name)
    }
    /// Runs a granted tool, whichever of the three kinds it is.
    fn invoke_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
        if self.children.contains_key(name) { return self.run_delegate(name, arguments); }
        if self.is_remote(name) { return self.call_remote_tool(name, arguments); }
        self.run_local_tool(name, arguments)
    }
    /// Calls a scoped WASM tool, replaying a recorded result when the journal
    /// already holds one so a resumed run cannot repeat a completed effect.
    fn run_local_tool(&mut self, name: &str, arguments: &Value) -> Result<Value, String> {
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
    fn record_operation(&mut self, name: &str, arguments: &Value, result: &Value) -> Result<(), String> {
        if self.checks.is_empty() && self.before_checks.is_empty() { return Ok(()); }
        self.operations.push(json!({"name":name,"arguments":arguments,"result":result}));
        if serde_json::to_vec(&self.operations).map_err(|_| "invalid verification history")?.len() > MAX_MESSAGE / 2 {
            return Err("verification tool history exceeds 512 KiB".into());
        }
        Ok(())
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
