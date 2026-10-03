//! The operator's policy: which modules may run, which host directories a job
//! may be given and how, which environment names it may set, and the ceilings
//! no job can exceed. A job can only ask for less than this, never more.
//!
//! The policy is identified by the operator's `label` and by the SHA-256 of
//! the file's bytes; every run record carries both, so a record says which
//! version of the rules it ran under.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PolicyFile {
    version: u32,
    label: String,
    service: ServiceFile,
    limits: LimitsFile,
    defaults: Option<LimitsFile>,
    #[serde(default)]
    env_names: Vec<String>,
    #[serde(default)]
    modules: Vec<ModuleFile>,
    #[serde(default)]
    directories: Vec<DirectoryFile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ServiceFile {
    records: String,
    workspaces: String,
    #[serde(default = "default_concurrency")]
    max_concurrent: usize,
    #[serde(default)]
    retain_output: bool,
    #[serde(default = "default_request_bytes")]
    max_request_bytes: usize,
    #[serde(default = "default_os_sandbox")]
    os_sandbox: String,
}
fn default_os_sandbox() -> String { "required".into() }
fn default_concurrency() -> usize { 2 }
fn default_request_bytes() -> usize { 4 * 1024 * 1024 }

#[derive(Deserialize, Default, Clone)]
#[serde(deny_unknown_fields)]
pub(super) struct LimitsFile {
    pub(super) timeout_ms: Option<u64>,
    pub(super) fuel: Option<u64>,
    pub(super) memory_mib: Option<u64>,
    pub(super) max_output_bytes: Option<u64>,
    pub(super) max_input_bytes: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModuleFile { name: String, wasm: String, sha256: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryFile { name: String, host: String, access: Access }

/// How a directory is reachable from inside the guest.
#[derive(Deserialize, Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Access { Read, ReadWrite }

impl Access {
    pub(super) fn name(self) -> &'static str { match self { Access::Read => "read", Access::ReadWrite => "read-write" } }
}

/// Limits a run executes under. Every field is set once resolved.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub(super) struct Limits {
    pub(super) timeout_ms: u64,
    pub(super) fuel: u64,
    pub(super) memory_mib: u64,
    pub(super) max_output_bytes: u64,
    pub(super) max_input_bytes: u64,
}

/// What the platform will accept as a ceiling at all, whatever a policy says:
/// an hour, 4 GiB of linear memory, 64 MiB of output or input.
const HARD: Limits = Limits {
    timeout_ms: 3_600_000,
    fuel: u64::MAX / 2,
    memory_mib: 4096,
    max_output_bytes: 64 * 1024 * 1024,
    max_input_bytes: 64 * 1024 * 1024,
};

pub(super) struct Module { pub(super) path: PathBuf, pub(super) sha256: String }
pub(super) struct Directory { pub(super) host: PathBuf, pub(super) access: Access }

pub(super) struct Policy {
    pub(super) label: String,
    pub(super) sha256: String,
    pub(super) records: PathBuf,
    pub(super) workspaces: PathBuf,
    pub(super) max_concurrent: usize,
    pub(super) retain_output: bool,
    pub(super) max_request_bytes: usize,
    pub(super) os_sandbox: bool,
    pub(super) ceilings: Limits,
    pub(super) defaults: Limits,
    pub(super) env_names: BTreeSet<String>,
    pub(super) modules: BTreeMap<String, Module>,
    pub(super) directories: BTreeMap<String, Directory>,
}

impl Policy {
    pub(super) fn load(path: &Path) -> Result<Policy, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read policy {}: {e}", path.display()))?;
        let text = std::str::from_utf8(&bytes).map_err(|_| "policy must be UTF-8".to_string())?;
        let file: PolicyFile = toml::from_str(text).map_err(|e| format!("policy {}: {e}", path.display()))?;
        if file.version != 1 { return Err(format!("policy version {} is not supported; expected 1", file.version)); }
        if file.label.trim().is_empty() || file.label.len() > 128 { return Err("policy label must be 1-128 characters".into()); }
        let base = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
        let base = std::fs::canonicalize(base).map_err(|e| format!("resolve policy directory: {e}"))?;
        let ceilings = complete(&file.limits, None, "limits")?;
        let defaults = complete(file.defaults.as_ref().unwrap_or(&LimitsFile::default()), Some(ceilings), "defaults")?;
        let (records, workspaces, os_sandbox) = service_settings(&base, &file.service)?;
        let modules = load_modules(&base, file.modules)?;
        let protected = Protected { policy: std::fs::canonicalize(path).map_err(|e| format!("resolve policy: {e}"))?, records: &records, workspaces: &workspaces, modules: &modules };
        let directories = load_directories(&base, file.directories, &protected)?;
        Ok(Policy {
            label: file.label,
            sha256: sha256_hex(&bytes),
            env_names: env_names(file.env_names)?,
            max_concurrent: file.service.max_concurrent,
            retain_output: file.service.retain_output,
            max_request_bytes: file.service.max_request_bytes,
            records,
            workspaces,
            os_sandbox,
            ceilings,
            defaults,
            modules,
            directories,
        })
    }

