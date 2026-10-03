//! A job, version 1: which registered module to run, its input, the files and
//! directories it may touch, the environment it gets, and its limits. A job
//! is checked against the policy before anything is created for it; a job
//! that asks for more than the policy grants is refused, never narrowed.

use super::*;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JobFile {
    version: u32,
    module: String,
    #[serde(default)]
    input: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default)]
    directories: Vec<DirectoryRequest>,
    #[serde(default = "yes")]
    output: bool,
    #[serde(default)]
    limits: LimitsFile,
}
fn yes() -> bool { true }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectoryRequest { name: String, access: Access }

/// Why a job never ran. `code` is stable for clients; `message` is for people.
#[derive(Debug)]
pub(super) struct Refusal { pub(super) code: &'static str, pub(super) message: String }

fn refuse<T>(code: &'static str, message: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal { code, message: message.into() })
}

/// One directory the guest will see, as the worker opens it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct Mount { pub(super) host: PathBuf, pub(super) guest: String, pub(super) writable: bool }

/// A job after the policy has accepted it: everything the worker needs, and
/// nothing it could use to reach further.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(super) struct Resolved {
    pub(super) module: String,
    pub(super) module_path: PathBuf,
    pub(super) module_sha256: String,
    pub(super) input: String,
    pub(super) args: Vec<String>,
    pub(super) env: Vec<(String, String)>,
    pub(super) files: BTreeMap<String, String>,
    pub(super) directories: Vec<(String, Access)>,
    pub(super) output: bool,
    pub(super) limits: Limits,
    pub(super) mounts: Vec<Mount>,
}

impl Resolved {
    /// What the record says was granted: names and access, never values.
    pub(super) fn grants(&self) -> Value {
        json!({
            "input_files": self.files.keys().collect::<Vec<_>>(),
            "output": self.output,
            "directories": self.directories.iter().map(|(n, a)| json!({"name": n, "access": a.name(), "guest_path": format!("/data/{n}")})).collect::<Vec<_>>(),
            "env_names": self.env.iter().map(|(n, _)| n).collect::<Vec<_>>(),
            "network": "none",
            "host_functions": "wasi",
        })
    }
}

const MAX_ARGS: usize = 64;
const MAX_FILES: usize = 64;

/// Parses and checks one job. The digest is of the canonical JSON, so the
/// same job always has the same digest whatever its whitespace or key order.
pub(super) fn resolve(policy: &Policy, body: &[u8]) -> Result<(Resolved, String), Refusal> {
    if body.len() > policy.max_request_bytes { return refuse("request_too_large", format!("the job is {} bytes; the policy accepts {}", body.len(), policy.max_request_bytes)); }
    let value: Value = serde_json::from_slice(body).or_else(|e| refuse("invalid_job", format!("not JSON: {e}")))?;
    let digest = sha256_hex(value.to_string().as_bytes());
    let job: JobFile = serde_json::from_value(value).or_else(|e| refuse("invalid_job", e.to_string()))?;
    if job.version != 1 { return refuse("invalid_job", format!("job version {} is not supported; expected 1", job.version)); }
    let Some(module) = policy.modules.get(&job.module) else { return refuse("module_not_registered", format!("module '{}' is not in the policy", job.module)) };
    let limits = limits(policy, &job.limits)?;
    check_inputs(policy, &job, &limits)?;
    let directories = directories(policy, &job.directories)?;
    let resolved = Resolved {
        module: job.module.clone(),
        module_path: module.path.clone(),
        module_sha256: module.sha256.clone(),
        input: job.input,
        args: job.args,
        env: job.env.into_iter().collect(),
        files: job.files,
        directories,
        output: job.output,
        limits,
        mounts: Vec::new(),
    };
    Ok((resolved, digest))
}

/// Arguments, environment names, file names and the total input size.
fn check_inputs(policy: &Policy, job: &JobFile, limits: &Limits) -> Result<(), Refusal> {
    if job.args.len() > MAX_ARGS { return refuse("invalid_job", format!("at most {MAX_ARGS} arguments")); }
    if let Some(name) = job.env.keys().find(|name| !policy.env_names.contains(*name)) {
        return refuse("env_not_allowed", format!("environment variable {name} is not allowed by the policy"));
    }
    if job.files.len() > MAX_FILES { return refuse("invalid_job", format!("at most {MAX_FILES} input files")); }
    if let Some(name) = job.files.keys().find(|name| !valid_name(name)) {
        return refuse("invalid_file_name", format!("input file name '{name}' must be one path component of [A-Za-z0-9._-]"));
    }
    let input_bytes = job.input.len() + job.files.values().map(String::len).sum::<usize>()
        + job.args.iter().map(String::len).sum::<usize>() + job.env.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>();
    if input_bytes as u64 > limits.max_input_bytes { return refuse("input_too_large", format!("input is {input_bytes} bytes; the limit is {}", limits.max_input_bytes)); }
    Ok(())
}

/// Each requested directory must be in the policy, at no more than the access
/// it grants, and asked for once.
fn directories(policy: &Policy, requests: &[DirectoryRequest]) -> Result<Vec<(String, Access)>, Refusal> {
    let mut granted: Vec<(String, Access)> = Vec::new();
    for request in requests {
        let Some(grant) = policy.directories.get(&request.name) else { return refuse("directory_not_granted", format!("directory '{}' is not in the policy", request.name)) };
        if request.access == Access::ReadWrite && grant.access == Access::Read {
            return refuse("access_exceeds_grant", format!("directory '{}' is granted read-only; the job asked for read-write", request.name));
        }
        if granted.iter().any(|(name, _)| name == &request.name) { return refuse("invalid_job", format!("directory '{}' is requested twice", request.name)); }
        granted.push((request.name.clone(), request.access));
    }
    Ok(granted)
}

fn limits(policy: &Policy, asked: &LimitsFile) -> Result<Limits, Refusal> {
    let pick = |value: Option<u64>, default: u64, ceiling: u64, name: &str| -> Result<u64, Refusal> {
        let value = value.unwrap_or(default);
        if value == 0 { return refuse("invalid_job", format!("limits.{name} must be greater than zero")); }
        if value > ceiling { return refuse("limit_exceeds_ceiling", format!("limits.{name} = {value} exceeds the policy ceiling {ceiling}")); }
        Ok(value)
    };
    let (d, c) = (policy.defaults, policy.ceilings);
    Ok(Limits {
        timeout_ms: pick(asked.timeout_ms, d.timeout_ms, c.timeout_ms, "timeout_ms")?,
        fuel: pick(asked.fuel, d.fuel, c.fuel, "fuel")?,
        memory_mib: pick(asked.memory_mib, d.memory_mib, c.memory_mib, "memory_mib")?,
        max_output_bytes: pick(asked.max_output_bytes, d.max_output_bytes, c.max_output_bytes, "max_output_bytes")?,
        max_input_bytes: pick(asked.max_input_bytes, d.max_input_bytes, c.max_input_bytes, "max_input_bytes")?,
    })
}
