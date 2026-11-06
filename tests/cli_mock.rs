//! Execute the public CLI against a tiny native stdio fixture. The fixture only
//! uses stdio and records requests in its temporary HOME. It has no network or
//! authentication-file code.
use codex_reset::{
    rpc::CONSUME,
    runtime::Runtime,
    store::{Binding, Store, now},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: tempfile::TempDir,
    binary: PathBuf,
    login: PathBuf,
    state: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::with_version("0.167.0")
    }

    fn with_version(version: &str) -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("fake-codex.c");
        let binary = root.path().join("codex");
        let login = root.path().join("synthetic-login");
        let state = root.path().join("state");
        fs::write(&source, NATIVE_FIXTURE).unwrap();
        fs::create_dir(&login).unwrap();
        let compile = Command::new("cc")
            .args(["-std=c99", "-Wall", "-Wextra", "-Werror"])
            .arg(format!("-DFIXTURE_VERSION=\"{version}\""))
            .arg("-o")
            .arg(&binary)
            .arg(&source)
            .output()
            .expect("A C compiler is required for the native CLI fixture");
        assert!(
            compile.status.success(),
            "Native fixture build failed: {}",
            String::from_utf8_lossy(&compile.stderr)
        );
        Self {
            root,
            binary,
            login,
            state,
        }
    }

    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codex-reset"));
        command
            .env_clear()
            .env("HOME", self.root.path())
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::null())
            .arg("--codex-bin")
            .arg(&self.binary)
            .arg("--codex-home")
            .arg(&self.login)
            .arg("--state-dir")
            .arg(&self.state)
            .arg("--json");
        command
    }

    fn output(&self, arguments: &[&str]) -> Output {
        self.command().args(arguments).output().unwrap()
    }

    fn json(&self, arguments: &[&str]) -> Value {
        let output = self.output(arguments);
        assert!(
            output.status.success(),
            "CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }

    fn binding(&self) -> Binding {
        let runtime = Runtime::discover(Some(&self.binary)).unwrap();
        let identity =
            serde_json::to_vec(&json!(["mock-account", "mock@example.invalid"])).unwrap();
        Binding {
            account_fingerprint: format!("{:x}", Sha256::digest(identity)),
            codex_home: self.login.canonicalize().unwrap(),
            binary_path: runtime.path,
            binary_sha256: runtime.sha256,
            binary_version: runtime.version,
        }
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.root.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_read_only_requests(&self) {
        let requests = self.requests();
        assert!(!requests.is_empty());
        for request in requests {
            assert!(
                [
                    "initialize",
                    "initialized",
                    "account/read",
                    "account/rateLimits/read"
                ]
                .contains(&request["method"].as_str().unwrap()),
                "unexpected request: {request}"
            );
        }
    }

    fn assert_no_reservation(&self) {
        assert!(
            !self.state.exists(),
            "Read-only command created application state"
        );
        assert!(!self.root.path().join("Library/LaunchAgents").exists());
        assert!(!self.root.path().join(".config/systemd/user").exists());
    }
}

fn future_time() -> String {
    (chrono::Utc::now() + chrono::Duration::minutes(3)).to_rfc3339()
}

#[test]
fn public_check_uses_native_fixture_readonly_and_creates_no_state() {
    for version in ["0.165.0", "0.166.0", "0.167.0"] {
        let fixture = Fixture::with_version(version);
        let result = fixture.json(&["check"]);
        assert_eq!(result["codex_version"], version);
        assert_eq!(result["account"]["email"], "m***@example.invalid");
        assert_eq!(result["account"]["available_reset_credits"], 1);
        assert_eq!(
            result["account"]["rate_limits"]["primary"]["usedPercent"],
            80
        );
        assert_eq!(result["reset_requests_sent"], 0);
        fixture.assert_read_only_requests();
        fixture.assert_no_reservation();
    }
}

#[test]
fn public_check_rejects_unapproved_versions_before_starting_app_server() {
    for version in ["0.164.0", "0.167.1", "0.168.0", "0.167.0-alpha.1"] {
        let fixture = Fixture::with_version(version);
        let output = fixture.output(&["check"]);
        assert_eq!(output.status.code(), Some(2), "version {version}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Supported Codex versions:"),
            "version {version}"
        );
        assert!(!fixture.root.path().join("requests.jsonl").exists());
        fixture.assert_no_reservation();
    }
}

#[test]
fn public_schedule_dry_run_never_creates_a_job_or_service() {
    let fixture = Fixture::new();
    let result = fixture.json(&[
        "schedule",
        "--at",
        &future_time(),
        "--timezone",
        "UTC",
        "--dry-run",
    ]);
    assert_eq!(result["dry_run"], true);
    assert_eq!(result["reset_requests_sent"], 0);
    assert_eq!(result["account"]["available_reset_credits"], 1);
    assert!(result["at"].as_str().is_some());
    fixture.assert_read_only_requests();
    fixture.assert_no_reservation();
}

#[test]
fn public_schedule_without_noninteractive_authorization_creates_nothing() {
    let fixture = Fixture::new();
    let output = fixture.output(&["schedule", "--at", &future_time(), "--timezone", "UTC"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--yes"));
    fixture.assert_no_reservation();
}

#[test]
fn public_worker_gate_rejects_execution_before_any_state_is_opened() {
    let fixture = Fixture::new();
    let output = fixture.output(&["_run", "--id", "00000000"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--execute-reset"));
    fixture.assert_no_reservation();
}

#[test]
fn public_worker_completes_one_fake_reset_and_second_invocation_does_not_dispatch() {
    for version in ["0.165.0", "0.166.0", "0.167.0"] {
        let fixture = Fixture::with_version(version);
        let binding = fixture.binding();
        let store = Store::open(&fixture.state).unwrap();
        let job = store.create(now() + 0.2, binding).unwrap();
        let first = fixture.json(&["_run", "--id", &job.id, "--execute-reset"]);
        assert_eq!(first["reset_requests_sent"], 1);
        assert_eq!(first["job"]["status"], "completed");
        assert_eq!(first["job"]["outcome"], "reset");
        let finished = store.show(&job.id).unwrap();
        let second = fixture.json(&["_run", "--id", &job.id, "--execute-reset"]);
        assert_eq!(second["reset_requests_sent"], 0);
        assert_eq!(second["job"]["status"], "completed");
        assert_eq!(store.show(&job.id).unwrap(), finished);
        let consume_requests: Vec<_> = fixture
            .requests()
            .into_iter()
            .filter(|request| request["method"] == CONSUME)
            .collect();
        assert_eq!(consume_requests.len(), 1);
        assert_eq!(
            consume_requests[0]["params"],
            json!({"idempotencyKey":job.idempotency_key,"creditId":"mock-credit"})
        );
        assert!(!fixture.root.path().join("Library/LaunchAgents").exists());
        assert!(!fixture.root.path().join(".config/systemd/user").exists());
    }
}

#[test]
fn public_worker_account_mismatch_fails_before_claim_or_reset() {
    let fixture = Fixture::new();
    let mut binding = fixture.binding();
    binding.account_fingerprint = "different-synthetic-account".into();
    let store = Store::open(&fixture.state).unwrap();
    let job = store.create(now() + 0.2, binding).unwrap();
    let output = fixture.output(&["_run", "--id", &job.id, "--execute-reset"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("account differs"));
    let failed = store.show(&job.id).unwrap();
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error.as_deref(), Some("preflight_failed"));
    assert_eq!(failed.claimed_at, None);
    assert_eq!(failed.outcome, None);
    let second = fixture.json(&["_run", "--id", &job.id, "--execute-reset"]);
    assert_eq!(second["reset_requests_sent"], 0);
    assert_eq!(store.show(&job.id).unwrap(), failed);
    fixture.assert_read_only_requests();
}

const NATIVE_FIXTURE: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        puts("codex-cli " FIXTURE_VERSION);
        return 0;
    }
    if (argc < 2 || strcmp(argv[1], "app-server") != 0) return 90;
    const char *home = getenv("HOME");
    if (home == NULL) return 92;
    char trace_path[4096];
    int length = snprintf(trace_path, sizeof trace_path, "%s/requests.jsonl", home);
    if (length < 0 || (size_t)length >= sizeof trace_path) return 92;
    FILE *trace = fopen(trace_path, "a");
    if (trace == NULL) return 92;
    setvbuf(stdout, NULL, _IONBF, 0);
    char line[8192];
    while (fgets(line, sizeof(line), stdin)) {
        if (fputs(line, trace) == EOF || fflush(trace) == EOF) return 92;
        char *id_text = strstr(line, "\"id\":");
        unsigned long id = id_text ? strtoul(id_text + 5, NULL, 10) : 0;
        if (strstr(line, "\"method\":\"initialize\"")) {
            printf("{\"id\":%lu,\"result\":{}}\n", id);
        } else if (strstr(line, "\"method\":\"initialized\"")) {
            continue;
        } else if (strstr(line, "\"method\":\"account/read\"")) {
            printf("{\"id\":%lu,\"result\":{"
                   "\"account\":{\"type\":\"chatgpt\",\"email\":\"mock@example.invalid\","
                   "\"planType\":\"pro\",\"workspaceName\":null},\"requiresOpenaiAuth\":true,"
                   "\"workspaceRouting\":{\"chatgptAccountId\":\"mock-account\","
                   "\"backendOrigin\":\"https://mock.example.invalid\","
                   "\"accountRoutingOverride\":\"NO_CONSTRAINT\"}}}\n", id);
        } else if (strstr(line, "\"method\":\"account/rateLimits/read\"")) {
            printf("{\"id\":%lu,\"result\":{\"accountId\":\"mock-account\","
                   "\"rateLimits\":{\"limitId\":\"codex\",\"primary\":{\"usedPercent\":80,"
                   "\"windowDurationMins\":300,\"resetsAt\":4102444800},\"secondary\":null},"
                   "\"rateLimitResetCredits\":{\"availableCount\":1,\"credits\":["
                   "{\"id\":\"mock-credit\",\"status\":\"available\",\"resetType\":\"codexRateLimits\","
                   "\"grantedAt\":0,\"expiresAt\":4102444800}]}}}\n", id);
        } else if (strstr(line, "\"method\":\"account/rateLimitResetCredit/consume\"")) {
            printf("{\"id\":%lu,\"result\":{\"outcome\":\"reset\"}}\n", id);
        } else {
            return 91;
        }
    }
    fclose(trace);
    return 0;
}
"#;