    /// What a client may see: names, digests and limits, never host paths.
    pub(super) fn summary(&self) -> Value {
        json!({
            "label": self.label,
            "sha256": self.sha256,
            "limits": self.ceilings,
            "defaults": self.defaults,
            "env_names": self.env_names,
            "modules": self.modules.iter().map(|(name, m)| json!({"name": name, "sha256": m.sha256})).collect::<Vec<_>>(),
            "directories": self.directories.iter().map(|(name, d)| json!({"name": name, "access": d.access.name()})).collect::<Vec<_>>(),
            "network": "none",
            "os_sandbox": self.os_sandbox,
            "max_concurrent": self.max_concurrent,
            "retain_output": self.retain_output,
        })
    }

    pub(super) fn identity(&self) -> Value { json!({"label": self.label, "sha256": self.sha256}) }
}

/// What a writable grant must not reach: the policy, the service's state and
/// the modules it would otherwise be able to rewrite.
struct Protected<'a> { policy: PathBuf, records: &'a Path, workspaces: &'a Path, modules: &'a BTreeMap<String, Module> }

fn service_settings(base: &Path, service: &ServiceFile) -> Result<(PathBuf, PathBuf, bool), String> {
    if !(1..=64).contains(&service.max_concurrent) { return Err("service.max_concurrent must be 1-64".into()); }
    if service.max_request_bytes < 1024 || service.max_request_bytes > 256 * 1024 * 1024 {
        return Err("service.max_request_bytes must be between 1 KiB and 256 MiB".into());
    }
    let os_sandbox = match service.os_sandbox.as_str() {
        "required" => true,
        "off" => false,
        other => return Err(format!("service.os_sandbox must be \"required\" or \"off\", not \"{other}\"")),
    };
    let records = state_dir(base, &service.records, "service.records")?;
    let workspaces = state_dir(base, &service.workspaces, "service.workspaces")?;
    if records.starts_with(&workspaces) || workspaces.starts_with(&records) {
        return Err("service.records and service.workspaces must not contain one another".into());
    }
    Ok((records, workspaces, os_sandbox))
}

fn env_names(names: Vec<String>) -> Result<BTreeSet<String>, String> {
    match names.iter().find(|name| !valid_env_name(name)) {
        Some(bad) => Err(format!("env_names: '{bad}' is not a valid variable name")),
        None => Ok(names.into_iter().collect()),
    }
}

/// Every module's bytes must match the digest the policy pins, now.
fn load_modules(base: &Path, listed: Vec<ModuleFile>) -> Result<BTreeMap<String, Module>, String> {
    let mut modules = BTreeMap::new();
    for module in listed {
        if !valid_name(&module.name) { return Err(format!("module name '{}' must be 1-64 of [A-Za-z0-9._-]", module.name)); }
        let wasm = std::fs::canonicalize(base.join(&module.wasm)).map_err(|e| format!("module {}: {}: {e}", module.name, module.wasm))?;
        let expected = module.sha256.to_ascii_lowercase();
        let actual = sha256_hex(&std::fs::read(&wasm).map_err(|e| format!("module {}: {e}", module.name))?);
        if actual != expected { return Err(format!("module {}: sha256 is {actual}, the policy pins {expected}", module.name)); }
        if modules.insert(module.name.clone(), Module { path: wasm, sha256: expected }).is_some() {
            return Err(format!("module {} is listed twice", module.name));
        }
    }
    Ok(modules)
}

