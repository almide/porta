//! Loading one agent, and with it the whole team below it.
//!
//! Everything a delegated child inherits travels in a `Descent`, so a child
//! cannot be handed a wider budget, a looser pin or a shorter actor path than
//! its parent had. The team is frozen here, at launch, before any guest runs.

use super::*;

/// How an agent was reached. A root run builds this with [`Descent::root`];
/// every delegated child gets one derived from its parent's.
pub(super) struct Descent {
    chain: Vec<PathBuf>,
    shared: Option<Arc<Mutex<Budget>>>,
    actor: String,
    defer_credential: bool,
    inherited_hashes: bool,
    expected_config: Option<String>,
}

impl Descent {
    /// A run with nothing above it: its own budget and no inherited pin.
    /// `defer_credential` is for the paths that must not resolve a credential
    /// at all, such as `agent-check` and offline replay.
    pub(super) fn root(defer_credential: bool) -> Self {
        Self {
            chain: vec![], shared: None, actor: "root".into(),
            defer_credential, inherited_hashes: false, expected_config: None,
        }
    }

    /// What this agent's children inherit: its own path appended to the chain,
    /// its budget, and its pinning requirement once local strictness is folded
    /// in. Strict pinning therefore only ever widens as the team deepens.
    fn below(&self, path: &Path, budget: &Arc<Mutex<Budget>>, require_hashes: bool) -> Self {
        let mut chain = self.chain.clone();
        chain.push(path.to_path_buf());
        Self {
            chain, shared: Some(budget.clone()), actor: self.actor.clone(),
            defer_credential: self.defer_credential, inherited_hashes: require_hashes,
            expected_config: None,
        }
    }

    /// That inheritance addressed to one named delegate and its config pin.
    fn child(&self, name: &str, expected_config: Option<&str>) -> Self {
        Self {
            chain: self.chain.clone(),
            shared: self.shared.clone(),
            actor: format!("{}/{}", self.actor, name),
            defer_credential: self.defer_credential,
            inherited_hashes: self.inherited_hashes,
            expected_config: expected_config.map(str::to_string),
        }
    }
}

/// Where this agent's modules come from and whether their hashes are required.
struct Artifacts {
    engine: Engine,
    base: PathBuf,
    require_hashes: bool,
}

impl Artifacts {
    fn compile(&self, wasm: &Path, expected: Option<&str>) -> Result<(InstancePre<Context>, String), String> {
        compile(&self.engine, &relative(&self.base, wasm), expected, self.require_hashes)
    }
}

/// A fuelled, interruptible engine: fuel bounds the work one step may do and
/// the epoch timer bounds its wall clock, so neither a loop nor a stall runs on.
fn fuelled_engine() -> Result<Engine, String> {
    let mut config = Config::new();
    config.consume_fuel(true);
    config.epoch_interruption(true);
    Engine::new(&config).map_err(|e| e.to_string())
}

impl Budget {
    /// A fresh root budget for one run's declared limits.
    fn for_limits(limits: &Limits) -> Self {
        Self {
            max_steps: limits.max_steps, max_model_calls: limits.max_model_calls,
            steps: 0, model_calls: 0, tool_calls: 0, delegations: 0,
            verification_calls: 0, verification_failures: 0, fuel: 0,
            started: Instant::now(), timeout: Duration::from_secs(limits.timeout_seconds),
            prior_elapsed: Duration::ZERO,
        }
    }
}

/// The credential this host holds for the model, read from the environment
/// variable the operator named. A named variable that is missing or empty
/// fails the load rather than the first model call.
fn model_credential(model: &Model) -> Result<Option<String>, String> {
    model.token_env.as_ref().map(|name| {
        std::env::var(name).ok().filter(|value| !value.is_empty())
            .ok_or_else(|| format!("required model credential environment variable is missing: {name}"))
    }).transpose()
}

impl Runtime {
    pub(super) fn open(path: &Path, task: &str) -> Result<Self, String> {
        Self::load(path, task, Descent::root(false))
    }

