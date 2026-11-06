//! Execute the public CLI against a tiny native stdio fixture. The fixture only
//! uses only stdio and synthetic files below its temporary HOME; it has no
//! network, authentication-file, or OS job code.
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

    fn scenario(&self, scenario: &str) {
        fs::write(self.root.path().join("scenario"), scenario).unwrap();
    }

    fn worker(&self) -> Value {
        let store = Store::open(&self.state).unwrap();
        let binding = self.binding();
        let job = store.create(now() + 0.2, binding).unwrap();
        self.json(&["_run", "--id", &job.id, "--execute-reset"])
    }

    fn requests(&self) -> Vec<Value> {
        fs::read_to_string(self.root.path().join("requests.jsonl"))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn assert_one_consume_then_readonly(&self) {
        let requests = self.requests();
        let consumes: Vec<_> = requests
            .iter()
            .enumerate()
            .filter(|(_, request)| request["method"] == "account/rateLimitResetCredit/consume")
            .collect();
        assert_eq!(consumes.len(), 1);
        let following = &requests[consumes[0].0 + 1..];
        assert!(
            following
                .iter()
                .any(|request| request["method"] == "initialize")
        );
        assert!(
            following
                .iter()
                .any(|request| request["method"] == "account/rateLimits/read")
        );
        assert!(following.iter().all(|request| {
            [
                "initialize",
                "initialized",
                "account/read",
                "account/rateLimits/read",
            ]
            .contains(&request["method"].as_str().unwrap())
        }));
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
    let evidence = &first["job"]["evidence"];
    assert_eq!(evidence["server_outcome"], "reset");
    assert_eq!(evidence["before"]["available_reset_credits"], 1);
    assert_eq!(
        evidence["before"]["rate_limits"]["primary"]["usedPercent"],
        80
    );
    assert_eq!(evidence["verification"]["status"], "confirmed");
    assert_eq!(evidence["verification"]["usage_reset_observed"], true);
    assert_eq!(evidence["verification"]["credit_count_decreased"], true);
    assert!(
        evidence["request_started_at"].as_f64().unwrap()
            <= evidence["response_received_at"].as_f64().unwrap()
    );
    assert_eq!(
        evidence["verification"]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let status = fixture.json(&["status"]);
    assert_eq!(status["jobs"][0]["evidence"], *evidence);
    let finished = store.show(&job.id).unwrap();
    let second = fixture.json(&["_run", "--id", &job.id, "--execute-reset"]);
    assert_eq!(second["reset_requests_sent"], 0);
    assert_eq!(second["job"]["status"], "completed");
    assert_eq!(store.show(&job.id).unwrap(), finished);
    assert!(!fixture.root.path().join("Library/LaunchAgents").exists());
    assert!(!fixture.root.path().join(".config/systemd/user").exists());
    fixture.assert_one_consume_then_readonly();
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
    assert!(failed.error.as_deref().unwrap().contains("preflight"));
    assert!(failed.error.as_deref().unwrap().contains("account differs"));
    assert_eq!(failed.claimed_at, None);
    assert_eq!(failed.outcome, None);
    let second = fixture.json(&["_run", "--id", &job.id, "--execute-reset"]);
    assert_eq!(second["reset_requests_sent"], 0);
    assert_eq!(store.show(&job.id).unwrap(), failed);
}

#[test]
fn server_reset_response_does_not_confirm_unchanged_usage() {
    let fixture = Fixture::new();
    fixture.scenario("unchanged");
    let result = fixture.worker();
    assert_eq!(result["job"]["outcome"], "reset");
    let evidence = &result["job"]["evidence"];
    assert_eq!(evidence["server_outcome"], "reset");
    assert_eq!(evidence["verification"]["status"], "not_observed");
    assert_eq!(evidence["verification"]["usage_reset_observed"], false);
    assert_eq!(
        evidence["verification"]["attempts"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    fixture.assert_one_consume_then_readonly();
}

#[test]
fn readback_failure_preserves_successful_server_response_without_retry() {
    let fixture = Fixture::new();
    fixture.scenario("read-error");
    let result = fixture.worker();
    assert_eq!(result["job"]["status"], "completed");
    assert_eq!(result["job"]["outcome"], "reset");
    let evidence = &result["job"]["evidence"];
    assert_eq!(evidence["server_outcome"], "reset");
    assert_eq!(evidence["verification"]["status"], "read_failed");
    let attempts = evidence["verification"]["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 3);
    assert!(attempts.iter().all(|attempt| {
        attempt["error"]
            .as_str()
            .unwrap()
            .contains("mock usage lookup failed")
            && attempt["snapshot"].is_null()
    }));
    fixture.assert_one_consume_then_readonly();
}

#[test]
fn lost_response_keeps_unknown_outcome_and_records_state_change_readonly() {
    let fixture = Fixture::new();
    fixture.scenario("lost-response");
    let result = fixture.worker();
    assert_eq!(result["job"]["status"], "unknown");
    assert!(result["job"]["outcome"].is_null());
    let evidence = &result["job"]["evidence"];
    assert!(evidence["server_outcome"].is_null());
    assert!(
        evidence["dispatch_error"]
            .as_str()
            .unwrap()
            .contains("account/rateLimitResetCredit/consume")
    );
    assert_eq!(evidence["verification"]["status"], "inconclusive");
    assert_eq!(evidence["verification"]["usage_reset_observed"], true);
    assert_eq!(evidence["verification"]["credit_count_decreased"], true);
    fixture.assert_one_consume_then_readonly();
}

#[test]
fn server_error_details_survive_in_job_record() {
    let fixture = Fixture::new();
    fixture.scenario("rpc-error");
    let result = fixture.worker();
    assert_eq!(result["job"]["status"], "unknown");
    let error = result["job"]["error"].as_str().unwrap();
    assert!(error.contains("account/rateLimitResetCredit/consume"));
    assert!(error.contains("-32001"));
    assert!(error.contains("mock reset rejected"));
    assert!(result["job"]["evidence"]["server_outcome"].is_null());
    fixture.assert_one_consume_then_readonly();
}

#[test]
fn delayed_state_reflection_retries_only_reads() {
    let fixture = Fixture::new();
    fixture.scenario("delayed");
    let result = fixture.worker();
    let verification = &result["job"]["evidence"]["verification"];
    assert_eq!(verification["status"], "confirmed");
    let attempts = verification["attempts"].as_array().unwrap();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0]["snapshot"]["available_reset_credits"], 1);
    assert_eq!(attempts[1]["snapshot"]["available_reset_credits"], 0);
    fixture.assert_one_consume_then_readonly();
}

#[test]
fn account_switch_during_readback_cannot_confirm_reset() {
    let fixture = Fixture::new();
    fixture.scenario("account-mismatch");
    let result = fixture.worker();
    assert_eq!(result["job"]["outcome"], "reset");
    let verification = &result["job"]["evidence"]["verification"];
    assert_eq!(verification["status"], "read_failed");
    assert!(
        verification["attempts"]
            .as_array()
            .unwrap()
            .iter()
            .all(|attempt| {
                attempt["error"]
                    .as_str()
                    .unwrap()
                    .contains("account differs")
                    && attempt["snapshot"].is_null()
            })
    );
    let requests = fixture.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|request| request["method"] == "account/rateLimitResetCredit/consume")
            .count(),
        1
    );
}

const NATIVE_FIXTURE: &str = r#"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static FILE *synthetic_file(const char *name, const char *mode) {
    char path[4096];
    const char *home = getenv("HOME");
    if (!home || snprintf(path, sizeof(path), "%s/%s", home, name) >= (int)sizeof(path)) exit(92);
    return fopen(path, mode);
}

