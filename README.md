# Codex Reset

A Rust command-line tool for scheduling **one Codex usage-reset attempt** on macOS or Ubuntu. Run `codex-reset` for an interactive menu. Python, pipx, Cargo and Rust are not needed when using a release binary.

This is an independent, experimental project, not an official OpenAI tool. Existing Codex CLI and a ChatGPT login with an available reset credit are required. Accepted Codex versions: **0.165.0 and 0.166.0**. Other versions fail closed until verified.

## Install from GitHub Releases

Releases are available at [jinzer0/codex-reset](https://github.com/jinzer0/codex-reset/releases). The first release is an alpha preview.

Download the installer for this release and run:

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.1/install.sh
sh install.sh
```

The installer detects OS/CPU, downloads the release archive and verifies its SHA256, then places `codex-reset` and its license notices in `~/.local/bin`. It does not require sudo, build code, execute Codex, or register a reservation. It reports how to add the directory to PATH if necessary. `--bin-dir /absolute/path` changes the destination. This installer defaults to `v0.4.0-alpha.1`. Use `--version vVERSION` to select another release.

Release workflow targets:

- `codex-reset-aarch64-apple-darwin.tar.gz` — Apple Silicon Mac
- `codex-reset-x86_64-apple-darwin.tar.gz` — Intel Mac
- `codex-reset-x86_64-unknown-linux-musl.tar.gz` — Ubuntu/Linux x86_64
- `SHA256SUMS` and `install.sh`

Alternatively, download the appropriate archive, verify its checksum, and place the executable in a stable location yourself. Do not move/delete that executable while it has pending reservations. Linux arm64 is not included in this first release configuration.

## Use

```sh
codex-reset
codex-reset check
codex-reset schedule --at '내일 04:00' --timezone Asia/Seoul --dry-run
codex-reset schedule --at 'tomorrow 04:00' --timezone Asia/Seoul
codex-reset status
codex-reset cancel JOB_ID
```

`check` can only read account and usage information. `--dry-run` previews the account/time and creates no reservation or OS job. **`schedule` without `--dry-run` asks for confirmation and then authorizes one real reset at the displayed time.** For scripts, `--yes` explicitly supplies that authorization. Installation itself never schedules or resets anything.

Time forms: `04:00` (next occurrence), `today 04:00`, `tomorrow 04:00`, `오늘 04:00`, `내일 04:00`, `2030-01-01 04:00`, or ISO datetime with an explicit offset. The default timezone is the detected IANA zone, otherwise UTC; the preview displays the exact date and offset. Ambiguous/nonexistent DST times require an explicit offset. Schedule at least 60 seconds ahead.

Use `--codex-bin /path/to/codex` for a non-default installed executable or npm launcher, and `--codex-home /path/to/existing/login` for another existing login directory. Authentication remains managed by Codex; this tool does not read/copy tokens. `--json` is available for scripts. Cancellation accepts unique job ID prefixes of at least eight characters.

## Background operation

Ubuntu uses a per-user systemd timer/service; macOS uses a launchd user agent. Closing the terminal does not stop a registered worker while the user manager/session remains available. Ubuntu logout persistence requires lingering; the tool warns if it is not confirmed and never changes that server setting. The computer must be running and awake at the scheduled time.

A durable SQLite claim allows **at most one attempt**, not guaranteed success. Jobs over five seconds late expire. Crashes, lost responses, and unknown outcomes do not trigger automatic retries. A claimed request cannot safely be retracted. Account/workspace and Codex binary hash are bound to each job and checked before dispatch. Upgrading Codex can invalidate reservations; cancel and schedule again after an upgrade.

The app-server opens shortly before dispatch and can retain account state during the final preflight window (up to 15 seconds). Logging out or switching accounts elsewhere during that window does not guarantee cancellation; use `cancel` while the job is pending.

State: `~/Library/Application Support/codex-reset` on Mac, or `$XDG_STATE_HOME/codex-reset` (default `~/.local/state/codex-reset`) on Linux. Rust uses `rust-jobs.sqlite3`, mode 0600, under a private directory. Logs are in `logs/`. `cancel` removes a cancelled/finished job's OS definition and retains history/logs. `pending` alone does not prove the OS worker is healthy.

Rust does **not** migrate or execute earlier Python reservations (`jobs.sqlite3`). Manage/cancel old jobs through that prototype before switching. Python remains a development reference/mock harness and is not included in Rust release archives. To uninstall, first cancel pending jobs, then delete the executable and its `codex-reset.LICENSES` sidecar.

## Build and verify

Contributors need Rust and a C compiler for bundled SQLite; end users do not.

```sh
cargo test --locked --all-targets
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
sh scripts/test-install.sh
cargo build --locked --release
./target/release/codex-reset --help
```

Integration tests use synthetic identities and a shell-only fake JSON-RPC server. Installer tests use fake downloads and never execute the installed program. `cargo run --example service_smoke` explicitly registers a temporary marker-writing OS job and removes it; it never starts Codex or uses an account.

[Verification details](docs/RUST_VERIFICATION.md) separate fake-RPC tests, real read-only checks, harmless OS scheduling, and coverage gaps. **Real-account redemption is intentionally untested.** Mock success or a successful account read does not prove production redemption.

## Release process

`.github/workflows/release.yml` builds three targets, tests, packages executables, combines checksums, and creates a **draft** release for a version tag matching Cargo.toml. Manual dispatch accepts an existing tag. Public publication is a separate step after review. Build and test results are available in [GitHub Actions](https://github.com/jinzer0/codex-reset/actions).

Do not upload the whole workspace: Codex source/binaries, credentials, local databases and artifacts are excluded from release archives. See [release notes](docs/RELEASE_NOTES.md).

## License

[MIT](LICENSE). Third-party components retain their respective licenses; notices are included in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) and the release archives.
