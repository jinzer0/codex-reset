use crate::{
    rpc::{CONSUME, Transport, diagnostic_text},
    store::{Job, now},
};
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub fingerprint: String,
    pub email: String,
    pub plan: String,
    pub limits: Value,
}
impl Snapshot {
    pub fn report(&self) -> Value {
        let (local, domain) = self.email.split_once('@').unwrap_or((&self.email, ""));
        let first = local.chars().next().unwrap_or('*');
        json!({"account_fingerprint":self.fingerprint,"email":format!("{first}***@{domain}"),"plan":self.plan,
   "available_reset_credits":self.limits.pointer("/rateLimitResetCredits/availableCount"),
   "ordinary_usage_allowed":self.limits.get("ordinaryUsageAllowed"),"rate_limits":self.limits.get("rateLimits")})
    }
}

#[derive(Debug, Serialize)]
pub struct ResetEvidence {
    pub phase: String,
    pub before: Option<Value>,
    pub credit_expires_at: Option<f64>,
    pub request_started_at: Option<f64>,
    pub response_received_at: Option<f64>,
    pub server_outcome: Option<String>,
    pub dispatch_error: Option<String>,
    pub verification: Verification,
}

impl Default for ResetEvidence {
    fn default() -> Self {
        Self {
            phase: "preflight".into(),
            before: None,
            credit_expires_at: None,
            request_started_at: None,
            response_received_at: None,
            server_outcome: None,
            dispatch_error: None,
            verification: Verification {
                status: "not_requested".into(),
                attempts: Vec::new(),
                usage_reset_observed: None,
                credit_count_decreased: None,
                natural_reset_possible: false,
            },
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Verification {
    pub status: String,
    pub attempts: Vec<Readback>,
    pub usage_reset_observed: Option<bool>,
    pub credit_count_decreased: Option<bool>,
    pub natural_reset_possible: bool,
}

#[derive(Debug, Serialize)]
pub struct Readback {
    pub observed_at: f64,
    pub snapshot: Option<Value>,
    pub error: Option<String>,
}

impl ResetEvidence {
    /// Only observes account state. A missing response never authorizes another reset.
    pub fn observe(&mut self, result: Result<Snapshot>) {
        self.phase = "verification".into();
        let observed_at = now();
        let snapshot = match result {
            Ok(snapshot) => snapshot.report(),
            Err(error) => {
                self.verification.status = "read_failed".into();
                self.verification.attempts.push(Readback {
                    observed_at,
                    snapshot: None,
                    error: Some(diagnostic_text(&format!("readback: {error:#}"))),
                });
                return;
            }
        };
        if let Some(before) = &self.before {
            let (usage, natural_reset_possible) = compare_usage(before, &snapshot, observed_at);
            let verification = &mut self.verification;
            verification.usage_reset_observed = usage;
            verification.credit_count_decreased = before["available_reset_credits"]
                .as_u64()
                .zip(snapshot["available_reset_credits"].as_u64())
                .map(|(before, after)| after < before);
            verification.natural_reset_possible = natural_reset_possible
                || self
                    .credit_expires_at
                    .is_some_and(|expiry| expiry <= observed_at);
            verification.status = if self.server_outcome.as_deref() == Some("reset")
                && usage == Some(true)
                && verification.credit_count_decreased == Some(true)
                && !verification.natural_reset_possible
            {
                "confirmed"
            } else if usage == Some(false) {
                "not_observed"
            } else {
                "inconclusive"
            }
            .into();
        }
        self.verification.attempts.push(Readback {
            observed_at,
            snapshot: Some(snapshot),
            error: None,
        });
    }
}

/// A zero baseline, missing windows, or changed limit identity cannot prove a reset.
fn compare_usage(before: &Value, after: &Value, observed_at: f64) -> (Option<bool>, bool) {
    let before = &before["rate_limits"];
    let after = &after["rate_limits"];
    if !before.is_object() || !after.is_object() || before["limitId"] != after["limitId"] {
        return (None, true);
    }
    let mut positive = false;
    let mut reset = true;
    let mut natural_reset_possible = false;
    for name in ["primary", "secondary"] {
        let window = &before[name];
        if window.is_null() {
            continue;
        }
        let current = &after[name];
        let percent = |value: &Value| {
            value["usedPercent"]
                .as_f64()
                .filter(|n| (0.0..=100.0).contains(n))
        };
        let Some((used_before, used_after)) = percent(window).zip(percent(current)) else {
            return (None, true);
        };
        if window["windowDurationMins"].as_u64().is_none()
            || window["windowDurationMins"] != current["windowDurationMins"]
        {
            return (None, true);
        }
        if used_before > 0.0 {
            positive = true;
            reset &= used_after == 0.0;
            natural_reset_possible |= window["resetsAt"]
                .as_f64()
                .is_none_or(|at| at <= observed_at);
        }
    }
    (positive.then_some(reset), natural_reset_possible)
}
fn valid_https_origin(origin: &str) -> bool {
    if origin.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some(rest) = origin.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let (host, port) = if let Some(ipv6) = authority.strip_prefix('[') {
        let Some((host, tail)) = ipv6.split_once(']') else {
            return false;
        };
        if host.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        if tail.is_empty() {
            return true;
        }
        let Some(port) = tail.strip_prefix(':') else {
            return false;
        };
        return port.parse::<u16>().is_ok_and(|p| p > 0);
    } else if let Some((host, port)) = authority.split_once(':') {
        (host, Some(port))
    } else {
        (authority, None)
    };
    if port.is_some_and(|p| !p.parse::<u16>().is_ok_and(|n| n > 0)) {
        return false;
    }
    !host.is_empty()
        && host.len() <= 253
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

fn identity(response: &Value) -> Result<(String, String)> {
    ensure!(
        response.pointer("/account/type").and_then(Value::as_str) == Some("chatgpt"),
        "A ChatGPT login is required"
    );
    let id = response
        .pointer("/workspaceRouting/chatgptAccountId")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("Selected account ID is missing")?;
    let email = response
        .pointer("/account/email")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .context("Account email is missing")?;
    let origin = response
        .pointer("/workspaceRouting/backendOrigin")
        .and_then(Value::as_str)
        .context("Workspace origin missing")?;
    ensure!(
        valid_https_origin(origin),
        "Workspace must have a valid HTTPS host"
    );
    let bytes = serde_json::to_vec(&json!([id, email.to_lowercase()]))?;
    Ok((id.into(), format!("{:x}", Sha256::digest(bytes))))
}
pub fn read_snapshot(rpc: &mut impl Transport, expected: Option<&str>) -> Result<Snapshot> {
    let first = rpc.request("account/read", json!({"refreshToken":false}))?;
    let (account_id, fingerprint) = identity(&first)?;
    if let Some(expected) = expected {
        ensure!(
            expected == fingerprint,
            "Signed-in account differs from reservation"
        );
    }
    let limits = rpc.request(
        "account/rateLimits/read",
        json!({"excludeResetCreditDetails":false}),
    )?;
    ensure!(
        limits.get("accountId").and_then(Value::as_str) == Some(&account_id),
        "Usage response account mismatch"
    );
    let second = rpc.request("account/read", json!({"refreshToken":false}))?;
    ensure!(
        identity(&second)?.1 == fingerprint,
        "Account changed during preflight"
    );
    Ok(Snapshot {
        fingerprint,
        email: first["account"]["email"]
            .as_str()
            .context("Email missing")?
            .into(),
        plan: first["account"]["planType"]
            .as_str()
            .unwrap_or("unknown")
            .into(),
        limits,
    })
}
pub fn choose_credit(snapshot: &Snapshot, at: f64) -> Result<Option<String>> {
    let summary = snapshot
        .limits
        .get("rateLimitResetCredits")
        .context("Reset-credit availability unknown")?;
    let count = summary
        .get("availableCount")
        .and_then(Value::as_u64)
        .context("Reset-credit count unknown")?;
    if count == 0 {
        return Ok(None);
    }
    let credits = summary
        .get("credits")
        .and_then(Value::as_array)
        .context("Reset-credit details required")?;
    let mut valid = Vec::new();
    for credit in credits {
        if credit["status"] != "available" || credit["resetType"] != "codexRateLimits" {
            continue;
        }
        let Some(id) = credit["id"].as_str().filter(|s| !s.trim().is_empty()) else {
            continue;
        };
        let expiry = match credit.get("expiresAt") {
            None | Some(Value::Null) => f64::INFINITY,
            Some(value) => match value.as_f64().filter(|n| n.is_finite() && *n > at) {
                Some(n) => n,
                None => continue,
            },
        };
        valid.push((expiry, id));
    }
    valid.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)));
    Ok(Some(
        valid
            .first()
            .context("No unexpired available Codex reset credit")?
            .1
            .to_owned(),
    ))
}
pub fn consume_checked(rpc: &mut impl Transport, job: &Job) -> Result<String> {
    consume_recorded(rpc, job, &mut ResetEvidence::default(), |_| Ok(()))
}

/// Persist the pre-request snapshot and response before any post-request reads.
pub fn consume_recorded(
    rpc: &mut impl Transport,
    job: &Job,
    evidence: &mut ResetEvidence,
    mut record: impl FnMut(&ResetEvidence) -> Result<()>,
) -> Result<String> {
    let result = (|| {
        ensure!(
            job.status == "claimed",
            "A durable scheduler claim is required"
        );
        let in_window = || {
            let t = now();
            t >= job.due_at && t <= job.due_at + job.grace_seconds
        };
        ensure!(in_window(), "Reservation outside dispatch window");
        let snapshot = read_snapshot(rpc, Some(&job.binding.account_fingerprint))?;
        let mut before = snapshot.report();
        before["observed_at"] = json!(now());
        evidence.before = Some(before);
        ensure!(in_window(), "Preflight exceeded reservation deadline");
        let Some(credit) = choose_credit(&snapshot, now())? else {
            record(evidence).context("Persist no-credit observation")?;
            return Ok("noCredit".into());
        };
        evidence.credit_expires_at = snapshot.limits["rateLimitResetCredits"]["credits"]
            .as_array()
            .and_then(|credits| credits.iter().find(|entry| entry["id"] == credit))
            .and_then(|credit| credit["expiresAt"].as_f64());
        evidence.phase = "consume".into();
        evidence.request_started_at = Some(now());
        record(evidence).context("Persist evidence before dispatch")?;
        ensure!(in_window(), "Reservation outside dispatch window");
        let result = rpc.request(
            CONSUME,
            json!({"idempotencyKey":job.idempotency_key,"creditId":credit}),
        )?;
        evidence.phase = "response".into();
        evidence.response_received_at = Some(now());
        evidence.server_outcome = result["outcome"].as_str().map(diagnostic_text);
        let outcome = result["outcome"]
            .as_str()
            .context("Unknown reset response")?;
        ensure!(
            ["reset", "nothingToReset", "noCredit", "alreadyRedeemed"].contains(&outcome),
            "Unknown reset outcome"
        );
        Ok(outcome.into())
    })();
    if let Err(error) = &result {
        evidence.dispatch_error = Some(diagnostic_text(&format!("{}: {error:#}", evidence.phase)));
    }
    if let Err(error) = record(evidence) {
        let dispatch = match &result {
            Ok(outcome) => format!("server outcome={outcome}"),
            Err(error) => format!("{error:#}"),
        };
        return Err(error.context(format!("Persist dispatch result ({dispatch}); no retry")));
    }
    result
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    struct Fake {
        responses: VecDeque<Value>,
        methods: Vec<String>,
    }
    impl Transport for Fake {
        fn request(&mut self, m: &str, _p: Value) -> Result<Value> {
            self.methods.push(m.into());
            self.responses.pop_front().context("No fixture")
        }
    }
    fn account(id: &str) -> Value {
        json!({"account":{"type":"chatgpt","email":"mock@example.invalid","planType":"pro"},"workspaceRouting":{"chatgptAccountId":id,"backendOrigin":"https://example.invalid"}})
    }
    fn snapshot() -> Snapshot {
        Snapshot {
            fingerprint: String::new(),
            email: "mock@example.invalid".into(),
            plan: "pro".into(),
            limits: json!({"rateLimitResetCredits":{"availableCount":1,"credits":[{"id":"credit","status":"available","resetType":"codexRateLimits","expiresAt":100}]}}),
        }
    }
    #[test]
    fn rejects_malformed_workspace_origins() {
        for origin in [
            "https://?x",
            "https://#x",
            "https:///path",
            "https://",
            "https://bad host",
            "http://example.invalid",
            "https://host:bad",
            "https://user@host",
        ] {
            assert!(!valid_https_origin(origin), "{origin}");
        }
        assert!(valid_https_origin("https://chatgpt.com/backend-api/"));
        assert!(valid_https_origin("https://[::1]:443/"));
    }
    #[test]
    fn reads_only_and_checks_identity() {
        let mut f = Fake {
            responses: VecDeque::from([
                account("test"),
                json!({"accountId":"test"}),
                account("test"),
            ]),
            methods: Vec::new(),
        };
        read_snapshot(&mut f, None).unwrap();
        assert!(!f.methods.iter().any(|m| m == CONSUME));
    }
    #[test]
    fn mismatch_stops_before_usage() {
        let mut f = Fake {
            responses: VecDeque::from([account("test")]),
            methods: Vec::new(),
        };
        assert!(read_snapshot(&mut f, Some("wrong")).is_err());
        assert_eq!(f.methods.len(), 1);
    }
    #[test]
    fn usage_mismatch_refused() {
        let mut f = Fake {
            responses: VecDeque::from([account("test"), json!({"accountId":"other"})]),
            methods: Vec::new(),
        };
        assert!(read_snapshot(&mut f, None).is_err());
    }
    #[test]
    fn expired_credit_refused() {
        assert!(choose_credit(&snapshot(), 101.).is_err());
        assert_eq!(
            choose_credit(&snapshot(), 99.).unwrap(),
            Some("credit".into())
        );
    }
    #[test]
    fn no_credit_is_noop() {
        let mut s = snapshot();
        s.limits["rateLimitResetCredits"]["availableCount"] = json!(0);
        assert!(choose_credit(&s, 99.).unwrap().is_none());
    }
    #[test]
    fn mask_email() {
        assert_eq!(snapshot().report()["email"], "m***@example.invalid");
    }

    fn usage_snapshot(primary: f64, secondary: f64, credits: u64) -> Snapshot {
        let mut snapshot = snapshot();
        snapshot.limits["rateLimitResetCredits"]["availableCount"] = json!(credits);
        snapshot.limits["rateLimits"] = json!({
            "limitId": "codex",
            "primary": {"usedPercent":primary,"windowDurationMins":300,"resetsAt":now()+3600.0},
            "secondary": {"usedPercent":secondary,"windowDurationMins":10080,"resetsAt":now()+86400.0}
        });
        snapshot
    }

    fn evidence() -> ResetEvidence {
        ResetEvidence {
            before: Some(usage_snapshot(80.0, 75.0, 1).report()),
            server_outcome: Some("reset".into()),
            ..ResetEvidence::default()
        }
    }

    #[test]
    fn verification_requires_both_usage_change_and_credit_change() {
        let mut report = evidence();
        report.observe(Ok(usage_snapshot(0.0, 0.0, 1)));
        assert_eq!(report.verification.status, "inconclusive");
        report.observe(Ok(usage_snapshot(0.0, 75.0, 0)));
        assert_eq!(report.verification.status, "not_observed");
        report.observe(Ok(usage_snapshot(0.0, 0.0, 0)));
        assert_eq!(report.verification.status, "confirmed");
    }

    #[test]
    fn natural_rollover_or_credit_expiry_is_not_confirmed() {
        let mut report = evidence();
        report.before.as_mut().unwrap()["rate_limits"]["primary"]["resetsAt"] = json!(now() - 1.0);
        report.observe(Ok(usage_snapshot(0.0, 0.0, 0)));
        assert_eq!(report.verification.status, "inconclusive");
        assert!(report.verification.natural_reset_possible);
        let mut report = evidence();
        report.credit_expires_at = Some(now() - 1.0);
        report.observe(Ok(usage_snapshot(0.0, 0.0, 0)));
        assert_eq!(report.verification.status, "inconclusive");
    }

    #[test]
    fn zero_baseline_and_incomplete_or_different_usage_are_inconclusive() {
        let mut report = evidence();
        report.before = Some(usage_snapshot(0.0, 0.0, 1).report());
        report.observe(Ok(usage_snapshot(0.0, 0.0, 0)));
        assert_eq!(report.verification.status, "inconclusive");
        for field in ["primary", "secondary"] {
            let mut report = evidence();
            let mut after = usage_snapshot(0.0, 0.0, 0);
            after.limits["rateLimits"][field] = Value::Null;
            report.observe(Ok(after));
            assert_eq!(report.verification.status, "inconclusive");
        }
        let mut report = evidence();
        let mut after = usage_snapshot(0.0, 0.0, 0);
        after.limits["rateLimits"]["limitId"] = json!("another-limit");
        report.observe(Ok(after));
        assert_eq!(report.verification.status, "inconclusive");
    }

    #[test]
    fn observed_change_without_reset_response_does_not_confirm_success() {
        for outcome in [
            None,
            Some("nothingToReset"),
            Some("alreadyRedeemed"),
            Some("unexpected"),
        ] {
            let mut report = evidence();
            report.server_outcome = outcome.map(str::to_owned);
            report.observe(Ok(usage_snapshot(0.0, 0.0, 0)));
            assert_eq!(report.verification.status, "inconclusive");
            assert_eq!(report.verification.usage_reset_observed, Some(true));
        }
    }

    #[test]
    fn readback_errors_preserve_server_response_and_safe_details() {
        let mut report = evidence();
        report.observe(Err(anyhow::anyhow!(
            "usage lookup failed: authorization: Bearer private-value"
        )));
        assert_eq!(report.server_outcome.as_deref(), Some("reset"));
        assert_eq!(report.verification.status, "read_failed");
        let error = report.verification.attempts[0].error.as_deref().unwrap();
        assert!(error.contains("usage lookup failed"));
        assert!(!error.contains("private-value"));
    }
}
