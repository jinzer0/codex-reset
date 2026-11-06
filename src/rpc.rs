use crate::runtime::{Runtime, home};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, sync_channel},
    },
    thread,
    time::{Duration, Instant},
};

pub const CONSUME: &str = "account/rateLimitResetCredit/consume";
const READ_METHODS: &[&str] = &[
    "initialize",
    "initialized",
    "account/read",
    "account/rateLimits/read",
];

fn sensitive_offset(text: &str) -> Option<usize> {
    let lower = text.to_ascii_lowercase();
    [
        "authorization",
        "bearer ",
        "access_token",
        "accesstoken",
        "refresh_token",
        "refreshtoken",
        "id_token",
        "idtoken",
        "api_key",
        "api-key",
        "apikey",
        "password",
        "secret",
        "cookie",
        "\"token\"",
        "token=",
        "token:",
    ]
    .iter()
    .filter_map(|key| lower.find(key))
    .min()
}

/// Bounded diagnostics only: never persist raw RPC parameters or credentials.
pub fn diagnostic_text(text: &str) -> String {
    let bounded: String = text.chars().take(8192).collect();
    let mut output = Vec::new();
    for line in bounded.lines() {
        let line: String = line
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let sensitive = sensitive_offset(&line);
        let visible = sensitive.map_or(line.as_str(), |index| &line[..index]);
        let mut words = Vec::new();
        for word in visible.split_whitespace() {
            let token = word.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-');
            let opaque = token.len() >= 40
                && token
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./+=".contains(c));
            words.push(
                if token.contains("sk-") || token.contains("eyJ") || opaque {
                    "[redacted]"
                } else {
                    word
                },
            );
        }
        let mut clean = words.join(" ");
        if sensitive.is_some() {
            clean.push_str("[redacted]");
        }
        output.push(clean);
        // Credential values may continue on later lines; omit the remaining text.
        if sensitive.is_some() {
            break;
        }
    }
    let mut output = output.join(" | ");
    if output.len() > 4096 {
        let mut end = 4064;
        while !output.is_char_boundary(end) {
            end -= 1;
        }
        output.truncate(end);
        output.push_str(" [truncated]");
    }
    output
}

pub trait Transport {
    fn request(&mut self, method: &str, params: Value) -> Result<Value>;
}

#[derive(Default)]
struct Gate {
    allow_reset: bool,
    reset_sent: bool,
}
impl Gate {
    fn authorize(&mut self, method: &str) -> Result<()> {
        ensure!(
            READ_METHODS.contains(&method) || (self.allow_reset && method == CONSUME),
            "RPC method is not permitted"
        );
        if method == CONSUME {
            ensure!(!self.reset_sent, "A reset was already attempted; no retry");
            self.reset_sent = true; // Before the write, including failed writes.
        }
        Ok(())
    }
}

