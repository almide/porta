//! Agent configuration: the declared shape of a run, and everything that
//! has to hold before one starts.
//!
//! Nothing here instantiates WASM, resolves a credential or contacts a
//! service; `agent-check` runs exactly this much and no more.

use super::*;

pub(super) fn fail(message: impl ToString) -> String { json!({"error":message.to_string()}).to_string() }
pub(super) fn relative(base: &Path, path: &Path) -> PathBuf { if path.is_absolute() { path.into() } else { base.join(path) } }

pub(super) fn verified_digest(bytes: &[u8], expected: Option<&str>, required: bool, label: &str) -> Result<String, String> {
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
pub(super) fn compile(engine: &Engine, path: &Path, expected: Option<&str>, required: bool) -> Result<(InstancePre<Context>, String), String> {
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
pub(super) fn load_checks(configs: &mut Vec<CheckConfig>, engine: &Engine, base: &Path, label: &str, required: bool) -> Result<Vec<(String, Guest, Value)>, String> {
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
pub(super) fn name_check(check: &CheckConfig, taken: &mut HashSet<String>, label: &str) -> Result<(), String> {
    if check.name.is_empty() || !taken.insert(check.name.clone()) || !check.name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(format!("{label} check names must be unique nonempty ASCII identifiers"));
    }
    if !check.parameters.is_object() { return Err(format!("{label} check parameters must be an object")); }
    Ok(())
}
/// Checks read the artifacts they judge and never write them, so every mount is
/// resolved to a real directory here and refused unless it is read-only.
pub(super) fn resolve_check_mounts(mounts: &mut [Mount], base: &Path, label: &str) -> Result<(), String> {
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
pub(super) fn validate_settings(config: &AgentConfig, task: &str) -> Result<(), String> {
    if config.version != 1 { return Err("unsupported agent config version (expected 1)".into()); }
    if task.is_empty() || task.len() > MAX_MESSAGE / 2 { return Err("task must be nonempty and at most 512 KiB".into()); }
    validate_limits(&config.limits)?;
    validate_model(&config.model)
}

/// Every budget is positive and bounded; an unbounded run is not a run.
pub(super) fn validate_limits(limits: &Limits) -> Result<(), String> {
    let positive = limits.max_steps > 0 && limits.max_model_calls > 0 && limits.fuel_per_step > 0
        && limits.memory_pages > 0 && limits.timeout_seconds > 0 && limits.max_output_tokens > 0;
    let bounded = limits.memory_pages <= 65536 && limits.timeout_seconds <= 86400;
    if positive && bounded { return Ok(()); }
    Err("agent budgets must be positive; memory <= 65536 pages and timeout <= 86400 seconds".into())
}

pub(super) fn validate_model(model: &Model) -> Result<(), String> {
    if model.temperature.is_some_and(|v| !v.is_finite() || !(0.0..=2.0).contains(&v)) {
        return Err("model.temperature must be finite and between 0 and 2".into());
    }
    if model.name.trim().is_empty() { return Err("model.name is required".into()); }
    validate_endpoint(&model.endpoint)
}

/// The model endpoint may not carry credentials and is plaintext only on loopback.
pub(super) fn validate_endpoint(endpoint: &str) -> Result<(), String> {
    let url = reqwest::Url::parse(endpoint).map_err(|_| "invalid model endpoint")?;
    let loopback = url.host_str().is_some_and(|h| h == "localhost" || h == "127.0.0.1" || h == "[::1]");
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback) || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() {
        return Err("model endpoint must use HTTPS (HTTP only on loopback), without credentials, query or fragment".into());
    }
    Ok(())
}

/// A name is unique across every tool and delegated agent in one team.
pub(super) fn claim_name(names: &mut HashSet<String>, name: &str, kind: &str) -> Result<(), String> {
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') || !names.insert(name.to_string()) {
        return Err(format!("{kind} names must be unique, nonempty ASCII letters, numbers, underscores or hyphens"));
    }
    Ok(())
}

/// Schema validation never reaches the network or the host filesystem.
pub(super) fn offline_validator(schema: &Value, name: &str) -> Result<jsonschema::Validator, String> {
    jsonschema::options().offline().should_validate_formats(true)
        .should_ignore_unknown_formats(false).build(schema)
        .map_err(|_| format!("tool {name} has an invalid or unresolved input_schema"))
}

/// Resolve every tool mount before the guest runs, so nothing can widen it later.
pub(super) fn resolve_tool_mounts(tool: &mut ToolConfig, base: &Path) -> Result<(), String> {
    let mut guest_paths = HashSet::new();
    for mount in &mut tool.mounts {
        if mount.guest.is_empty() || !guest_paths.insert(mount.guest.clone()) { return Err("tool mount guest paths must be nonempty and unique".into()); }
        mount.host = std::fs::canonicalize(relative(base, &mount.host)).map_err(|e| format!("resolve tool mount: {e}"))?;
        if !mount.host.is_dir() { return Err("tool mount must be a directory".into()); }
    }
    Ok(())
}
