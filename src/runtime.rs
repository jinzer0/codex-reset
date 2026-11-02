use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File},
    io::Read,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const SUPPORTED: &[&str] = &["0.165.0", "0.166.0"];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Runtime {
    pub path: PathBuf,
    pub sha256: String,
    pub version: String,
}

pub fn private_tempdir() -> Result<tempfile::TempDir> {
    Ok(tempfile::Builder::new()
        .prefix("codex-reset-")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()?)
}

pub fn home() -> Result<PathBuf> {
    let path = PathBuf::from(env::var_os("HOME").context("HOME is not set")?);
    ensure!(path.is_absolute(), "HOME must be absolute");
    Ok(path)
}

pub fn digest(path: &Path) -> Result<String> {
    let mut file = File::open(path).context("Cannot open executable")?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

impl Runtime {
    pub fn discover(explicit: Option<&Path>) -> Result<Self> {
        let candidate = if let Some(path) = explicit {
            path.to_owned()
        } else {
            env::split_paths(&env::var_os("PATH").unwrap_or_default())
                .map(|p| p.join("codex"))
                .find(|p| p.is_file())
                .or_else(|| {
                    home()
                        .ok()
                        .map(|p| p.join(".local/bin/codex"))
                        .filter(|p| p.is_file())
                })
                .context("Codex CLI not found; install it or use --codex-bin PATH")?
        };
        let mut path = candidate
            .canonicalize()
            .context("Cannot resolve Codex executable")?;
        if path.extension().is_some_and(|ext| ext == "js") {
            let os = match env::consts::OS {
                "macos" => "darwin",
                "linux" => "linux",
                _ => bail!("Unsupported OS"),
            };
            let arch = match env::consts::ARCH {
                "aarch64" => "arm64",
                "x86_64" => "x64",
                _ => bail!("Unsupported CPU"),
            };
            let cpu = if arch == "arm64" { "aarch64" } else { "x86_64" };
            let triple = format!(
                "{cpu}-{}",
                if os == "darwin" {
                    "apple-darwin"
                } else {
                    "unknown-linux-musl"
                }
            );
            let package = path
                .parent()
                .and_then(Path::parent)
                .context("Invalid npm launcher path")?;
            let native_package = format!("codex-{os}-{arch}");
            let candidates = [
                package
                    .join("node_modules/@openai")
                    .join(&native_package)
                    .join("vendor"),
                package
                    .parent()
                    .context("Invalid npm package")?
                    .join(&native_package)
                    .join("vendor"),
                package.join("vendor"),
            ];
            path = candidates
                .iter()
                .map(|p| p.join(&triple).join("bin/codex"))
                .find(|p| p.is_file())
                .context("Cannot find npm native Codex executable; use --codex-bin PATH")?
                .canonicalize()?;
        }
        let meta = fs::metadata(&path)?;
        ensure!(
            meta.is_file() && meta.permissions().mode() & 0o111 != 0,
            "Codex must be executable"
        );
        let mut magic = [0u8; 4];
        File::open(&path)?.read_exact(&mut magic)?;
        ensure!(
            [
                *b"\x7fELF",
                [0xcf, 0xfa, 0xed, 0xfe],
                [0xfe, 0xed, 0xfa, 0xcf],
                [0xca, 0xfe, 0xba, 0xbe]
            ]
            .contains(&magic),
            "Codex must be a native executable, not a wrapper"
        );
        let temp = private_tempdir()?;
        let output = File::create(temp.path().join("version"))?;
        let mut child = Command::new(&path)
            .arg("--version")
            .env_clear()
            .env("HOME", temp.path())
            .env("CODEX_HOME", temp.path())
            .env("PATH", "/usr/bin:/bin")
            .env("CODEX_REMOTE_CONTROL_DISABLED", "1")
            .stdin(Stdio::null())
            .stdout(output)
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .context("Cannot run Codex --version")?;
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = child.try_wait()? {
                ensure!(status.success(), "Codex --version failed");
                break;
            }
            if Instant::now() >= deadline {
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.wait();
                bail!("Codex --version timed out");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let output_path = temp.path().join("version");
        ensure!(
            fs::metadata(&output_path)?.len() < 1024,
            "Invalid Codex version output"
        );
        let text = fs::read_to_string(output_path)?;
        let version = text
            .trim()
            .strip_prefix("codex-cli ")
            .context("Unexpected Codex version output")?;
        ensure!(
            SUPPORTED.contains(&version),
            "Supported Codex versions: {}. No reset was sent.",
            SUPPORTED.join(", ")
        );
        Ok(Self {
            sha256: digest(&path)?,
            path,
            version: version.to_owned(),
        })
    }

    pub fn verify(&self) -> Result<()> {
        ensure!(
            SUPPORTED.contains(&self.version.as_str()),
            "Unsupported stored Codex version"
        );
        ensure!(
            digest(&self.path)? == self.sha256,
            "Codex changed since scheduling; cancel and schedule again"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn changed_binary_refused() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("codex");
        fs::write(&p, "fixture").unwrap();
        let r = Runtime {
            sha256: digest(&p).unwrap(),
            path: p.clone(),
            version: "0.165.0".into(),
        };
        r.verify().unwrap();
        fs::write(p, "changed").unwrap();
        assert!(r.verify().is_err());
    }
    #[test]
    fn unsupported_version_refused() {
        let r = Runtime {
            path: PathBuf::from("missing"),
            sha256: String::new(),
            version: "0.1.0".into(),
        };
        assert!(r.verify().unwrap_err().to_string().contains("Unsupported"));
    }
    #[test]
    fn wrapper_refused_before_execution() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("codex");
        fs::write(&p, "#!/bin/sh\nexit 42").unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(Runtime::discover(Some(&p)).is_err());
    }
}
