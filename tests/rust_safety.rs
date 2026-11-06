//! End-to-end safety checks with a shell-only, local stdio RPC fixture.
//! No Codex binary, authentication store, network connection or OS job is used.
use codex_reset::{
    account::{ResetEvidence, consume_checked, consume_recorded, read_snapshot},
    rpc::{CONSUME, Session, Transport},
    store::{Binding, Job, Store, now},
};
use serde_json::{Value, json};
use std::{fs, path::PathBuf, process::Command, thread, time::Duration};

struct FakeServer {
    root: tempfile::TempDir,
    script: PathBuf,
    log: PathBuf,
}

impl FakeServer {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let script = root.path().join("fake-app-server.sh");
        let log = root.path().join("requests.jsonl");
        fs::write(&script, FAKE_SERVER).unwrap();
        Self { root, script, log }
    }

    fn start(&self, mode: &str, allow_reset: bool) -> Session {
        let temporary = tempfile::tempdir_in(self.root.path()).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .arg(&self.script)
            .arg(&self.log)
            .arg(mode)
            .env_clear()
            .current_dir(temporary.path());
        Session::start(command, temporary, allow_reset).unwrap()
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(&self.log)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn consume_requests(&self) -> Vec<Value> {
        self.requests()
            .into_iter()
            .filter(|request| request["method"] == CONSUME)
            .collect()
    }

    fn store(&self) -> Store {
        Store::open(&self.root.path().join("state")).unwrap()
    }
}

fn binding(rpc: &mut Session) -> Binding {
    let account = read_snapshot(rpc, None).unwrap();
    Binding {
        account_fingerprint: account.fingerprint,
        codex_home: PathBuf::from("/synthetic/codex-home"),
        binary_path: PathBuf::from("/synthetic/codex"),
        binary_sha256: "synthetic-binary-digest".into(),
        binary_version: "0.165.0".into(),
    }
}

fn claimed_job(store: &Store, binding: Binding) -> Job {
    let job = store.create(now() + 0.05, binding).unwrap();
    let deadline = now() + 2.0;
    loop {
        if let Some(job) = store.claim(&job.id).unwrap() {
            return job;
        }
        assert!(
            now() < deadline,
            "Test fixture did not reach its claim time"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn readonly_session_blocks_consume_before_writing_to_stdin() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", false);
    let snapshot = read_snapshot(&mut rpc, None).unwrap();
    assert_eq!(snapshot.email, "mock@example.invalid");
    let sent_before = rpc.sent_methods.clone();
    assert!(
        rpc.request(CONSUME, json!({"idempotencyKey":"must-not-be-sent"}))
            .is_err()
    );
    assert_eq!(rpc.sent_methods, sent_before);
    assert!(!rpc.reset_sent());
    // A subsequent response synchronizes the fixture after all preceding writes.
    rpc.request("account/read", json!({"refreshToken": false}))
        .unwrap();
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
    assert!(fixture.requests().iter().all(|request| {
        [
            "initialize",
            "initialized",
            "account/read",
            "account/rateLimits/read",
        ]
        .contains(&request["method"].as_str().unwrap())
    }));
}

#[test]
fn durable_claim_and_session_gate_allow_exactly_one_fake_reset() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    assert_eq!(
        Store::open(&fixture.root.path().join("state"))
            .unwrap()
            .show(&job.id)
            .unwrap()
            .status,
        "claimed"
    );
    let outcome = consume_checked(&mut rpc, &job).unwrap();
    assert_eq!(outcome, "reset");
    assert!(rpc.reset_sent());
    let finished = store.finish(&job.id, Some(&outcome), None).unwrap();
    assert_eq!(finished.status, "completed");
    assert!(store.claim(&job.id).unwrap().is_none());
    assert!(
        Store::open(&fixture.root.path().join("state"))
            .unwrap()
            .claim(&job.id)
            .unwrap()
            .is_none()
    );
    let before_duplicate = rpc.sent_methods.clone();
    assert!(
        rpc.request(CONSUME, json!({"idempotencyKey":job.idempotency_key}))
            .is_err()
    );
    assert_eq!(rpc.sent_methods, before_duplicate);
    drop(rpc);
    let requests = fixture.consume_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0]["params"],
        json!({
            "idempotencyKey": job.idempotency_key,
            "creditId": "mock-credit",
        })
    );
}

#[test]
fn account_mismatch_stops_before_a_reset_write() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", true);
    let store = fixture.store();
    let mut account = binding(&mut rpc);
    account.account_fingerprint = "different-account-fingerprint".into();
    let job = claimed_job(&store, account);
    assert!(consume_checked(&mut rpc, &job).is_err());
    assert!(!rpc.reset_sent());
    let result = store
        .finish(&job.id, None, Some("AccountMismatch"))
        .unwrap();
    assert_eq!(result.status, "unknown");
    assert!(store.claim(&job.id).unwrap().is_none());
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
}

#[test]
fn expired_credit_stops_before_a_reset_write() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("expired-credit", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    assert!(consume_checked(&mut rpc, &job).is_err());
    assert!(!rpc.reset_sent());
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
}

#[test]
fn zero_credits_finishes_without_consuming() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("no-credit", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    let outcome = consume_checked(&mut rpc, &job).unwrap();
    assert_eq!(outcome, "noCredit");
    assert_eq!(
        store.finish(&job.id, Some(&outcome), None).unwrap().status,
        "completed"
    );
    assert!(!rpc.reset_sent());
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
}