    pub(super) fn load(path: &Path, task: &str, descent: Descent) -> Result<Self, String> {
        let path = std::fs::canonicalize(path).map_err(|e| format!("resolve agent config: {e}"))?;
        if descent.chain.contains(&path) { return Err("cyclic agent delegation configuration".into()); }
        if descent.chain.len() >= 4 { return Err("agent delegation depth exceeds 4".into()); }
        let source = std::fs::read_to_string(&path).map_err(|e| format!("read agent config: {e}"))?;
        let config_digest = verified_digest(source.as_bytes(), descent.expected_config.as_deref(),
            descent.inherited_hashes, &format!("agent config {}", path.display()))?;
        let mut config: AgentConfig = toml::from_str(&source).map_err(|e| format!("invalid agent config: {e}"))?;
        validate_settings(&config, task)?;
        let budget = descent.shared.clone()
            .unwrap_or_else(|| Arc::new(Mutex::new(Budget::for_limits(&config.limits))));
        let token = if descent.defer_credential { None } else { model_credential(&config.model)? };
        let artifacts = Artifacts {
            engine: fuelled_engine()?,
            base: path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf(),
            require_hashes: descent.inherited_hashes || config.require_artifact_hashes,
        };
        let (prepared, digest) = artifacts.compile(&config.agent.wasm, config.agent.sha256.as_deref())?;
        let agent = Guest { prepared, digest, mounts: vec![] };
        let mut names = HashSet::new();
        let (tools, schemas) = load_tools(&mut config, &artifacts, &mut names)?;
        let checks = load_checks(&mut config.completion_checks, &artifacts, "completion")?;
        let before_checks = load_checks(&mut config.before_tool_checks, &artifacts, "before-tool")?;
        let below = descent.below(&path, &budget, artifacts.require_hashes);
        let children = load_children(&config.agents, &artifacts, &mut names, &below)?;
        let engine = artifacts.engine;
        let mut run = Self { config, engine, agent, tools, schemas, checks, before_checks, operations:vec![], token, event:Value::Null, steps:0, model_calls:0, tool_calls:0, fuel_consumed:0, started:Instant::now(), done:false, children, budget, actor:descent.actor, config_digest, decision:Value::Null, journal:None };
        run.reset(task);
        Ok(run)
    }
}

/// Compiles the scoped WASM tools and validates every declared input schema,
/// local and remote alike, before either kind can be offered to a model.
fn load_tools(config: &mut AgentConfig, artifacts: &Artifacts, names: &mut HashSet<String>)
    -> Result<(BTreeMap<String, Guest>, BTreeMap<String, jsonschema::Validator>), String> {
    let mut tools = BTreeMap::new();
    let mut schemas = BTreeMap::new();
    for tool in &mut config.tools {
        claim_name(names, &tool.name, "tool")?;
        if !tool.input_schema.is_object() { return Err(format!("tool {} input_schema must be an object", tool.name)); }
        schemas.insert(tool.name.clone(), offline_validator(&tool.input_schema, &tool.name)?);
        resolve_tool_mounts(tool, &artifacts.base)?;
        let (prepared, digest) = artifacts.compile(&tool.wasm, tool.sha256.as_deref())?;
        tools.insert(tool.name.clone(), Guest { prepared, digest, mounts: std::mem::take(&mut tool.mounts) });
    }
    for tool in &config.mcp_tools {
        claim_name(names, &tool.name, "MCP tool")?;
        tool.check()?;
        schemas.insert(tool.name.clone(), offline_validator(&tool.input_schema, &tool.name)?);
    }
    Ok((tools, schemas))
}

/// Loads every delegated agent now, so the team is fixed at launch: nothing a
/// guest writes later can change which child runs or what it may reach.
fn load_children(agents: &[Delegate], artifacts: &Artifacts, names: &mut HashSet<String>, below: &Descent)
    -> Result<BTreeMap<String, Runtime>, String> {
    let mut children = BTreeMap::new();
    for delegate in agents {
        if delegate.name.is_empty() || !delegate.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            return Err("invalid delegated agent name".into());
        }
        let name = format!("delegate_{}", delegate.name);
        if !names.insert(name.clone()) { return Err(format!("duplicate tool or agent name: {name}")); }
        let config = relative(&artifacts.base, &delegate.config);
        let descent = below.child(&name, delegate.sha256.as_deref());
        children.insert(name, Runtime::load(&config, "pending delegation", descent)?);
    }
    Ok(children)
}

/// Operator-owned checks: compiled here, mounted read-only, and bounded in
/// number so a configuration cannot spend the whole budget on verification.
fn load_checks(configs: &mut Vec<CheckConfig>, artifacts: &Artifacts, label: &str) -> Result<Vec<(String, Guest, Value)>, String> {
    if configs.len() > 16 { return Err(format!("at most 16 {label} checks are supported")); }
    let mut checks = Vec::new();
    let mut check_names = HashSet::new();
    for check in configs {
        name_check(check, &mut check_names, label)?;
        resolve_check_mounts(&mut check.mounts, &artifacts.base, label)?;
        let (prepared, digest) = artifacts.compile(&check.wasm, check.sha256.as_deref())?;
        checks.push((check.name.clone(), Guest { prepared, digest, mounts: std::mem::take(&mut check.mounts) }, check.parameters.clone()));
    }
    Ok(checks)
}

/// A check is addressed by name in verdicts and journal records, so the name
/// has to be a unique identifier before anything else about it is read.
fn name_check(check: &CheckConfig, taken: &mut HashSet<String>, label: &str) -> Result<(), String> {
    if check.name.is_empty() || !taken.insert(check.name.clone()) || !check.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("{label} check names must be unique nonempty ASCII identifiers"));
    }
    if !check.parameters.is_object() { return Err(format!("{label} check parameters must be an object")); }
    Ok(())
}

/// Checks read the artifacts they judge and never write them, so every mount
/// is resolved to a real directory here and refused unless it is read-only.
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
