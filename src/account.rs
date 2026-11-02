use crate::{
    rpc::{CONSUME, Transport},
    store::{Job, now},
};
use anyhow::{Context, Result, ensure};
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
    ensure!(in_window(), "Preflight exceeded reservation deadline");
    let Some(credit) = choose_credit(&snapshot, now())? else {
        return Ok("noCredit".into());
    };
    ensure!(in_window(), "Reservation outside dispatch window");
    let result = rpc.request(
        CONSUME,
        json!({"idempotencyKey":job.idempotency_key,"creditId":credit}),
    )?;
    let outcome = result["outcome"]
        .as_str()
        .context("Unknown reset response")?;
    ensure!(
        ["reset", "nothingToReset", "noCredit", "alreadyRedeemed"].contains(&outcome),
        "Unknown reset outcome"
    );
    Ok(outcome.into())
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
}
