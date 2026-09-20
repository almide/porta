//! Read-only views of a loaded run: what it is, what it would be allowed to
//! do, and where its journal may live.

use super::*;

impl Runtime {
    pub(super) fn identity(&self) -> Value {
        let tools: BTreeMap<_, _> = self.tools.iter().map(|(name, guest)| (name.clone(), json!({"wasm":guest.digest,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()}))).collect();
        let children: BTreeMap<_, _> = self.children.iter().map(|(name, child)| (name.clone(), child.identity())).collect();
        let checks: Vec<_> = self.checks.iter().map(|(name, guest, parameters)| json!({"name":name,"wasm":guest.digest,"parameters":parameters,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()})).collect();
        let before_checks: Vec<_> = self.before_checks.iter().map(|(name, guest, parameters)| json!({"name":name,"wasm":guest.digest,"parameters":parameters,"mounts":guest.mounts.iter().map(|m| json!({"host":m.host,"guest":m.guest,"read_only":m.read_only})).collect::<Vec<_>>()})).collect();
        json!({"broker_protocol":5,"before_tool_checks":before_checks,"config":self.config_digest,"agent":self.agent.digest,"tools":tools,"children":children,"checks":checks})
    }
    pub(super) fn inspection(&self, inherited_pins: bool) -> Value {
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
    pub(super) fn check_journal_path(&self, path: &Path) -> Result<(), String> {
        for guest in self.tools.values().chain(self.checks.iter().chain(self.before_checks.iter()).map(|(_, guest, _)| guest)) {
            if guest.mounts.iter().any(|mount| path.starts_with(&mount.host)) {
                return Err("journal must be outside every tool mount to preserve integrity and conversation isolation".into());
            }
        }
        for child in self.children.values() { child.check_journal_path(path)?; }
        Ok(())
    }
}
