//! Durable broker-boundary journal. It is not an exactly-once filesystem or
//! remote transaction: an intent without a result is deliberately unrecoverable
//! without operator reconciliation. Replay never executes external operations.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{fs::{File, OpenOptions}, io::{Read, Seek, SeekFrom, Write}, path::{Path, PathBuf}};

const MAX_JOURNAL: u64 = 64 * 1024 * 1024;
pub fn digest(bytes: &[u8]) -> String { format!("{:x}", Sha256::digest(bytes)) }
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope { previous: String, hash: String, payload: Value }
#[derive(Clone)]
struct Exchange { request: Value, response: Value, fuel: u64 }
pub struct Journal {
    file: File, pub path: PathBuf, pub task: String, pub fingerprint: String,
    pub replay_only: bool, pub elapsed_ms: u64,
    previous: String, bytes: u64, exchanges: Vec<Exchange>, cursor: usize,
    pending: Option<Value>, complete: Option<String>, last_record: String,
}
fn open_file(path: &Path, create: bool, writable: bool) -> Result<File, String> {
    let mut options = OpenOptions::new();
    options.read(true).write(writable).create_new(create);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path).map_err(|e| format!("open agent journal: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() { return Err("journal must be a regular file".into()); }
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        if file.metadata().map_err(|e| e.to_string())?.permissions().mode() & 0o077 != 0 {
            return Err("journal permissions must exclude group and other users (chmod 600)".into());
        }
    }
    // Keep this descriptor alive until the entire run finishes. Concurrent
    // resume must not replay the same write intent independently.
    file.try_lock().map_err(|e| format!("agent journal is already in use or cannot be locked: {e}"))?;
    Ok(file)
}
impl Journal {
    pub fn create(path: &Path, fingerprint: &str, task: &str) -> Result<Self, String> {
        let file = open_file(path, true, true)?;
        let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
        let mut journal = Self { file, path, task:task.into(), fingerprint:fingerprint.into(), replay_only:false, elapsed_ms:0,
            previous:String::new(), bytes:0, exchanges:vec![], cursor:0, pending:None, complete:None, last_record:"header".into() };
        journal.append(json!({"kind":"header","version":1,"fingerprint":fingerprint,"task":task}))?;
        // Persist directory entry as well as contents for a newly created file.
        if let Some(parent) = journal.path.parent() {
            File::open(parent).and_then(|f| f.sync_all()).map_err(|e| format!("sync journal directory: {e}"))?;
        }
        Ok(journal)
    }
    pub fn load(path: &Path, replay_only: bool) -> Result<Self, String> {
        Self::load_internal(path, replay_only, false)
    }
    fn load_internal(path: &Path, replay_only: bool, inspection: bool) -> Result<Self, String> {
        let mut file = open_file(path, false, !replay_only && !inspection)?;
        let metadata = file.metadata().map_err(|e| e.to_string())?;
        if metadata.len() > MAX_JOURNAL { return Err("journal exceeds 64 MiB".into()); }
        let mut source = String::new();
        Read::by_ref(&mut file).take(MAX_JOURNAL + 1).read_to_string(&mut source).map_err(|e| format!("read journal: {e}"))?;
        if source.len() as u64 > MAX_JOURNAL || !source.ends_with('\n') { return Err("journal is truncated or oversized; refusing to repeat uncertain operations".into()); }
        let mut previous = String::new();
        let mut exchanges = Vec::new();
        let mut pending: Option<Value> = None;
        let mut header: Option<Value> = None;
        let mut complete = None;
        let mut elapsed_ms = 0;
        let mut last_record = String::new();
        for line in source.lines() {
            let envelope: Envelope = serde_json::from_str(line).map_err(|_| "invalid journal record")?;
            let expected = digest(format!("{}\n{}", previous, envelope.payload).as_bytes());
            if envelope.previous != previous || envelope.hash != expected { return Err("journal hash chain mismatch".into()); }
            previous = envelope.hash;
            let entry = envelope.payload;
            last_record = entry["kind"].as_str().unwrap_or("").into();
            if complete.is_some() { return Err("journal contains records after completion".into()); }
            if header.is_none() {
                if entry["kind"] != "header" || entry["version"] != 1 { return Err("unsupported journal header".into()); }
                header = Some(entry);
                continue;
            }
            match entry["kind"].as_str() {
                Some("intent") => {
                    if pending.is_some() || entry["index"].as_u64() != Some(exchanges.len() as u64) { return Err("invalid journal intent ordering".into()); }
                    pending = Some(entry.get("request").cloned().ok_or("missing journal request")?);
                }
                Some("result") => {
                    let request = pending.take().ok_or("journal result has no intent")?;
                    if entry["index"].as_u64() != Some(exchanges.len() as u64) { return Err("invalid journal result ordering".into()); }
                    let fuel = entry["fuel"].as_u64().ok_or("invalid recorded fuel")?;
                    let elapsed = entry["elapsed_ms"].as_u64().ok_or("invalid recorded elapsed time")?;
                    if elapsed < elapsed_ms { return Err("journal elapsed time moved backwards".into()); }
                    elapsed_ms = elapsed;
                    exchanges.push(Exchange { request, response:entry.get("response").cloned().ok_or("missing journal response")?, fuel });
                }
                Some("checkpoint") => {
                    if pending.is_some() { return Err("checkpoint has an uncertain operation".into()); }
                    let elapsed = entry["elapsed_ms"].as_u64().ok_or("invalid checkpoint elapsed time")?;
                    if elapsed < elapsed_ms { return Err("journal elapsed time moved backwards".into()); }
                    elapsed_ms = elapsed;
                }
                Some("complete") => {
                    if pending.is_some() { return Err("journal completed with an uncertain operation".into()); }
                    complete = Some(entry["output"].as_str().ok_or("invalid recorded final output")?.to_owned());
                    let elapsed = entry["elapsed_ms"].as_u64().ok_or("invalid completion elapsed time")?;
                    if elapsed < elapsed_ms { return Err("journal elapsed time moved backwards".into()); }
                    elapsed_ms = elapsed;
                }
                _ => return Err("unknown journal record kind".into()),
            }
        }
        if pending.is_some() && !inspection { return Err("journal has an intent without a result; operation outcome is uncertain and will not be repeated".into()); }
        let header = header.ok_or("empty journal")?;
        let task = header["task"].as_str().ok_or("missing journal task")?.to_owned();
        let fingerprint = header["fingerprint"].as_str().ok_or("missing journal fingerprint")?.to_owned();
        if replay_only && !inspection && complete.is_none() { return Err("replay requires a completed journal; use agent-resume for a checkpoint".into()); }
        file.seek(SeekFrom::End(0)).map_err(|e| e.to_string())?;
        Ok(Self { file, path:std::fs::canonicalize(path).map_err(|e| e.to_string())?, task, fingerprint, replay_only,
            elapsed_ms, previous, bytes:source.len() as u64, exchanges, cursor:0, pending, complete, last_record })
    }
    pub fn inspect(path: &Path) -> Result<Value, String> {
        let journal = Self::load_internal(path, true, true)?;
        let mut operations = std::collections::BTreeMap::<String, usize>::new();
        let mut actors = std::collections::BTreeMap::<String, usize>::new();
        for exchange in &journal.exchanges {
            *operations.entry(exchange.request["kind"].as_str().unwrap_or("unknown").into()).or_default() += 1;
            *actors.entry(exchange.request["actor"].as_str().unwrap_or("unknown").into()).or_default() += 1;
        }
        let uncertain = journal.pending.as_ref().map(|request| json!({
            "index":journal.exchanges.len(),"actor":request.get("actor").and_then(Value::as_str),
            "kind":request.get("kind").and_then(Value::as_str),"name":request.pointer("/request/name").and_then(Value::as_str)
        }));
        let status = if journal.complete.is_some() { "complete" }
            else if journal.pending.is_some() { "uncertain" }
            else if journal.last_record == "checkpoint" { "checkpoint" }
            else { "incomplete" };
        Ok(json!({"status":status,"hash_chain_verified":true,"replay_verified":false,
            "fingerprint":journal.fingerprint,"bytes":journal.bytes,"recorded_elapsed_ms":journal.elapsed_ms,
            "completed_operations":journal.exchanges.len(),"operations_by_kind":operations,
            "operations_by_actor":actors,"pending_operation":uncertain}))
    }
    fn append(&mut self, payload: Value) -> Result<(), String> {
        if self.replay_only { return Err("replay is read-only".into()); }
        let kind = payload["kind"].as_str().unwrap_or("").to_owned();
        let hash = digest(format!("{}\n{}", self.previous, payload).as_bytes());
        let line = serde_json::to_vec(&Envelope { previous:self.previous.clone(), hash:hash.clone(), payload }).map_err(|e| e.to_string())?;
        if self.bytes + line.len() as u64 + 1 > MAX_JOURNAL { return Err("journal exceeds 64 MiB".into()); }
        self.file.write_all(&line).and_then(|_| self.file.write_all(b"\n")).and_then(|_| self.file.sync_all()).map_err(|e| format!("persist journal: {e}"))?;
        self.bytes += line.len() as u64 + 1;
        self.previous = hash;
        self.last_record = kind;
        Ok(())
    }
    pub fn cached(&mut self, request: &Value) -> Result<Option<(Value, u64)>, String> {
        if self.pending.is_some() { return Err("journal operation is already pending".into()); }
        if let Some(exchange) = self.exchanges.get(self.cursor) {
            if &exchange.request != request { return Err(format!("agent replay diverged at operation {}", self.cursor)); }
            self.cursor += 1;
            return Ok(Some((exchange.response.clone(), exchange.fuel)));
        }
        if self.replay_only || self.complete.is_some() { return Err("agent replay requested an unrecorded operation".into()); }
        Ok(None)
    }
    pub fn begin(&mut self, request: Value) -> Result<Option<(Value, u64)>, String> {
        if let Some(result) = self.cached(&request)? { return Ok(Some(result)); }
        self.append(json!({"kind":"intent","index":self.cursor,"request":request}))?;
        self.pending = Some(request);
        Ok(None)
    }
    pub fn commit(&mut self, response: &Value, fuel: u64, elapsed_ms: u64) -> Result<(), String> {
        let request = self.pending.as_ref().ok_or("journal result has no pending operation")?.clone();
        self.append(json!({"kind":"result","index":self.cursor,"response":response,"fuel":fuel,"elapsed_ms":elapsed_ms}))?;
        self.pending = None;
        self.exchanges.push(Exchange { request, response:response.clone(), fuel });
        self.cursor += 1;
        self.elapsed_ms = elapsed_ms;
        Ok(())
    }
    // A resumed prefix may end after an old verifier pass but before completion.
    // Live completion must recheck current artifacts; offline historical replay must not.
    pub fn next_matches(&self, request: &Value) -> bool {
        self.exchanges.get(self.cursor).is_some_and(|exchange| &exchange.request == request)
    }
    pub fn continuing_live(&self) -> bool {
        !self.replay_only && self.complete.is_none() && self.cursor == self.exchanges.len()
    }
    pub fn checkpoint(&mut self, elapsed_ms: u64) -> Result<(), String> {
        if self.pending.is_some() || self.complete.is_some() { return Err("cannot checkpoint a pending or completed journal".into()); }
        self.append(json!({"kind":"checkpoint","elapsed_ms":elapsed_ms}))?;
        self.elapsed_ms = elapsed_ms;
        Ok(())
    }
    pub fn finish(&mut self, output: &str, elapsed_ms: u64) -> Result<(), String> {
        if self.pending.is_some() || self.cursor != self.exchanges.len() { return Err("agent replay ended before consuming the recorded operations".into()); }
        if let Some(expected) = &self.complete {
            if expected != output { return Err("agent replay final output diverged".into()); }
        } else {
            self.append(json!({"kind":"complete","output":output,"elapsed_ms":elapsed_ms}))?;
            self.complete = Some(output.into());
        }
        Ok(())
    }
}
