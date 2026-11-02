//! Execute the public CLI against a tiny native stdio fixture. The fixture only
//! reads stdin and writes stdout; it has no network or authentication-file code.
use codex_reset::{
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
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("fake-codex.c");
        let binary = root.path().join("codex");
        let login = root.path().join("synthetic-login");
        let state = root.path().join("state");
        fs::write(&source, NATIVE_FIXTURE).unwrap();
        fs::create_dir(&login).unwrap();
        let compile = Command::new("cc")
            .args(["-std=c99", "-Wall", "-Wextra", "-Werror", "-o"])
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
    let fixture = Fixture::new();
    let result = fixture.json(&["check"]);
    assert_eq!(result["codex_version"], "0.165.0");
    assert_eq!(result["account"]["email"], "m***@example.invalid");
    assert_eq!(result["account"]["available_reset_credits"], 1);
    assert_eq!(result["reset_requests_sent"], 0);
    fixture.assert_no_reservation();
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
    let fixture = Fixture::new();
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
    assert!(!fixture.root.path().join("Library/LaunchAgents").exists());
    assert!(!fixture.root.path().join(".config/systemd/user").exists());
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
}

const NATIVE_FIXTURE: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        puts("codex-cli 0.165.0");
        return 0;
    }
    if (argc < 2 || strcmp(argv[1], "app-server") != 0) return 90;
    setvbuf(stdout, NULL, _IONBF, 0);
    char line[8192];
    while (fgets(line, sizeof(line), stdin)) {
        char *id_text = strstr(line, "\"id\":");
        unsigned long id = id_text ? strtoul(id_text + 5, NULL, 10) : 0;
        if (strstr(line, "\"method\":\"initialize\"")) {
            printf("{\"id\":%lu,\"result\":{}}\n", id);
        } else if (strstr(line, "\"method\":\"initialized\"")) {
            continue;
        } else if (strstr(line, "\"method\":\"account/read\"")) {
            printf("{\"id\":%lu,\"result\":{\"account\":{\"type\":\"chatgpt\",\"email\":\"mock@example.invalid\",\"planType\":\"pro\"},\"workspaceRouting\":{\"chatgptAccountId\":\"mock-account\",\"backendOrigin\":\"https://mock.example.invalid\"}}}\n", id);
        } else if (strstr(line, "\"method\":\"account/rateLimits/read\"")) {
            printf("{\"id\":%lu,\"result\":{\"accountId\":\"mock-account\",\"rateLimitResetCredits\":{\"availableCount\":1,\"credits\":[{\"id\":\"mock-credit\",\"status\":\"available\",\"resetType\":\"codexRateLimits\",\"expiresAt\":4102444800}]}}}\n", id);
        } else if (strstr(line, "\"method\":\"account/rateLimitResetCredit/consume\"")) {
            printf("{\"id\":%lu,\"result\":{\"outcome\":\"reset\"}}\n", id);
        } else {
            return 91;
        }
    }
    return 0;
}
"#;
