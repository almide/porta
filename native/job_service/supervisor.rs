//! The supervisor: turns an accepted job into a workspace and a worker
//! process, stops that process at its hard deadline or on cancel, and writes
//! the run record. It is the same code under `porta job-run` and under
//! `porta job-serve`.
//!
//! A run record is written when the run starts (`status: running`) and again
//! when it ends (`status: finished`). A service that starts and finds a record
//! still `running` marks it `interrupted`; it never runs it again.

use super::*;
use std::process::{Child, Command, Stdio};

mod collect;
mod launch;
use collect::{outcome, stop_reason, stream, usage, Ended, Outputs};
use launch::{kill_group, prepare, probe_os_sandbox, worker_command, Spawner, GRACE};

pub(super) struct Service {
    pub(super) policy: Policy,
    live: Mutex<HashMap<String, Live>>,
    changed: Condvar,
    events: Mutex<()>,
    shutting_down: AtomicBool,
    spawner: Spawner,
}

#[derive(Default)]
struct Live { pid: i32, stop: Option<&'static str> }

/// Why a submission produced no running job, and the HTTP status that says so.
pub(super) struct Rejected { pub(super) status: u16, pub(super) record: Value }

/// A refusal on its way into a record.
struct Refused<'a> { status: u16, code: &'a str, message: String, digest: Option<String> }

/// A worker that has started, and what its record needs when it ends.
struct Running { child: Child, record: Value, workspace: PathBuf, limits: Limits, run_id: String }

impl Service {
    pub(super) fn open(policy: Policy) -> Result<Arc<Service>, String> {
        // As root, file permissions protect nothing a job could reach after a
        // wasmtime escape; porta's other commands refuse root for the same reason.
        if unsafe { libc::geteuid() } == 0 { return Err("refusing to run jobs as root: run porta as an ordinary user".into()); }
        if policy.os_sandbox { probe_os_sandbox(&policy.workspaces)?; }
        let spawner = Spawner::start()?;
        let service = Arc::new(Service { policy, live: Mutex::new(HashMap::new()), changed: Condvar::new(), events: Mutex::new(()), shutting_down: AtomicBool::new(false), spawner });
        service.recover()?;
        Ok(service)
    }

    /// Marks every run a previous service left `running` as interrupted, and
    /// removes its workspace. Nothing is re-run: a job whose effects are
    /// unknown is not repeated behind the client's back.
    fn recover(&self) -> Result<(), String> {
        let entries = std::fs::read_dir(&self.policy.records).map_err(|e| format!("read records: {e}"))?;
        let unfinished = entries.flatten().map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
            .filter_map(|p| read_json(&p).ok().map(|r| (p, r)))
            .filter(|(_, r)| r["status"] == "running" && valid_run_id(r["run_id"].as_str().unwrap_or_default()));
        for (path, mut record) in unfinished {
            let run_id = record["run_id"].as_str().unwrap_or_default().to_string();
            let _ = std::fs::remove_dir_all(self.policy.workspaces.join(&run_id));
            record["status"] = json!("finished");
            record["outcome"] = json!("stopped");
            record["stop_reason"] = json!("interrupted");
            record["detail"] = json!("the service stopped before this run finished; it was not run again");
            record["finished_at"] = json!(rfc3339(SystemTime::now()));
            write_atomic(&path, &pretty(&record))?;
            self.event(&run_id, "interrupted", &record);
        }
        Ok(())
    }

    /// Checks a job and, if the policy accepts it and there is capacity,
    /// starts it. Returns the record as it stands once the worker is running.
    pub(super) fn submit(self: &Arc<Self>, body: &[u8]) -> Result<Value, Rejected> {
        let run_id = new_run_id();
        let mut record = json!({
            "record_version": 1,
            "run_id": run_id,
            "porta_version": util_version(),
            "policy": self.policy.identity(),
            "submitted_at": rfc3339(SystemTime::now()),
        });
        let (mut job, digest) = self.admit(&run_id, body).map_err(|refused| self.reject(record.clone(), refused))?;
        record["job_sha256"] = json!(digest);
        record["module"] = json!({"name": job.module, "sha256": job.module_sha256});
        record["limits"] = json!(job.limits);
        record["grants"] = job.grants();
        record["isolation"] = json!({"wasm": "wasmtime", "worker_process": true, "os_sandbox": self.policy.os_sandbox});
        let workspace = self.policy.workspaces.join(&run_id);
        let child = match self.launch(&workspace, &mut job) {
            Ok(child) => child,
            Err(message) => {
                let _ = std::fs::remove_dir_all(&workspace);
                self.release(&run_id);
                return Err(self.reject(record, Refused { status: 500, code: "workspace_error", message, digest: None }));
            }
        };
        record["status"] = json!("running");
        record["started_at"] = json!(rfc3339(SystemTime::now()));
        if let Some(live) = locked(&self.live).get_mut(&run_id) { live.pid = child.id() as i32; }
        let _ = write_atomic(&self.record_path(&run_id), &pretty(&record));
        self.event(&run_id, "started", &record);
        let running = Running { child, record: record.clone(), workspace, limits: job.limits, run_id };
        let service = Arc::clone(self);
        std::thread::spawn(move || service.supervise(running));
        Ok(record)
    }