#[test]
fn lost_response_is_unknown_and_cannot_be_retried() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("lost-response", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    assert!(consume_checked(&mut rpc, &job).is_err());
    assert!(rpc.reset_sent());
    let finished = store
        .finish(&job.id, None, Some("AppServerExited"))
        .unwrap();
    assert_eq!(finished.status, "unknown");
    assert_eq!(finished.error.as_deref(), Some("AppServerExited"));
    assert!(store.claim(&job.id).unwrap().is_none());
    let reopened = Store::open(&fixture.root.path().join("state")).unwrap();
    assert!(reopened.claim(&job.id).unwrap().is_none());
    let before_retry = rpc.sent_methods.clone();
    assert!(
        rpc.request(CONSUME, json!({"idempotencyKey":job.idempotency_key}))
            .is_err()
    );
    assert_eq!(rpc.sent_methods, before_retry);
    drop(rpc);
    assert_eq!(fixture.consume_requests().len(), 1);
}

#[test]
fn before_snapshot_and_detailed_lost_response_are_durable() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("lost-response", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    let mut evidence = ResetEvidence::default();
    let mut saw_before_write = false;
    let error = consume_recorded(&mut rpc, &job, &mut evidence, |report| {
        if report.response_received_at.is_none() && report.dispatch_error.is_none() {
            saw_before_write = true;
            assert!(fixture.consume_requests().is_empty());
            assert!(report.before.is_some());
        }
        store.record_evidence(&job.id, &serde_json::to_value(report)?)
    })
    .unwrap_err();
    assert!(saw_before_write);
    let error = format!("{error:#}");
    assert!(error.contains("account/rateLimitResetCredit/consume"));
    assert!(error.contains("phase=read/decode"));
    let reopened = Store::open(&fixture.root.path().join("state")).unwrap();
    let saved = reopened.show(&job.id).unwrap().evidence.unwrap();
    assert_eq!(saved["before"]["available_reset_credits"], 1);
    assert!(
        saved["dispatch_error"]
            .as_str()
            .unwrap()
            .contains("App-server exited")
    );
    assert!(saved["server_outcome"].is_null());
    assert!(reopened.claim(&job.id).unwrap().is_none());
    drop(rpc);
    assert_eq!(fixture.consume_requests().len(), 1);
}

#[test]
fn failure_to_persist_before_snapshot_prevents_any_reset_request() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    let mut evidence = ResetEvidence::default();
    assert!(
        consume_recorded(&mut rpc, &job, &mut evidence, |_| {
            anyhow::bail!("Synthetic evidence storage failure")
        })
        .is_err()
    );
    assert!(!rpc.reset_sent());
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
    assert!(store.claim(&job.id).unwrap().is_none());
}

#[test]
fn persistence_failure_after_response_keeps_server_outcome_in_error() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", true);
    let store = fixture.store();
    let job = claimed_job(&store, binding(&mut rpc));
    let mut evidence = ResetEvidence::default();
    let error = consume_recorded(&mut rpc, &job, &mut evidence, |report| {
        if report.response_received_at.is_some() {
            anyhow::bail!("Synthetic evidence storage failure");
        }
        store.record_evidence(&job.id, &serde_json::to_value(report)?)
    })
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("server outcome=reset"));
    assert!(message.contains("Synthetic evidence storage failure"));
    assert_eq!(evidence.server_outcome.as_deref(), Some("reset"));
    assert!(rpc.reset_sent());
    assert!(store.claim(&job.id).unwrap().is_none());
    drop(rpc);
    assert_eq!(fixture.consume_requests().len(), 1);
}

#[test]
fn pending_or_cancelled_job_never_starts_reset_preflight() {
    let fixture = FakeServer::new();
    let mut rpc = fixture.start("normal", true);
    let store = fixture.store();
    let job = store.create(now() + 60.0, binding(&mut rpc)).unwrap();
    let before = rpc.sent_methods.clone();
    assert!(consume_checked(&mut rpc, &job).is_err());
    assert_eq!(rpc.sent_methods, before);
    let cancelled = store.cancel(&job.id).unwrap();
    assert!(consume_checked(&mut rpc, &cancelled).is_err());
    assert_eq!(rpc.sent_methods, before);
    assert!(!rpc.reset_sent());
    drop(rpc);
    assert!(fixture.consume_requests().is_empty());
}

const FAKE_SERVER: &str = r#"
log=$1
mode=$2
while IFS= read -r line; do
    printf '%s\n' "$line" >> "$log"
    id=$(printf '%s\n' "$line" | /usr/bin/sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    case "$line" in
        *'"method":"initialize"'*)
            printf '{"id":%s,"result":{}}\n' "$id"
            ;;
        *'"method":"initialized"'*)
            ;;
        *'"method":"account/read"'*)
            printf '{"id":%s,"result":{"account":{"type":"chatgpt","email":"mock@example.invalid","planType":"pro"},"workspaceRouting":{"chatgptAccountId":"mock-account","backendOrigin":"https://mock.example.invalid"}}}\n' "$id"
            ;;
        *'"method":"account/rateLimits/read"'*)
            expiry=4102444800
            count=1
            if [ "$mode" = 'expired-credit' ]; then expiry=1; fi
            if [ "$mode" = 'no-credit' ]; then count=0; fi
            printf '{"id":%s,"result":{"accountId":"mock-account","ordinaryUsageAllowed":true,"rateLimitResetCredits":{"availableCount":%s,"credits":[{"id":"mock-credit","status":"available","resetType":"codexRateLimits","expiresAt":%s}]}}}\n' "$id" "$count" "$expiry"
            ;;
        *'"method":"account/rateLimitResetCredit/consume"'*)
            if [ "$mode" = 'lost-response' ]; then exit 0; fi
            printf '{"id":%s,"result":{"outcome":"reset"}}\n' "$id"
            ;;
        *)
            exit 2
            ;;
    esac
done
"#;
