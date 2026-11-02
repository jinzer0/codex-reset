//! Per-user one-shot OS jobs. Callers must persist cancellation before removal.
use anyhow::{Context, Result, bail, ensure};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Backend {
    Systemd,
    Launchd,
}

struct Identity {
    backend: Backend,
    unit: String,
    paths: Vec<PathBuf>,
}

#[derive(Default)]
struct Response {
    success: bool,
    stdout: String,
}

type Runner<'a> = dyn FnMut(&[String]) -> Result<Response> + 'a;

fn current_identity(job_id: &str) -> Result<Identity> {
    let home = std::env::var_os("HOME").context("HOME is unavailable")?;
    let backend = match std::env::consts::OS {
        "linux" => Backend::Systemd,
        "macos" => Backend::Launchd,
        _ => bail!("Background scheduling requires Linux systemd or macOS launchd"),
    };
    identity(job_id, Path::new(&home), backend)
}

fn identity(job_id: &str, home: &Path, backend: Backend) -> Result<Identity> {
    let id = Uuid::parse_str(job_id).context("Job ID must be a canonical UUID")?;
    ensure!(id.to_string() == job_id, "Job ID must be a canonical UUID");
    ensure!(home.is_absolute(), "HOME must be an absolute path");
    let (unit, directory, suffixes) = match backend {
        Backend::Systemd => (
            format!("codex-reset-{job_id}"),
            home.join(".config/systemd/user"),
            vec!["service", "timer"],
        ),
        Backend::Launchd => (
            format!("io.github.codex-reset.{job_id}"),
            home.join("Library/LaunchAgents"),
            vec!["plist"],
        ),
    };
    let paths = suffixes
        .into_iter()
        .map(|suffix| directory.join(format!("{unit}.{suffix}")))
        .collect();
    Ok(Identity {
        backend,
        unit,
        paths,
    })
}

fn command_output(args: &[String]) -> Result<Response> {
    ensure!(!args.is_empty(), "Empty manager command");
    // Files avoid pipe deadlocks and bound the output read into memory.
    let mut stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    let mut child = Command::new(&args[0])
        .args(&args[1..])
        .stdin(Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .context("OS job manager is unavailable")?;
    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            // Own process group only; no unrelated jobs are affected.
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.kill();
            let _ = child.wait();
            bail!("OS job manager did not respond within 15 seconds");
        }
        thread::sleep(Duration::from_millis(20));
    };
    stdout.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    stdout.take(1_048_576).read_to_end(&mut bytes)?;
    Ok(Response {
        success: status.success(),
        stdout: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

fn invoke(run: &mut Runner<'_>, args: &[&str], required: bool) -> Result<Response> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    match run(&args) {
        Ok(response) if response.success || !required => Ok(response),
        Ok(_) => bail!("OS job manager command failed"),
        Err(_) if !required => Ok(Response::default()),
        Err(error) => Err(error),
    }
}

fn validate_text(value: &str) -> Result<()> {
    ensure!(
        !value.chars().any(|c| c.is_control()),
        "Control characters are not allowed in service arguments or paths"
    );
    Ok(())
}

fn quote(value: &str) -> Result<String> {
    validate_text(value)?;
    Ok(format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('%', "%%")
            .replace('$', "$$")
    ))
}

