//! Operator-owned checks. A guest cannot reach these, skip them, or answer
//! for them; a failed verdict is reported back as feedback, not bypassed.

use super::*;

impl Runtime {
    /// Charge one verification step against the team budget before running it.
    pub(super) fn charge_verification_step(&self) -> Result<(), String> {
        let mut budget = locked(&self.budget);
        if budget.steps >= budget.max_steps { return Err("team step budget exceeded during verification".into()); }
        budget.steps += 1;
        budget.verification_calls += 1;
        Ok(())
    }

    /// Run one check, replaying a recorded result when the journal holds one.
    /// Reports the verdict, the fuel it cost, and whether it came from the journal.
    pub(super) fn run_check(&self, name: &str, guest: &Guest, request: &Value, operation: &str)
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
    pub(super) fn journal_expects_recheck(&self, name: &str, input: &Value, operation: &str) -> bool {
        let Some(journal) = &self.journal else { return false };
        let request = json!({"name":name,"input":input});
        let record = json!({"actor":self.actor,"kind":operation,"event":self.event,"decision":self.decision,"request":request});
        let journal = locked(journal);
        journal.continuing_live() || journal.next_matches(&record)
    }

    pub(super) fn verify_checks(&mut self, candidate: &str, proposed: Option<(&str, &Value)>) -> Result<Option<Value>, String> {
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
    pub(super) fn refused_by(&self, name: &str, verdict: Verdict) -> Result<Option<Value>, String> {
        if verdict.passed { return Ok(None); }
        if verdict.feedback.trim().is_empty() { return Err("failed completion check must explain what needs correction".into()); }
        locked(&self.budget).verification_failures += 1;
        Ok(Some(json!({"check":name,"feedback":verdict.feedback})))
    }
}
