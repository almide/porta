//! The surface the Almide side binds to. Each entry point answers with JSON
//! and never lets an error escape as a panic.

use super::*;

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