static int read_number(const char *name) {
    FILE *file = synthetic_file(name, "r");
    int value = 0;
    if (file) {
        if (fscanf(file, "%d", &value) != 1) value = 0;
        fclose(file);
    }
    return value;
}

static void write_number(const char *name, int value) {
    FILE *file = synthetic_file(name, "w");
    if (!file) exit(93);
    fprintf(file, "%d", value);
    fclose(file);
}

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--version") == 0) {
        puts("codex-cli 0.165.0");
        return 0;
    }
    if (argc < 2 || strcmp(argv[1], "app-server") != 0) return 90;
    setvbuf(stdout, NULL, _IONBF, 0);
    char scenario[64] = "normal";
    FILE *configuration = synthetic_file("scenario", "r");
    if (configuration) {
        if (fscanf(configuration, "%63s", scenario) != 1) return 94;
        fclose(configuration);
    }
    char line[8192];
    while (fgets(line, sizeof(line), stdin)) {
        FILE *requests = synthetic_file("requests.jsonl", "a");
        if (!requests) return 95;
        fputs(line, requests);
        fclose(requests);
        char *id_text = strstr(line, "\"id\":");
        unsigned long id = id_text ? strtoul(id_text + 5, NULL, 10) : 0;
        int redeemed = read_number("redeemed");
        if (strstr(line, "\"method\":\"initialize\"")) {
            printf("{\"id\":%lu,\"result\":{}}\n", id);
        } else if (strstr(line, "\"method\":\"initialized\"")) {
            continue;
        } else if (strstr(line, "\"method\":\"account/read\"")) {
            const char *account = redeemed && strcmp(scenario, "account-mismatch") == 0 ? "another-account" : "mock-account";
            printf("{\"id\":%lu,\"result\":{\"account\":{\"type\":\"chatgpt\",\"email\":\"mock@example.invalid\",\"planType\":\"pro\"},\"workspaceRouting\":{\"chatgptAccountId\":\"%s\",\"backendOrigin\":\"https://mock.example.invalid\"}}}\n", id, account);
        } else if (strstr(line, "\"method\":\"account/rateLimits/read\"")) {
            if (redeemed && strcmp(scenario, "read-error") == 0) {
                printf("{\"id\":%lu,\"error\":{\"code\":-32004,\"message\":\"mock usage lookup failed\"}}\n", id);
                continue;
            }
            int reflected = redeemed && strcmp(scenario, "unchanged") != 0;
            if (redeemed && strcmp(scenario, "delayed") == 0) {
                int reads = read_number("post-reads") + 1;
                write_number("post-reads", reads);
                reflected = reads > 1;
            }
            printf("{\"id\":%lu,\"result\":{\"accountId\":\"mock-account\",\"ordinaryUsageAllowed\":true,\"rateLimits\":{\"limitId\":\"codex\",\"primary\":{\"usedPercent\":%d,\"resetsAt\":4102444800,\"windowDurationMins\":300},\"secondary\":{\"usedPercent\":%d,\"resetsAt\":4102444800,\"windowDurationMins\":10080}},\"rateLimitResetCredits\":{\"availableCount\":%d,\"credits\":[{\"id\":\"mock-credit\",\"status\":\"%s\",\"resetType\":\"codexRateLimits\",\"expiresAt\":4102444800}]}}}\n", id, reflected ? 0 : 80, reflected ? 0 : 75, reflected ? 0 : 1, reflected ? "redeemed" : "available");
        } else if (strstr(line, "\"method\":\"account/rateLimitResetCredit/consume\"")) {
            if (strcmp(scenario, "rpc-error") == 0) {
                printf("{\"id\":%lu,\"error\":{\"code\":-32001,\"message\":\"mock reset rejected\"}}\n", id);
                continue;
            }
            write_number("redeemed", 1);
            if (strcmp(scenario, "lost-response") == 0) return 0;
            printf("{\"id\":%lu,\"result\":{\"outcome\":\"reset\"}}\n", id);
        } else {
            return 91;
        }
    }
    return 0;
}
"#;
