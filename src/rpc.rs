use crate::runtime::{Runtime, home};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::process::CommandExt,
    path::Path,
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{Receiver, sync_channel},
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
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .context("Cannot start app-server")?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().context("Missing app-server stdout")?;
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
            _temporary: temp,
        };
        session.request("initialize",json!({"clientInfo":{"name":"codex_reset_scheduler","version":env!("CARGO_PKG_VERSION")},"capabilities":{"experimentalApi":true}}))?;
        session.send("initialized", json!({"method":"initialized"}))?;
        Ok(session)
    }
    pub fn reset_sent(&self) -> bool {
        self.gate.reset_sent
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
        self.send(method, json!({"id":id,"method":method,"params":params}))?;
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .context("App-server timed out; no retry")?;
            let response = self
                .messages
                .recv_timeout(remaining)
                .context("App-server unavailable or timed out; no retry")?
                .map_err(|e| anyhow::anyhow!(e))?;
            if response.get("method").is_some() {
                ensure!(
                    response.get("id").is_none(),
                    "Interactive app-server requests are unsupported"
                );
                continue;
            }
            if response.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = response.get("error") {
                bail!(
                    "JSON-RPC error code {}",
                    error.get("code").and_then(Value::as_i64).unwrap_or(-1)
                );
            }
            return response
                .get("result")
                .cloned()
                .context("App-server response has no result");
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