fn xml(value: &str) -> Result<String> {
    validate_text(value)?;
    Ok(value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;"))
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str().context("Service paths must be valid UTF-8")
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn directory(path: &Path, private: bool) -> Result<()> {
    DirBuilder::new().recursive(true).mode(0o700).create(path)?;
    let info = fs::symlink_metadata(path)?;
    ensure!(
        info.is_dir() && !info.file_type().is_symlink(),
        "Service directory cannot be a symlink"
    );
    ensure!(
        info.uid() == unsafe { libc::getuid() },
        "Service directory must belong to this user"
    );
    let forbidden = if private { 0o077 } else { 0o022 };
    ensure!(
        info.mode() & forbidden == 0,
        "Service directory {} has unsafe permissions (mode {:03o}; {})",
        path.display(),
        info.mode() & 0o777,
        if private {
            "private state/log directories require mode 0700"
        } else {
            "definition directories must not be group/other writable"
        }
    );
    Ok(())
}

fn write_exclusive(path: &Path, content: &[u8], created: &mut Vec<PathBuf>) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    created.push(path.to_owned());
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}

fn log_paths(
    state_dir: &Path,
    job_id: &str,
    created: &mut Vec<PathBuf>,
) -> Result<(PathBuf, PathBuf)> {
    ensure!(
        state_dir.is_absolute(),
        "State directory must be an absolute path"
    );
    directory(state_dir, true)?;
    let logs = state_dir.canonicalize()?.join("logs");
    directory(&logs, true)?;
    let stdout = logs.join(format!("{job_id}.out.log"));
    let stderr = logs.join(format!("{job_id}.err.log"));
    for path in [&stdout, &stderr] {
        validate_text(path_text(path)?)?;
        write_exclusive(path, b"", created)?;
    }
    Ok((stdout, stderr))
}

fn render(
    identity: &Identity,
    due_at: f64,
    now: f64,
    command: &[String],
    stdout: &Path,
    stderr: &Path,
) -> Result<Vec<String>> {
    ensure!(
        due_at.is_finite() && now.is_finite(),
        "Schedule times must be finite"
    );
    ensure!(
        !command.is_empty() && Path::new(&command[0]).is_absolute(),
        "Service executable must be an absolute path"
    );
    for argument in command {
        validate_text(argument)?;
    }
    let out = path_text(stdout)?;
    let err = path_text(stderr)?;
    validate_text(out)?;
    validate_text(err)?;
    match identity.backend {
        Backend::Systemd => {
            let fire_at = (now + 1.0).max(due_at - 15.0).ceil();
            ensure!(
                fire_at >= i64::MIN as f64 && fire_at < i64::MAX as f64,
                "Schedule time is out of range"
            );
            let calendar = DateTime::<Utc>::from_timestamp(fire_at as i64, 0)
                .context("Schedule time is out of range")?
                .format("%Y-%m-%d %H:%M:%S UTC");
            let command = command
                .iter()
                .map(|s| quote(s))
                .collect::<Result<Vec<_>>>()?
                .join(" ");
            let service = format!(
                "[Unit]\nDescription=Codex reset scheduled worker\n\n[Service]\nType=oneshot\nRestart=no\nUMask=0077\nExecStart={command}\nStandardOutput=append:{}\nStandardError=append:{}\n",
                out.replace('%', "%%"),
                err.replace('%', "%%")
            );
            let timer = format!(
                "[Unit]\nDescription=Codex reset one-time timer\n\n[Timer]\nOnCalendar={calendar}\nAccuracySec=1s\nRandomizedDelaySec=0\nPersistent=false\nUnit={}.service\n\n[Install]\nWantedBy=timers.target\n",
                identity.unit
            );
            Ok(vec![service, timer])
        }
        Backend::Launchd => {
            let arguments = command
                .iter()
                .map(|s| Ok(format!("<string>{}</string>", xml(s)?)))
                .collect::<Result<Vec<_>>>()?
                .join("");
            Ok(vec![format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict><key>Label</key><string>{}</string><key>ProgramArguments</key><array>{arguments}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><false/><key>Umask</key><integer>63</integer><key>StandardOutPath</key><string>{}</string><key>StandardErrorPath</key><string>{}</string></dict></plist>\n",
                xml(&identity.unit)?,
                xml(out)?,
                xml(err)?
            )])
        }
    }
}

fn inspect_with(identity: &Identity, run: &mut Runner<'_>) -> Result<Value> {
    let installed = identity.paths.iter().all(|path| {
        fs::symlink_metadata(path)
            .map(|m| m.is_file() && !m.file_type().is_symlink())
            .unwrap_or(false)
    });
    let backend = if identity.backend == Backend::Systemd {
        "systemd"
    } else {
        "launchd"
    };
    let mut result = json!({"backend":backend,"unit":identity.unit,"paths":identity.paths,"installed":installed,"loaded":false,"active":false,"warnings":[]});
    if identity.backend == Backend::Systemd {
        let mut loaded = false;
        let mut active = false;
        let mut available = false;
        for suffix in ["timer", "service"] {
            let unit = format!("{}.{suffix}", identity.unit);
            let response = invoke(
                run,
                &[
                    "/usr/bin/systemctl",
                    "--user",
                    "show",
                    &unit,
                    "--property=LoadState",
                    "--property=ActiveState",
                ],
                false,
            )?;
            available |= response.success;
            let mut load = "unknown";
            let mut state = "unknown";
            for line in response.stdout.lines() {
                if let Some(value) = line.strip_prefix("LoadState=") {
                    load = value;
                }
                if let Some(value) = line.strip_prefix("ActiveState=") {
                    state = value;
                }
            }
            let is_loaded = load == "loaded";
            loaded |= is_loaded;
            active |= matches!(state, "active" | "activating");
            result[format!("{suffix}_loaded")] = json!(is_loaded);
            result[format!("{suffix}_state")] = json!(state);
        }
        result["loaded"] = json!(loaded);
        result["active"] = json!(active);
        result["manager_available"] = json!(available);
        result["state"] = result["timer_state"].clone();
    } else {
        let target = format!("gui/{}/{}", unsafe { libc::getuid() }, identity.unit);
        let response = invoke(run, &["/bin/launchctl", "print", &target], false)?;
        let active = response.success && response.stdout.contains("state = running");
        result["loaded"] = json!(response.success);
        result["active"] = json!(active);
        result["state"] = json!(if active {
            "running"
        } else if response.success {
            "loaded"
        } else {
            "absent"
        });
    }
    Ok(result)
}

