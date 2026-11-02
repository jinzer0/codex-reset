//! Harmless OS-manager smoke test. This never starts Codex or uses an account.
use anyhow::{Context, Result, ensure};
use codex_reset::services;
use serde_json::json;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

struct JobCleanup {
    id: String,
    armed: bool,
}

impl Drop for JobCleanup {
    fn drop(&mut self) {
        if self.armed
            && let Err(error) = services::remove(&self.id)
        {
            eprintln!("Temporary job cleanup failed for {}: {error}", self.id);
        }
    }
}

fn main() -> Result<()> {
    let directory = tempfile::Builder::new()
        .prefix("codex-reset-service-smoke-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?;
    let root = directory.path().canonicalize()?;
    let marker = root.join("marker");
    let id = Uuid::new_v4().to_string();
    // Fixed shell program, marker path passed as an argument rather than code.
    // printf is a shell builtin; no Python, Codex, account or network is involved.
    let command = vec![
        "/bin/sh".to_owned(),
        "-c".to_owned(),
        "printf '%s' harmless-smoke > \"$1\"".to_owned(),
        "codex-reset-service-smoke".to_owned(),
        marker
            .to_str()
            .context("Temporary path is not UTF-8")?
            .to_owned(),
    ];
    let due = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64() + 17.0;
    let installed = services::install(&id, due, &command, &root)?;
    let mut cleanup = JobCleanup { id, armed: true };
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(200));
    }
    ensure!(
        fs::read_to_string(&marker).context("Harmless OS job did not produce its marker")?
            == "harmless-smoke",
        "Harmless OS job produced an unexpected marker"
    );
    services::remove(&cleanup.id)?;
    cleanup.armed = false;
    let state = services::inspect(&cleanup.id)?;
    ensure!(
        state["installed"] == false && state["loaded"] == false,
        "Temporary OS job was not fully removed"
    );
    println!(
        "{}",
        json!({
            "platform": std::env::consts::OS,
            "backend": installed["backend"],
            "marker_written": true,
            "job_removed": true,
            "codex_started": false,
            "reset_requests_sent": 0
        })
    );
    Ok(())
}