fn load_directories(base: &Path, listed: Vec<DirectoryFile>, protected: &Protected) -> Result<BTreeMap<String, Directory>, String> {
    let mut directories = BTreeMap::new();
    for dir in listed {
        if !valid_name(&dir.name) { return Err(format!("directory name '{}' must be 1-64 of [A-Za-z0-9._-]", dir.name)); }
        let host = std::fs::canonicalize(base.join(&dir.host)).map_err(|e| format!("directory {}: {}: {e}", dir.name, dir.host))?;
        if !host.is_dir() { return Err(format!("directory {}: {} is not a directory", dir.name, host.display())); }
        check_grant(&dir.name, &host, dir.access, protected)?;
        if directories.insert(dir.name.clone(), Directory { host, access: dir.access }).is_some() {
            return Err(format!("directory {} is listed twice", dir.name));
        }
    }
    Ok(directories)
}

/// A grant may not overlap the service's state, and a writable one may not
/// hold the policy or a module.
fn check_grant(name: &str, host: &Path, access: Access, protected: &Protected) -> Result<(), String> {
    if let Some(state) = [protected.records, protected.workspaces].into_iter().find(|s| host.starts_with(s) || s.starts_with(host)) {
        return Err(format!("directory {name} overlaps the service state directory {}", state.display()));
    }
    if access == Access::Read { return Ok(()); }
    if protected.policy.starts_with(host) { return Err(format!("directory {name} is writable and holds the policy itself")); }
    match protected.modules.iter().find(|(_, m)| m.path.starts_with(host)) {
        Some((module, _)) => Err(format!("directory {name} is writable and holds module {module}")),
        None => Ok(()),
    }
}

/// Fills every limit: a ceiling must be given, a default falls back to its
/// ceiling, and neither may pass what the platform accepts.
fn complete(file: &LimitsFile, ceilings: Option<Limits>, section: &str) -> Result<Limits, String> {
    let pick = |value: Option<u64>, ceiling: Option<u64>, hard: u64, name: &str| -> Result<u64, String> {
        let value = match (value, ceiling) {
            (Some(v), _) => v,
            (None, Some(c)) => c,
            (None, None) => return Err(format!("{section}.{name} is required")),
        };
        if value == 0 { return Err(format!("{section}.{name} must be greater than zero")); }
        let bound = ceiling.unwrap_or(hard).min(hard);
        if value > bound { return Err(format!("{section}.{name} = {value} exceeds {bound}")); }
        Ok(value)
    };
    Ok(Limits {
        timeout_ms: pick(file.timeout_ms, ceilings.map(|c| c.timeout_ms), HARD.timeout_ms, "timeout_ms")?,
        fuel: pick(file.fuel, ceilings.map(|c| c.fuel), HARD.fuel, "fuel")?,
        memory_mib: pick(file.memory_mib, ceilings.map(|c| c.memory_mib), HARD.memory_mib, "memory_mib")?,
        max_output_bytes: pick(file.max_output_bytes, ceilings.map(|c| c.max_output_bytes), HARD.max_output_bytes, "max_output_bytes")?,
        max_input_bytes: pick(file.max_input_bytes, ceilings.map(|c| c.max_input_bytes), HARD.max_input_bytes, "max_input_bytes")?,
    })
}

/// A state directory is created owner-only when missing, must be writable,
/// and is named by the path the kernel resolves, never by a symlink to it.
fn state_dir(base: &Path, value: &str, field: &str) -> Result<PathBuf, String> {
    let path = base.join(value);
    // Owner-only when porta creates it; an existing directory keeps the
    // permissions the operator gave it.
    if !path.exists() {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&path).map_err(|e| format!("{field}: create {}: {e}", path.display()))?;
    }
    let resolved = std::fs::canonicalize(&path).map_err(|e| format!("{field}: resolve {}: {e}", path.display()))?;
    // Refuse now, not on every job: a state directory the service cannot
    // write (a read-only root filesystem without a volume, say) runs nothing.
    let probe = resolved.join(".porta-write-check");
    std::fs::write(&probe, b"").and_then(|_| std::fs::remove_file(&probe))
        .map_err(|e| format!("{field}: {} is not writable: {e}", resolved.display()))?;
    Ok(resolved)
}

pub(super) fn valid_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 64 && name != "." && name != ".."
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
}

fn valid_env_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.as_bytes()[0].is_ascii_digit()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

pub(super) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}