/// Read files and manager state without registering or repairing a job.
pub fn inspect(job_id: &str) -> Result<Value> {
    inspect_with(&current_identity(job_id)?, &mut command_output)
}

fn ready(run: &mut Runner<'_>) -> Result<Vec<String>> {
    let response = invoke(
        run,
        &["/usr/bin/systemctl", "--user", "is-system-running"],
        false,
    )?;
    ensure!(
        matches!(response.stdout.trim(), "running" | "degraded" | "starting"),
        "The systemd user manager is unavailable; no job was installed"
    );
    let uid = unsafe { libc::getuid() }.to_string();
    let linger = invoke(
        run,
        &[
            "/usr/bin/loginctl",
            "show-user",
            &uid,
            "--property=Linger",
            "--value",
        ],
        false,
    )?;
    Ok(if linger.success && linger.stdout.trim() == "yes" {
        vec![]
    } else {
        vec!["User lingering is not confirmed; the job may stop when you log out. No lingering settings were changed.".to_owned()]
    })
}

fn install_with(
    identity: &Identity,
    job_id: &str,
    due_at: f64,
    command: &[String],
    state_dir: &Path,
    run: &mut Runner<'_>,
) -> Result<Value> {
    // Validate all strings before creating files or invoking a mutating command.
    ensure!(due_at.is_finite(), "Schedule time must be finite");
    ensure!(
        !command.is_empty() && Path::new(&command[0]).is_absolute(),
        "Service executable must be an absolute path"
    );
    for value in command {
        validate_text(value)?;
    }
    ensure!(
        !identity.paths.iter().any(|p| exists(p)) && inspect_with(identity, run)?["loaded"] != true,
        "A background job with this UUID already exists"
    );
    let warnings = if identity.backend == Backend::Systemd {
        ready(run)?
    } else {
        vec![]
    };
    let mut created = Vec::new();
    let mut attempted_registration = false;
    let result = (|| -> Result<()> {
        directory(
            identity.paths[0]
                .parent()
                .context("Missing service directory")?,
            false,
        )?;
        let (stdout, stderr) = log_paths(state_dir, job_id, &mut created)?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs_f64();
        let definitions = render(identity, due_at, now, command, &stdout, &stderr)?;
        for (path, content) in identity.paths.iter().zip(definitions) {
            write_exclusive(path, content.as_bytes(), &mut created)?;
        }
        if identity.backend == Backend::Systemd {
            invoke(
                run,
                &["/usr/bin/systemctl", "--user", "daemon-reload"],
                true,
            )?;
            attempted_registration = true;
            let timer = format!("{}.timer", identity.unit);
            invoke(
                run,
                &["/usr/bin/systemctl", "--user", "enable", "--now", &timer],
                true,
            )?;
        } else {
            let target = format!("gui/{}", unsafe { libc::getuid() });
            attempted_registration = true;
            invoke(
                run,
                &[
                    "/bin/launchctl",
                    "bootstrap",
                    &target,
                    path_text(&identity.paths[0])?,
                ],
                true,
            )?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        if attempted_registration {
            if identity.backend == Backend::Systemd {
                let timer = format!("{}.timer", identity.unit);
                let service = format!("{}.service", identity.unit);
                let _ = invoke(
                    run,
                    &["/usr/bin/systemctl", "--user", "disable", "--now", &timer],
                    false,
                );
                let _ = invoke(
                    run,
                    &["/usr/bin/systemctl", "--user", "stop", &service],
                    false,
                );
            } else {
                let target = format!("gui/{}/{}", unsafe { libc::getuid() }, identity.unit);
                let _ = invoke(run, &["/bin/launchctl", "bootout", &target], false);
            }
        }
        for path in created.iter().rev() {
            let _ = fs::remove_file(path);
        }
        if identity.backend == Backend::Systemd
            && identity.paths.iter().any(|p| created.contains(p))
        {
            let _ = invoke(
                run,
                &["/usr/bin/systemctl", "--user", "daemon-reload"],
                false,
            );
        }
        return Err(error.context("Background registration failed"));
    }
    let mut state = inspect_with(identity, run)?;
    state["warnings"] = json!(warnings);
    Ok(state)
}

/// Register a one-shot OS worker. The worker owns deadline and claim enforcement.
pub fn install(job_id: &str, due_at: f64, command: &[String], state_dir: &Path) -> Result<Value> {
    install_with(
        &current_identity(job_id)?,
        job_id,
        due_at,
        command,
        state_dir,
        &mut command_output,
    )
}

fn remove_with(identity: &Identity, run: &mut Runner<'_>) -> Result<()> {
    for path in &identity.paths {
        ensure!(
            !fs::symlink_metadata(path)
                .map(|m| m.file_type().is_symlink())
                .unwrap_or(false),
            "Refusing to remove a symlinked job definition"
        );
    }
    let state = inspect_with(identity, run)?;
    if !identity.paths.iter().any(|p| exists(p)) && state["loaded"] != true {
        return Ok(());
    }
    if identity.backend == Backend::Systemd {
        if exists(&identity.paths[1]) || state["timer_loaded"] == true {
            let timer = format!("{}.timer", identity.unit);
            invoke(
                run,
                &["/usr/bin/systemctl", "--user", "disable", "--now", &timer],
                true,
            )?;
        }
        if exists(&identity.paths[0]) || state["service_loaded"] == true {
            let service = format!("{}.service", identity.unit);
            invoke(
                run,
                &["/usr/bin/systemctl", "--user", "stop", &service],
                true,
            )?;
        }
    } else if state["loaded"] == true {
        let target = format!("gui/{}/{}", unsafe { libc::getuid() }, identity.unit);
        invoke(run, &["/bin/launchctl", "bootout", &target], true)?;
    }
    for path in &identity.paths {
        if exists(path) {
            fs::remove_file(path)?;
        }
    }
    if identity.backend == Backend::Systemd {
        invoke(
            run,
            &["/usr/bin/systemctl", "--user", "daemon-reload"],
            true,
        )?;
    }
    Ok(())
}

/// Unregister this job only. Caller must reject removal after a persisted claim.
pub fn remove(job_id: &str) -> Result<()> {
    remove_with(&current_identity(job_id)?, &mut command_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};

    const ID: &str = "7d6c5383-2dc9-4be3-8d9d-11d9b648c38c";

    fn fixture(backend: Backend) -> (tempfile::TempDir, Identity, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let identity = identity(ID, temp.path(), backend).unwrap();
        let state = temp.path().join("state");
        (temp, identity, state)
    }

    fn absent(args: &[String]) -> Result<Response> {
        let output = if args.iter().any(|s| s == "is-system-running") {
            "degraded"
        } else if args.iter().any(|s| s == "show-user") {
            "yes"
        } else {
            "LoadState=not-found\nActiveState=inactive\n"
        };
        Ok(Response {
            success: !args.iter().any(|s| s == "print"),
            stdout: output.to_owned(),
        })
    }

    #[test]
    fn uuid_validation_rejects_paths_and_noncanonical_forms() {
        for id in ["../x", "", "7D6C5383-2DC9-4BE3-8D9D-11D9B648C38C"] {
            assert!(identity(id, Path::new("/tmp"), Backend::Systemd).is_err());
        }
    }

    #[test]
    fn systemd_quotes_literal_arguments_and_exact_seconds() {
        let (_temp, identity, _) = fixture(Backend::Systemd);
        let definitions = render(
            &identity,
            120.1,
            1.0,
            &["/a path/reset".into(), "x%y$HOME\\\"".into()],
            Path::new("/logs/a b%out"),
            Path::new("/logs/err"),
        )
        .unwrap();
        assert!(definitions[0].contains("ExecStart=\"/a path/reset\" \"x%%y$$HOME\\\\\\\"\""));
        assert!(definitions[0].contains("StandardOutput=append:/logs/a b%%out"));
        assert!(definitions[0].contains("Restart=no"));
        assert!(definitions[1].contains("OnCalendar=1970-01-01 00:01:46 UTC"));
        assert!(definitions[1].contains("AccuracySec=1s"));
        assert!(definitions[1].contains("Persistent=false"));
    }

    #[test]
    fn launchd_xml_has_literal_arguments_and_no_restart() {
        let (_temp, identity, _) = fixture(Backend::Launchd);
        let text = render(
            &identity,
            10.0,
            1.0,
            &["/a/reset".into(), "a<&\"'b".into()],
            Path::new("/out"),
            Path::new("/err"),
        )
        .unwrap()
        .remove(0);
        assert!(text.contains("<string>a&lt;&amp;&quot;&apos;b</string>"));
        assert!(text.contains("<key>RunAtLoad</key><true/>"));
        assert!(text.contains("<key>KeepAlive</key><false/>"));
        assert!(!text.contains("StartCalendarInterval"));
    }

    #[test]
    fn invalid_input_never_creates_files_or_runs_manager() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        let mut calls = 0;
        let mut run = |_args: &[String]| {
            calls += 1;
            Ok(Response::default())
        };
        assert!(
            install_with(
                &identity,
                ID,
                f64::NAN,
                &["/bin/true".into()],
                &state,
                &mut run
            )
            .is_err()
        );
        assert!(install_with(&identity, ID, 10.0, &["relative".into()], &state, &mut run).is_err());
        assert!(
            install_with(
                &identity,
                ID,
                10.0,
                &["/bin/true".into(), "bad\nargument".into()],
                &state,
                &mut run
            )
            .is_err()
        );
        assert_eq!(calls, 0);
        assert!(!state.exists());
    }

    #[test]
    fn systemd_install_creates_private_files_without_executing_commands() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        let mut calls = vec![];
        let mut run = |args: &[String]| {
            calls.push(args.to_vec());
            absent(args)
        };
        let result = install_with(
            &identity,
            ID,
            1_900_000_000.0,
            &["/bin/true".into()],
            &state,
            &mut run,
        )
        .unwrap();
        assert_eq!(result["backend"], "systemd");
        assert_eq!(result["installed"], true);
        assert!(result["warnings"].as_array().unwrap().is_empty());
        for path in &identity.paths {
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(calls.iter().any(|args| args.iter().any(|s| s == "enable")));
        remove_with(&identity, &mut absent).unwrap();
        assert!(identity.paths.iter().all(|p| !exists(p)));
        assert!(state.join("logs").join(format!("{ID}.out.log")).is_file());
    }

    #[test]
    fn launchd_install_and_remove_are_mocked() {
        let (_temp, identity, state) = fixture(Backend::Launchd);
        let mut loaded = false;
        let mut run = |args: &[String]| {
            match args[1].as_str() {
                "bootstrap" => loaded = true,
                "bootout" => loaded = false,
                _ => (),
            }
            Ok(Response {
                success: args[1] != "print" || loaded,
                stdout: String::new(),
            })
        };
        let result = install_with(
            &identity,
            ID,
            1_900_000_000.0,
            &["/bin/true".into()],
            &state,
            &mut run,
        )
        .unwrap();
        assert_eq!(result["loaded"], true);
        remove_with(&identity, &mut run).unwrap();
        assert!(!identity.paths[0].exists());
    }

    #[test]
    fn failed_registration_rolls_back_only_own_files() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        directory(identity.paths[0].parent().unwrap(), false).unwrap();
        let unrelated = identity.paths[0]
            .parent()
            .unwrap()
            .join("unrelated.service");
        fs::write(&unrelated, "preserve").unwrap();
        let mut calls = vec![];
        let mut run = |args: &[String]| {
            calls.push(args.to_vec());
            if args.iter().any(|s| s == "enable") {
                bail!("simulated failure")
            }
            absent(args)
        };
        assert!(
            install_with(
                &identity,
                ID,
                1_900_000_000.0,
                &["/bin/true".into()],
                &state,
                &mut run
            )
            .is_err()
        );
        assert!(identity.paths.iter().all(|p| !exists(p)));
        assert_eq!(fs::read_to_string(unrelated).unwrap(), "preserve");
        assert!(fs::read_dir(state.join("logs")).unwrap().next().is_none());
        assert!(calls.iter().any(|args| args.iter().any(|s| s == "disable")));
    }

    #[test]
    fn existing_definition_is_not_overwritten() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        directory(identity.paths[0].parent().unwrap(), false).unwrap();
        fs::write(&identity.paths[0], "original").unwrap();
        let mut run = |_args: &[String]| -> Result<Response> { panic!("manager must not run") };
        assert!(
            install_with(&identity, ID, 10.0, &["/bin/true".into()], &state, &mut run).is_err()
        );
        assert_eq!(fs::read_to_string(&identity.paths[0]).unwrap(), "original");
    }

    #[test]
    fn symlinked_definition_and_logs_are_rejected() {
        let (temp, identity, state) = fixture(Backend::Launchd);
        directory(identity.paths[0].parent().unwrap(), false).unwrap();
        let target = temp.path().join("target");
        fs::write(&target, "preserve").unwrap();
        symlink(&target, &identity.paths[0]).unwrap();
        assert!(remove_with(&identity, &mut absent).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "preserve");
        fs::remove_file(&identity.paths[0]).unwrap();
        directory(&state, true).unwrap();
        symlink(temp.path(), state.join("logs")).unwrap();
        assert!(
            install_with(
                &identity,
                ID,
                10.0,
                &["/bin/true".into()],
                &state,
                &mut absent
            )
            .is_err()
        );
        assert!(!identity.paths[0].exists());
    }

    #[test]
    fn absent_lingering_warns_without_changing_policy() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        let mut calls = vec![];
        let mut run = |args: &[String]| {
            calls.push(args.to_vec());
            if args.iter().any(|s| s == "show-user") {
                return Ok(Response {
                    success: true,
                    stdout: "no".into(),
                });
            }
            absent(args)
        };
        let result = install_with(
            &identity,
            ID,
            1_900_000_000.0,
            &["/bin/true".into()],
            &state,
            &mut run,
        )
        .unwrap();
        assert_eq!(result["warnings"].as_array().unwrap().len(), 1);
        assert!(!calls.iter().flatten().any(|s| s == "enable-linger"));
    }

    #[test]
    fn normal_definition_directory_is_accepted_without_chmod() {
        let temp = tempfile::tempdir().unwrap();
        let definitions = temp.path().join("LaunchAgents");
        fs::create_dir(&definitions).unwrap();
        fs::set_permissions(&definitions, fs::Permissions::from_mode(0o755)).unwrap();
        directory(&definitions, false).unwrap();
        assert_eq!(fs::metadata(&definitions).unwrap().mode() & 0o777, 0o755);
        assert!(directory(&definitions, true).is_err());
        fs::set_permissions(&definitions, fs::Permissions::from_mode(0o775)).unwrap();
        assert!(directory(&definitions, false).is_err());
        assert_eq!(fs::metadata(&definitions).unwrap().mode() & 0o777, 0o775);
    }

    #[test]
    fn readable_state_directory_is_rejected_without_chmod() {
        let (_temp, identity, state) = fixture(Backend::Launchd);
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o755)).unwrap();
        let error = install_with(
            &identity,
            ID,
            1_900_000_000.0,
            &["/bin/true".into()],
            &state,
            &mut absent,
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("private state/log directories require mode 0700"));
        assert_eq!(fs::metadata(&state).unwrap().mode() & 0o777, 0o755);
        assert!(!identity.paths[0].exists());
    }

    #[test]
    fn missing_user_manager_fails_before_writes() {
        let (_temp, identity, state) = fixture(Backend::Systemd);
        let mut run = |_args: &[String]| Ok(Response::default());
        assert!(
            install_with(&identity, ID, 10.0, &["/bin/true".into()], &state, &mut run).is_err()
        );
        assert!(!state.exists());
    }

    #[test]
    fn inspection_is_read_only_and_reports_running_service() {
        let (_temp, identity, _) = fixture(Backend::Systemd);
        let mut run = |args: &[String]| {
            assert!(args.iter().any(|s| s == "show"));
            Ok(Response {
                success: true,
                stdout: if args.iter().any(|s| s.ends_with(".service")) {
                    "LoadState=loaded\nActiveState=active\n"
                } else {
                    "LoadState=loaded\nActiveState=inactive\n"
                }
                .into(),
            })
        };
        let result = inspect_with(&identity, &mut run).unwrap();
        assert_eq!(result["loaded"], true);
        assert_eq!(result["active"], true);
        assert_eq!(result["service_state"], "active");
    }
}
