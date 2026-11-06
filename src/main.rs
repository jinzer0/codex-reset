use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use codex_reset::{
    account::{ResetEvidence, choose_credit, consume_recorded, read_snapshot},
    rpc::{Session, diagnostic_text},
    runtime::{Runtime, home},
    services,
    store::{Binding, Store, local_zone, now, parse_time},
};
use serde_json::{Value, json};
use std::{
    env,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    thread,
    time::Duration,
};
#[derive(Parser)]
#[command(version, about = "Schedule one Codex usage reset on macOS or Ubuntu")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,
    #[arg(long, global = true, hide = true)]
    state_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    codex_bin: Option<PathBuf>,
    #[arg(long, global = true)]
    codex_home: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Action>,
}
#[derive(Subcommand)]
enum Action {
    /// Read account and available credits; never redeem
    Check,
    /// Preview and confirm a one-time background reset
    Schedule {
        #[arg(long)]
        at: String,
        #[arg(long)]
        timezone: Option<String>,
        /// Preview only; do not create a reservation or OS job
        #[arg(long)]
        dry_run: bool,
        /// Authorize one real reset without interactive confirmation
        #[arg(long)]
        yes: bool,
    },
    /// Show local reservations
    Status,
    /// Cancel a pending reservation, or remove a finished job's OS definition
    Cancel { id: String },
    #[command(name = "_run", hide = true)]
    Run {
        #[arg(long)]
        id: String,
        #[arg(long)]
        execute_reset: bool,
    },
}
fn state_dir(cli: &Cli) -> Result<PathBuf> {
    if let Some(path) = &cli.state_dir {
        return absolute(path);
    }
    #[cfg(target_os = "macos")]
    {
        Ok(home()?.join("Library/Application Support/codex-reset"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Ok(env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or(home()?.join(".local/state"))
            .join("codex-reset"))
    }
}
fn absolute(path: &Path) -> Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_owned()
    } else {
        env::current_dir()?.join(path)
    })
}
fn login_home(cli: &Cli) -> Result<PathBuf> {
    let path = cli
        .codex_home
        .clone()
        .or_else(|| env::var_os("CODEX_HOME").map(PathBuf::from))
        .unwrap_or(home()?.join(".codex"));
    ensure!(
        path.is_dir(),
        "Codex home does not exist; sign in through Codex first"
    );
    Ok(path.canonicalize()?)
}
fn prompt(label: &str) -> Result<String> {
    print!("{label}");
    io::stdout().flush()?;
    let mut value = String::new();
    ensure!(
        io::stdin().read_line(&mut value)? > 0,
        "Input closed; no action confirmed"
    );
    Ok(value.trim().to_owned())
}
fn wizard() -> Result<Option<Action>> {
    println!(
        "Codex Reset\n  1. Check account and credits\n  2. Schedule one reset\n  3. Show reservations\n  4. Cancel a reservation\n  q. Quit"
    );
    match prompt("Choose [1]: ")?.as_str() {
        "" | "1" => Ok(Some(Action::Check)),
        "2" => {
            let at = prompt("When (e.g. tomorrow 04:00): ")?;
            let timezone = prompt(&format!("Timezone [{}]: ", local_zone()))?;
            Ok(Some(Action::Schedule {
                at,
                timezone: if timezone.is_empty() {
                    None
                } else {
                    Some(timezone)
                },
                dry_run: false,
                yes: false,
            }))
        }
        "3" => Ok(Some(Action::Status)),
        "4" => Ok(Some(Action::Cancel {
            id: prompt("Job ID: ")?,
        })),
        "q" | "Q" => Ok(None),
        _ => bail!("Unknown menu choice"),
    }
}
fn emit(value: &Value, as_json: bool) -> Result<()> {
    if as_json {
        println!("{}", serde_json::to_string_pretty(value)?);
        return Ok(());
    }
    if let Some(a) = value.get("account") {
        println!(
            "Account: {} ({}) | reset credits: {}",
            a["email"].as_str().unwrap_or("unknown"),
            a["plan"].as_str().unwrap_or("unknown"),
            a["available_reset_credits"]
        );
    }
    if let Some(at) = value.get("at") {
        println!(
            "When: {} [{}]",
            at.as_str().unwrap_or("unknown"),
            value["timezone"].as_str().unwrap_or("UTC")
        );
    }
    if value["dry_run"] == true {
        println!("Preview only. No reservation or background job was created.");
    }
    if value["cancelled"] == true {
        println!("No reservation created.");
    }
    let print_job = |job: &Value| {
        let date = job["due_at"]
            .as_f64()
            .and_then(|v| chrono::DateTime::from_timestamp(v as i64, 0))
            .map(|d| d.to_rfc3339())
            .unwrap_or_else(|| "unknown".into());
        println!(
            "{}  {}  {}{}{}",
            job["id"].as_str().unwrap_or("?"),
            job["status"].as_str().unwrap_or("?"),
            date,
            job["outcome"]
                .as_str()
                .map(|s| format!("  {s}"))
                .unwrap_or_default(),
            job["error"]
                .as_str()
                .map(|s| format!("  ({s})"))
                .unwrap_or_default()
        );
        if let Some(status) = job
            .pointer("/evidence/verification/status")
            .and_then(Value::as_str)
        {
            println!("  Usage verification: {status}");
        }
    };
    if let Some(j) = value.get("job") {
        print_job(j);
    }
    if let Some(jobs) = value["jobs"].as_array() {
        if jobs.is_empty() {
            println!("No reservations.");
        }
        for job in jobs {
            print_job(job);
        }
    }
    if let Some(warnings) = value.pointer("/service/warnings").and_then(Value::as_array) {
        for w in warnings {
            println!("Note: {}", w.as_str().unwrap_or(""));
        }
    }
    Ok(())
}
fn worker(store: &Store, id: &str, execute_reset: bool) -> Result<Value> {
    ensure!(execute_reset, "Worker requires --execute-reset");
    let result = (|| -> Result<Value> {
        loop {
            let job = store.show(id)?;
            if job.status != "pending" {
                return Ok(json!({"job":job,"reset_requests_sent":0}));
            }
            let remaining = job.due_at - now();
            if remaining <= 15. {
                break;
            }
            thread::sleep(Duration::from_secs_f64((remaining - 15.).min(0.5)));
        }
        let job = store.show(id)?;
        if now() > job.due_at + job.grace_seconds {
            let _ = store.claim(id)?;
            return Ok(json!({"job":store.show(id)?,"reset_requests_sent":0}));
        }
        let b = &job.binding;
        let runtime = Runtime {
            path: b.binary_path.clone(),
            sha256: b.binary_sha256.clone(),
            version: b.binary_version.clone(),
        };
        runtime.verify().context("preflight: verify Codex binary")?;
        let mut rpc =
            Session::live(&runtime, &b.codex_home, true).context("preflight: start app-server")?;
        read_snapshot(&mut rpc, Some(&b.account_fingerprint))
            .context("preflight: verify signed-in account")?;
        loop {
            if let Some(claimed) = store.claim(id)? {
                let mut evidence = ResetEvidence::default();
                let result = consume_recorded(&mut rpc, &claimed, &mut evidence, |evidence| {
                    store.record_evidence(id, &serde_json::to_value(evidence)?)
                });
                let attempted = rpc.reset_sent();
                let job = match &result {
                    Ok(outcome) => store.finish(id, Some(outcome), None)?,
                    Err(error) => store.finish(
                        id,
                        None,
                        Some(&diagnostic_text(&format!("{}: {error:#}", evidence.phase))),
                    )?,
                };
                // Persist the dispatch result first. Every subsequent session is read-only,
                // including recovery after a lost consume response or app-server exit.
                drop(rpc);
                if attempted {
                    for delay in [0, 250, 750] {
                        thread::sleep(Duration::from_millis(delay));
                        let observation = Session::live(&runtime, &b.codex_home, false)
                            .context("Start read-only verification session")
                            .and_then(|mut reader| {
                                read_snapshot(&mut reader, Some(&b.account_fingerprint))
                            });
                        evidence.observe(observation);
                        store.record_evidence(id, &serde_json::to_value(&evidence)?)?;
                        if evidence.verification.status == "confirmed"
                            || evidence
                                .server_outcome
                                .as_deref()
                                .is_some_and(|value| value != "reset")
                        {
                            break;
                        }
                    }
                }
                return Ok(
                    json!({"job":store.show(&job.id)?,"reset_requests_sent":u8::from(attempted)}),
                );
            }
            let current = store.show(id)?;
            if current.status != "pending" {
                return Ok(json!({"job":current,"reset_requests_sent":u8::from(rpc.reset_sent())}));
            }
            thread::sleep(Duration::from_millis(50));
        }
    })();
    if let Err(error) = &result {
        store.fail_preflight(id, &diagnostic_text(&format!("preflight: {error:#}")))?;
    }
    result
}
fn execute(cli: &Cli, action: &Action) -> Result<Value> {
    match action {
        Action::Check => {
            let runtime = Runtime::discover(cli.codex_bin.as_deref())?;
            let mut rpc = Session::live(&runtime, &login_home(cli)?, false)?;
            Ok(
                json!({"account":read_snapshot(&mut rpc,None)?.report(),"codex_version":runtime.version,"reset_requests_sent":u8::from(rpc.reset_sent())}),
            )
        }
        Action::Schedule {
            at,
            timezone,
            dry_run,
            yes,
        } => {
            let zone = timezone.clone().unwrap_or_else(local_zone);
            let due = parse_time(at, &zone, now())?;
            ensure!(
                due >= now() + 60.,
                "Choose a time at least 60 seconds in the future"
            );
            let runtime = Runtime::discover(cli.codex_bin.as_deref())?;
            let login = login_home(cli)?;
            let snapshot = {
                let mut rpc = Session::live(&runtime, &login, false)?;
                read_snapshot(&mut rpc, None)?
            };
            ensure!(
                choose_credit(&snapshot, due)?.is_some(),
                "No reset credit available at scheduled time"
            );
            let stamp = chrono::DateTime::from_timestamp(due as i64, 0)
                .context("Time out of range")?
                .with_timezone(&zone.parse::<chrono_tz::Tz>()?)
                .to_rfc3339();
            let preview = json!({"account":snapshot.report(),"at":stamp,"timezone":zone,"dry_run":dry_run,"reset_requests_sent":0});
            if *dry_run {
                return Ok(preview);
            }
            if !*yes {
                ensure!(
                    io::stdin().is_terminal() && !cli.json,
                    "Use --dry-run for preview or --yes to authorize one real reset"
                );
                emit(&preview, false)?;
                if !["y", "yes"].contains(
                    &prompt("Authorize one real reset at this time? [y/N]: ")?
                        .to_lowercase()
                        .as_str(),
                ) {
                    return Ok(json!({"cancelled":true}));
                }
            }
            ensure!(
                due >= now() + 60.,
                "The time is now too close; choose a later time"
            );
            let directory = state_dir(cli)?;
            let store = Store::open(&directory)?;
            let job = store.create(
                due,
                Binding {
                    account_fingerprint: snapshot.fingerprint,
                    codex_home: login,
                    binary_path: runtime.path,
                    binary_sha256: runtime.sha256,
                    binary_version: runtime.version,
                },
            )?;
            let setup = (|| -> Result<Value> {
                let exe = env::current_exe()?.canonicalize()?;
                let command = vec![
                    exe.to_string_lossy().into_owned(),
                    "_run".into(),
                    "--id".into(),
                    job.id.clone(),
                    "--execute-reset".into(),
                    "--state-dir".into(),
                    directory.to_string_lossy().into_owned(),
                    "--json".into(),
                ];
                services::install(&job.id, due, &command, &directory)
            })();
            match setup {
                Ok(service) => Ok(json!({"job":job,"service":service,"at":stamp,"timezone":zone})),
                Err(error) => {
                    store.cancel(&job.id)?;
                    Err(error)
                }
            }
        }
        Action::Status => {
            let directory = state_dir(cli)?;
            if !directory.join("rust-jobs.sqlite3").exists() {
                return Ok(json!({"jobs":[]}));
            }
            Ok(json!({"jobs":Store::open(&directory)?.list()?}))
        }
        Action::Cancel { id } => {
            let store = Store::open(&state_dir(cli)?)?;
            let id = store.resolve_id(id)?;
            let job = store.cancel(&id)?;
            ensure!(
                job.status != "claimed",
                "Job already claimed; stopping cannot retract a sent request"
            );
            services::remove(&id)?;
            Ok(json!({"job":job}))
        }
        Action::Run { id, execute_reset } => {
            ensure!(*execute_reset, "Worker requires --execute-reset");
            let store = Store::open(&state_dir(cli)?)?;
            let id = store.resolve_id(id)?;
            worker(&store, &id, *execute_reset)
        }
    }
}
fn run() -> Result<()> {
    let mut cli = Cli::parse();
    if cli.command.is_none() {
        if !io::stdin().is_terminal() {
            use clap::CommandFactory;
            Cli::command().print_help()?;
            println!();
            return Ok(());
        }
        cli.command = wizard()?;
    }
    if let Some(action) = &cli.command {
        emit(&execute(&cli, action)?, cli.json)?;
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("Error: {}", diagnostic_text(&format!("{error:#}")));
        std::process::exit(2);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_worker_without_explicit_gate() {
        let dir = tempfile::tempdir().unwrap();
        let cli = Cli::try_parse_from([
            "codex-reset",
            "_run",
            "--id",
            "not-a-job",
            "--state-dir",
            dir.path().to_str().unwrap(),
        ])
        .unwrap();
        assert!(
            execute(&cli, cli.command.as_ref().unwrap())
                .unwrap_err()
                .to_string()
                .contains("--execute-reset")
        );
        assert!(!dir.path().join("rust-jobs.sqlite3").exists());
    }
    #[test]
    fn empty_status_creates_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state");
        let cli =
            Cli::try_parse_from(["codex-reset", "status", "--state-dir", p.to_str().unwrap()])
                .unwrap();
        assert_eq!(
            execute(&cli, cli.command.as_ref().unwrap()).unwrap()["jobs"],
            json!([])
        );
        assert!(!p.exists());
    }
    #[test]
    fn dryrun_and_authorization_are_distinct() {
        let cli =
            Cli::try_parse_from(["codex-reset", "schedule", "--at", "내일 04:00", "--dry-run"])
                .unwrap();
        assert!(matches!(
            cli.command,
            Some(Action::Schedule {
                dry_run: true,
                yes: false,
                ..
            })
        ));
    }
    #[test]
    fn old_clock_flag_is_not_silently_accepted() {
        assert!(Cli::try_parse_from(["codex-reset", "--clock", "0400"]).is_err());
    }
}
