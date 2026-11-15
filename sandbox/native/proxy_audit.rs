//! What the proxy decided, and where that is recorded.
//!
//! Every CONNECT the proxy sees produces exactly one decision, whether it
//! was tunnelled or refused. It always reaches stderr, so an operator
//! watching the run sees it, and reaches the audit file as one JSON line
//! per decision when the run asked for a trail.

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// One proxy decision, exactly as it is reported and recorded.
pub(crate) struct Decision {
    host: String,
    port: u16,
    verdict: &'static str,
    reason: String,
}

impl Decision {
    pub(crate) fn new(host: &str, port: u16, verdict: &'static str, reason: impl Into<String>) -> Self {
        Self { host: host.to_string(), port, verdict, reason: reason.into() }
    }
}

pub(crate) fn audit_log(audit_path: &Option<String>, decision: &Decision) {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    eprintln!("[porta proxy] {} {}:{} ({})", decision.verdict, decision.host, decision.port, decision.reason);
    if let Some(p) = audit_path {
        let line = format!(
            "{{\"ts\":{},\"host\":{},\"port\":{},\"decision\":{},\"reason\":{}}}\n",
            ts,
            serde_json::to_string(&decision.host).unwrap_or_else(|_| "\"\"".into()),
            decision.port,
            serde_json::to_string(decision.verdict).unwrap_or_else(|_| "\"\"".into()),
            serde_json::to_string(&decision.reason).unwrap_or_else(|_| "\"\"".into()),
        );
        // Written and synced before the connection proceeds: a record that a
        // crash can lose is a trail with a gap exactly where it mattered.
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = f.write_all(line.as_bytes());
            let _ = f.sync_data();
        }
    }
}