    /// The policy's verdict and a reserved slot, or why there is neither.
    fn admit(&self, run_id: &str, body: &[u8]) -> Result<(Resolved, String), Refused<'static>> {
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(Refused { status: 503, code: "service_stopping", message: "the service is shutting down".into(), digest: None });
        }
        let (job, digest) = spec::resolve(&self.policy, body).map_err(|refusal| Refused {
            status: match refusal.code { "request_too_large" => 413, "invalid_job" | "invalid_file_name" => 400, _ => 403 },
            code: refusal.code,
            message: refusal.message,
            digest: Some(sha256_hex(body)),
        })?;
        let mut live = locked(&self.live);
        if live.len() >= self.policy.max_concurrent {
            let message = format!("{0} runs are already running; the policy allows {0}", self.policy.max_concurrent);
            return Err(Refused { status: 429, code: "capacity", message, digest: Some(digest) });
        }
        live.insert(run_id.to_string(), Live::default());
        Ok((job, digest))
    }

    fn launch(&self, workspace: &Path, job: &mut Resolved) -> Result<Child, String> {
        prepare(workspace, job, &self.policy)?;
        let command = worker_command(workspace, job, self.policy.os_sandbox, self.spawner.lifeline())?;
        self.spawner.spawn(command)
    }

    fn reject(&self, mut record: Value, refused: Refused) -> Rejected {
        record["status"] = json!("finished");
        record["outcome"] = json!("refused");
        record["stop_reason"] = json!(refused.code);
        record["detail"] = json!(refused.message);
        record["finished_at"] = json!(rfc3339(SystemTime::now()));
        if let Some(digest) = refused.digest { record["job_sha256"] = json!(digest); }
        if let Some(run_id) = record["run_id"].as_str() {
            let _ = write_atomic(&self.record_path(run_id), &pretty(&record));
            self.event(run_id, "refused", &record);
        }
        Rejected { status: refused.status, record }
    }

    /// Waits for the worker, killing its process group at the hard deadline or
    /// when a stop was asked for.
    fn wait(&self, running: &mut Running) -> Ended {
        let started = Instant::now();
        let hard = Duration::from_millis(running.limits.timeout_ms) + GRACE;
        let mut killed_for = None;
        let status = loop {
            if let Ok(Some(status)) = running.child.try_wait() { break Some(status); }
            if killed_for.is_none() {
                killed_for = self.stop_asked(&running.run_id).or((started.elapsed() > hard).then_some("timeout"));
                if killed_for.is_some() { kill_group(running.child.id() as i32); }
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // `stop` kills the group itself, so the worker can be gone before this
        // loop saw why; the reason asked for still decides.
        Ended { status, killed_for: killed_for.or_else(|| self.stop_asked(&running.run_id)), wall: started.elapsed() }
    }

    fn stop_asked(&self, run_id: &str) -> Option<&'static str> { locked(&self.live).get(run_id).and_then(|l| l.stop) }

    /// Finishes the record and removes the workspace, whatever happened.
    fn supervise(self: Arc<Self>, mut running: Running) {
        let ended = self.wait(&mut running);
        let Running { mut record, workspace, limits, run_id, .. } = running;
        let result: Option<WorkerResult> = std::fs::read(workspace.join("result.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        let (reason, detail) = stop_reason(&ended, result.as_ref(), &workspace, self.policy.os_sandbox);
        record["status"] = json!("finished");
        record["stop_reason"] = json!(reason);
        record["detail"] = json!(detail);
        record["exit_code"] = json!(result.as_ref().and_then(|r| r.exit_code));
        record["usage"] = usage(result.as_ref(), ended.wall);
        let kept = |name: &str| self.policy.retain_output.then(|| self.policy.records.join(&run_id).join(name));
        record["stdout"] = stream(&workspace.join("stdout"), kept("stdout"), limits.max_output_bytes);
        record["stderr"] = stream(&workspace.join("stderr"), kept("stderr"), limits.max_output_bytes.min(256 * 1024));
        let outputs = Outputs::collect(&workspace.join("output"), kept("output"));
        record["output_files"] = json!(outputs.files);
        if reason == "exit" && outputs.total > limits.max_output_bytes {
            record["stop_reason"] = json!("output_limit");
            record["detail"] = json!(format!("output files total {} bytes; the limit is {}", outputs.total, limits.max_output_bytes));
        }
        record["outcome"] = json!(outcome(record["stop_reason"].as_str().unwrap_or_default(), record["exit_code"].as_i64()));
        let _ = std::fs::remove_dir_all(&workspace);
        record["workspace_removed"] = json!(!workspace.exists());
        record["finished_at"] = json!(rfc3339(SystemTime::now()));
        let _ = write_atomic(&self.record_path(&run_id), &pretty(&record));
        self.event(&run_id, "finished", &record);
        self.release(&run_id);
    }

    fn release(&self, run_id: &str) {
        locked(&self.live).remove(run_id);
        self.changed.notify_all();
    }

    /// Asks a running job to stop; false when no such run is running.
    pub(super) fn stop(&self, run_id: &str, reason: &'static str) -> bool {
        let mut live = locked(&self.live);
        let Some(run) = live.get_mut(run_id) else { return false };
        run.stop.get_or_insert(reason);
        kill_group(run.pid);
        drop(live);
        if let Ok(record) = read_json(&self.record_path(run_id)) { self.event(run_id, "stop_requested", &record); }
        true
    }

    /// Stops every running job and waits, up to `within`, for their records.
    pub(super) fn shutdown(&self, within: Duration) {
        self.shutting_down.store(true, Ordering::SeqCst);
        let ids: Vec<String> = locked(&self.live).keys().cloned().collect();
        for id in &ids { self.stop(id, "service_shutdown"); }
        let deadline = Instant::now() + within;
        let mut live = locked(&self.live);
        while !live.is_empty() && Instant::now() < deadline {
            live = self.changed.wait_timeout(live, Duration::from_millis(50)).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
    }

    pub(super) fn record_path(&self, run_id: &str) -> PathBuf { self.policy.records.join(format!("{run_id}.json")) }

    /// The record, waiting up to `wait` for the run to finish.
    pub(super) fn record(&self, run_id: &str, wait: Duration) -> Option<Value> {
        if !valid_run_id(run_id) { return None; }
        let deadline = Instant::now() + wait;
        let mut live = locked(&self.live);
        while live.contains_key(run_id) && Instant::now() < deadline {
            live = self.changed.wait_timeout(live, deadline.saturating_duration_since(Instant::now())).map(|(g, _)| g).unwrap_or_else(|e| e.into_inner().0);
        }
        drop(live);
        read_json(&self.record_path(run_id)).ok()
    }

    pub(super) fn is_live(&self, run_id: &str) -> bool { locked(&self.live).contains_key(run_id) }

    /// Newest first: run ids begin with their UTC start time.
    pub(super) fn list(&self, limit: usize) -> Vec<Value> {
        let mut ids: Vec<String> = std::fs::read_dir(&self.policy.records).into_iter().flatten().flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".json")).map(String::from))
            .filter(|id| valid_run_id(id)).collect();
        ids.sort_unstable_by(|a, b| b.cmp(a));
        ids.into_iter().take(limit).filter_map(|id| read_json(&self.record_path(&id)).ok())
            .map(|r| json!({"run_id": r["run_id"], "status": r["status"], "outcome": r["outcome"], "stop_reason": r["stop_reason"], "module": r["module"]["name"], "submitted_at": r["submitted_at"], "finished_at": r["finished_at"]}))
            .collect()
    }

    /// Removes a finished run's record and retained output. The event log
    /// keeps the line saying it was removed.
    pub(super) fn delete(&self, run_id: &str) -> Result<(), u16> {
        if !valid_run_id(run_id) { return Err(404); }
        if self.is_live(run_id) { return Err(409); }
        let path = self.record_path(run_id);
        let record = read_json(&path).map_err(|_| 404u16)?;
        let _ = std::fs::remove_dir_all(self.policy.records.join(run_id));
        std::fs::remove_file(&path).map_err(|_| 500u16)?;
        self.event(run_id, "deleted", &record);
        Ok(())
    }

    /// One line per state change, appended to `events.jsonl` beside the
    /// records: identifiers, outcome and reason, never inputs or outputs.
    fn event(&self, run_id: &str, event: &str, record: &Value) {
        use std::io::Write;
        let line = json!({
            "at": rfc3339(SystemTime::now()),
            "run_id": run_id,
            "event": event,
            "outcome": record.get("outcome"),
            "stop_reason": record.get("stop_reason"),
            "module": record.get("module").and_then(|m| m.get("name")),
            "policy_sha256": self.policy.sha256,
        });
        let _guard = locked(&self.events);
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.policy.records.join("events.jsonl"));
        if let Ok(mut file) = log { let _ = writeln!(file, "{line}"); }
    }
}