pub struct Session {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: Receiver<std::result::Result<Value, &'static str>>,
    gate: Gate,
    sequence: u64,
    pub sent_methods: Vec<String>,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    stderr_done: Receiver<()>,
    _temporary: tempfile::TempDir,
}
impl Session {
    pub fn live(runtime: &Runtime, codex_home: &Path, allow_reset: bool) -> Result<Self> {
        runtime.verify()?;
        ensure!(codex_home.is_dir(), "Codex login directory does not exist");
        let temp = crate::runtime::private_tempdir()?;
        let mut command = Command::new(&runtime.path);
        command
            .args([
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                "chatgpt_base_url=\"https://chatgpt.com/backend-api/\"",
                "-c",
                "analytics.enabled=false",
            ])
            .env_clear()
            .env("HOME", home()?)
            .env("CODEX_HOME", codex_home)
            .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
            .env("LANG", "en_US.UTF-8")
            .env("TMPDIR", temp.path())
            .env("CODEX_REMOTE_CONTROL_DISABLED", "1")
            .current_dir(temp.path());
        Self::start(command, temp, allow_reset)
    }
    // This Rust API is also used by a sandboxed test harness; the public CLI has no endpoint override.
    pub fn start(mut command: Command, temp: tempfile::TempDir, allow_reset: bool) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
            .context("Cannot start app-server")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("Missing app-server stdout")?;
        let stderr = child.stderr.take().context("Missing app-server stderr")?;
        let stderr_tail = Arc::new(Mutex::new(VecDeque::new()));
        let tail = Arc::clone(&stderr_tail);
        let (stderr_tx, stderr_done) = sync_channel(1);
        thread::spawn(move || {
            let mut reader = BufReader::new(stderr);
            let mut omit_remaining = false;
            loop {
                let mut line = Vec::new();
                match reader.by_ref().take(4097).read_until(b'\n', &mut line) {
                    Ok(0) | Err(_) => break,
                    _ => (),
                }
                if omit_remaining {
                    continue;
                }
                let clean = if line.len() > 4096 {
                    // The omitted tail could contain a credential label.
                    omit_remaining = true;
                    if line.last() != Some(&b'\n') && reader.skip_until(b'\n').is_err() {
                        break;
                    }
                    "[oversized stderr line omitted]".to_owned()
                } else {
                    let text = String::from_utf8_lossy(&line);
                    omit_remaining = sensitive_offset(&text).is_some();
                    diagnostic_text(&text)
                };
                if let Ok(mut lines) = tail.lock() {
                    lines.push_back(clean);
                    while lines.len() > 8 || lines.iter().map(String::len).sum::<usize>() > 8192 {
                        lines.pop_front();
                    }
                }
            }
            let _ = stderr_tx.try_send(());
        });
        let (tx, rx) = sync_channel(64);
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut line = Vec::new();
                let read = reader.by_ref().take(1_048_577).read_until(b'\n', &mut line);
                let message = match read {
                    Ok(0) => Err("App-server exited"),
                    Ok(n) if n > 1_048_576 => Err("Oversized app-server output"),
                    Ok(_) => serde_json::from_slice(&line).map_err(|_| "Invalid app-server JSON"),
                    Err(_) => Err("Cannot read app-server output"),
                };
                let failed = message.is_err();
                if tx.try_send(message).is_err() || failed {
                    break;
                }
            }
        });
        let mut session = Self {
            child,
            stdin,
            messages: rx,
            gate: Gate {
                allow_reset,
                reset_sent: false,
            },
            sequence: 0,
            sent_methods: Vec::new(),
            stderr_tail,
            stderr_done,
            _temporary: temp,
        };
        session.request("initialize",json!({"clientInfo":{"name":"codex_reset_scheduler","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        session.send("initialized", json!({"method":"initialized"}))?;
        Ok(session)
    }
    pub fn reset_sent(&self) -> bool {
        self.gate.reset_sent
    }
    fn failure(&self, method: &str, id: u64, phase: &str, error: anyhow::Error) -> anyhow::Error {
        // Let an exiting process's stderr reader finish without waiting indefinitely
        // on a descendant that inherited the pipe.
        let _ = self.stderr_done.recv_timeout(Duration::from_millis(25));
        let tail = self
            .stderr_tail
            .lock()
            .map(|lines| lines.iter().cloned().collect::<Vec<_>>().join(" | "))
            .unwrap_or_default();
        let mut tail = diagnostic_text(&tail);
        if tail.len() > 1024 {
            let mut end = 1000;
            while !tail.is_char_boundary(end) {
                end -= 1;
            }
            tail.truncate(end);
            tail.push_str(" [truncated]");
        }
        let suffix = if tail.is_empty() {
            String::new()
        } else {
            format!("; app-server stderr: {tail}")
        };
        // Put the primary cause before optional stderr so later persistence
        // truncation cannot turn a useful server/transport error into a log tail.
        anyhow::anyhow!(
            "RPC {} id={id} phase={phase}: {}{suffix}",
            diagnostic_text(method),
            diagnostic_text(&format!("{error:#}"))
        )
    }
    fn send(&mut self, method: &str, payload: Value) -> Result<()> {
        self.gate.authorize(method)?;
        self.sent_methods.push(method.into());
        let stdin = self
            .stdin
            .as_mut()
            .context("App-server stdin unavailable")?;
        let mut bytes = serde_json::to_vec(&payload)?;
        bytes.push(b'\n');
        ensure!(bytes.len() < 4096, "RPC request exceeds size limit");
        stdin
            .write_all(&bytes)
            .context("App-server write failed; no retry")?;
        stdin.flush().context("App-server flush failed; no retry")?;
        Ok(())
    }
}
impl Transport for Session {
    fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.sequence += 1;
        let id = self.sequence;
        self.send(method, json!({"id":id,"method":method,"params":params}))
            .map_err(|error| self.failure(method, id, "write", error))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("App-server timed out; no retry")
                .map_err(|error| self.failure(method, id, "wait", error))?;
            let response = self
                .messages
                .recv_timeout(remaining)
                .context("App-server unavailable or timed out; no retry")
                .map_err(|error| self.failure(method, id, "wait", error))?
                .map_err(|error| self.failure(method, id, "read/decode", anyhow::anyhow!(error)))?;
            if response.get("method").is_some() {
                if response.get("id").is_some() {
                    return Err(self.failure(
                        method,
                        id,
                        "response",
                        anyhow::anyhow!("Interactive app-server requests are unsupported"),
                    ));
                }
                continue;
            }
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                return Err(self.failure(
                    method,
                    id,
                    "server",
                    anyhow::anyhow!(
                        "JSON-RPC error code {}: {}",
                        error.get("code").and_then(Value::as_i64).unwrap_or(-1),
                        diagnostic_text(
                            error
                                .get("message")
                                .and_then(Value::as_str)
                                .unwrap_or("No error message")
                        )
                    ),
                ));
            }
            return response
                .get("result")
                .cloned()
                .context("App-server response has no result")
                .map_err(|error| self.failure(method, id, "response", error));
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.stdin.take();
        // Keep the leader unreaped until group cleanup so its PID cannot be recycled.
        thread::sleep(Duration::from_millis(25));
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn fake_session(reply: &str) -> Session {
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(format!(
                r#"
while IFS= read -r line; do
    case "$line" in
        *'"method":"initialize"'*) printf '{{"id":1,"result":{{}}}}\n' ;;
        *'"method":"account/read"'*) {reply} ;;
    esac
done
"#
            ))
            .env_clear();
        Session::start(command, tempfile::tempdir().unwrap(), false).unwrap()
    }

    #[test]
    fn server_errors_preserve_method_id_code_and_safe_message() {
        let mut rpc = fake_session(
            r#"printf '{"id":2,"error":{"code":-32001,"message":"upstream unavailable; Authorization: Bearer hidden-access-value","data":{"access_token":"must-not-appear"}}}\n'"#,
        );
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = format!("{error:#}");
        assert!(
            text.contains("RPC account/read id=2 phase=server"),
            "{text}"
        );
        assert!(text.contains("-32001: upstream unavailable"), "{text}");
        assert!(!text.contains("hidden-access-value"));
        assert!(!text.contains("must-not-appear"));
        assert!(!rpc.reset_sent());
    }

    #[test]
    fn long_stderr_cannot_displace_primary_server_error_from_persisted_diagnostics() {
        let mut rpc = fake_session(
            r#"i=0; while [ "$i" -lt 8 ]; do j=0; while [ "$j" -lt 80 ]; do printf 'ordinary upstream diagnostic '; j=$((j + 1)); done; printf '\n'; i=$((i + 1)); done >&2; printf '{"id":2,"error":{"code":-32009,"message":"reset endpoint rejected request"}}\n'"#,
        );
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = diagnostic_text(&format!("{error:#}"));
        assert!(
            text.contains("RPC account/read id=2 phase=server"),
            "{text}"
        );
        assert!(
            text.contains("-32009: reset endpoint rejected request"),
            "{text}"
        );
        assert!(text.contains("app-server stderr:"), "{text}");
        assert!(
            text.len() < 1400,
            "stderr suffix was not bounded: {}",
            text.len()
        );
        assert!(text.find("-32009").unwrap() < text.find("app-server stderr:").unwrap());
    }

    #[test]
    fn process_exit_includes_sanitized_stderr_and_read_phase() {
        let mut rpc = fake_session(
            r#"printf 'upstream connection refused; access_token=hidden-value\n' >&2; exit 7"#,
        );
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("phase=read/decode"), "{text}");
        assert!(text.contains("App-server exited"), "{text}");
        assert!(text.contains("upstream connection refused"), "{text}");
        assert!(!text.contains("hidden-value"));
    }

    #[test]
    fn invalid_json_records_decode_phase_without_echoing_raw_payload() {
        let mut rpc = fake_session(r#"printf 'invalid secret-payload\n'"#);
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("phase=read/decode"), "{text}");
        assert!(text.contains("Invalid app-server JSON"), "{text}");
        assert!(!text.contains("secret-payload"));
    }

    #[test]
    fn diagnostics_redact_credentials_and_bound_unicode_output() {
        for sensitive in [
            "Authorization: Bearer hidden-value",
            "access_token=hidden-value",
            r#"{"accessToken":"v1-abcdefghijklmnopqrstuv"}"#,
            "refresh_token: hidden-value",
            r#"{"refreshToken":"v1-abcdefghijklmnopqrstuv"}"#,
            "id_token=hidden-value",
            r#"{"idToken":"v1-abcdefghijklmnopqrstuv"}"#,
            "api_key=hidden-value",
            "Cookie: hidden-value",
            "key=sk-hidden-value",
            "value=eyJhbGciOiJIUzI1NiJ9.payload.signature",
        ] {
            assert!(!diagnostic_text(sensitive).contains("hidden-value"));
            assert!(!diagnostic_text(sensitive).contains("v1-abcdefghijklmnopqrstuv"));
            assert!(diagnostic_text(sensitive).contains("[redacted]"));
        }
        assert_eq!(
            diagnostic_text("network\u{1b} failure\nretry disabled"),
            "network failure | retry disabled"
        );
        let text = diagnostic_text(&"한".repeat(10000));
        assert!(text.len() <= 4096);
        assert!(text.ends_with("[truncated]"));
    }

    #[test]
    fn diagnostics_omit_credential_continuations() {
        for label in ["Cookie:", "password:", "accessToken:"] {
            let text = diagnostic_text(&format!(
                "authentication failed; {label}\n  private-value\nmore-private-value"
            ));
            assert!(text.contains("authentication failed"));
            assert!(text.contains("[redacted]"));
            assert!(!text.contains("private-value"));
        }
    }

    #[test]
    fn stderr_omits_credentials_split_across_reads() {
        let mut rpc = fake_session(
            r#"printf 'authentication failed; Cookie:\n' >&2; sleep 0.05; printf '  session=private-session\npassword-value\n' >&2; exit 7"#,
        );
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("authentication failed"), "{text}");
        assert!(!text.contains("private-session"), "{text}");
        assert!(!text.contains("password-value"), "{text}");
    }

    #[test]
    fn stderr_is_drained_and_oversized_lines_are_omitted() {
        let mut rpc = fake_session(
            r#"i=0; while [ "$i" -lt 20000 ]; do printf '0123456789'; i=$((i + 1)); done >&2; printf '\nlast diagnostic\n' >&2; exit 7"#,
        );
        let error = rpc.request("account/read", json!({})).unwrap_err();
        let text = format!("{error:#}");
        assert!(text.contains("oversized stderr line omitted"), "{text}");
        assert!(!text.contains("last diagnostic"), "{text}");
        assert!(text.len() < 5000);
    }

    #[test]
    fn readonly_cannot_consume() {
        let mut g = Gate::default();
        assert!(g.authorize(CONSUME).is_err());
        assert!(!g.reset_sent);
    }
    #[test]
    fn no_retry_even_after_failed_send() {
        let mut g = Gate {
            allow_reset: true,
            reset_sent: false,
        };
        g.authorize(CONSUME).unwrap();
        assert!(g.authorize(CONSUME).is_err());
    }
    #[test]
    fn unrelated_mutations_refused() {
        let mut g = Gate {
            allow_reset: true,
            reset_sent: false,
        };
        assert!(g.authorize("account/login/start").is_err());
        g.authorize("account/read").unwrap();
    }
}
