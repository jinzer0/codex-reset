# Rust verification

## Live system verification — 2026-11-07 KST

User-authorized reservations ran with `codex-reset 0.4.0-alpha.2` and Codex CLI `0.167.0` on both systems. Each job completed with server outcome `reset` and read-only usage verification `confirmed`.

| Platform | Scheduler | Scheduled time (KST) | Usage | Available reset credits |
| --- | --- | --- | --- | --- |
| Ubuntu x86_64 | systemd user service | 2026-11-07 02:30 | 100% → 0% | 3 → 2 |
| macOS Apple Silicon | launchd | 2026-11-07 08:25 | 81% → 0% | 3 → 2 |

The reservation database and per-job output logs agreed. Both services ran once, exited with status `0` and produced empty error logs. Readback confirmed the usage and credit-count changes before any natural usage rollover or selected-credit expiry.

## v0.4.0-alpha.1 verification — 2026-11-02 KST

Rust replaces the public Python/pipx entry point; legacy Python remains reference/test tooling.

### macOS

- Existing Rust 1.99.1: Apple Silicon optimized standalone executable built. Intel Mac executable cross-built and confirmed Mach-O x86_64; it was not executed on Intel hardware or Rosetta.
- Rust tests: **57 passed** (40 library, 4 CLI parser/gate, 7 fake stdio integration, 6 native-fixture public CLI).
- Formatting and clippy with `-D warnings`: passed.
- Installer: **19 offline cases passed**, including checksum failure, archive contents, symlinks, unsupported platform, spaces, and no automatic execution.
- Installed Codex 0.166.0: Rust `check` succeeded with `reset_requests_sent: 0`.
- Real launchd smoke: fixed shell builtin wrote a synthetic marker; temporary job removed; Codex never started.

### Ubuntu

Ubuntu 24.04.3 x86_64, existing Rust 1.95.0, temporary source/build directories. No separate Rust installation or server security-policy change.

- **57 Rust tests passed**: 51 core/fake-stdio tests, then six additional native-fixture public CLI tests in a separate run.
- Clippy and all **19 offline installer cases** passed.
- Native GNU release build read the real existing Codex 0.165.0 account successfully; reset requests: zero.
- Real systemd smoke registered/ran a harmless marker command and removed it. No Codex/account use in that job.
- Linux musl release binary built using the existing Rust installation's musl target and official Ubuntu musl packages extracted only into a temporary directory. No sudo/package installation. Remote temporary directory removed.
- musl binary `--version`/`--help` passed on Ubuntu; `file` reports static-pie, `readelf` reports no interpreter or shared-library dependencies.
- No persistent application or real reset job installed.

Private evidence excluded from release:

- `artifacts/rust-tests.log`
- `artifacts/rust-ubuntu-verification.log`
- `artifacts/rust-ubuntu-cli-verification.log`
- `artifacts/rust-mac-readonly.json` (0600)
- `artifacts/linux-musl/build.log`
- `artifacts/rust-linux-static-readonly.json`

### Safety cases and fixes

Tests cover durable single claim across concurrency/restart, cancellation races, deadline sampling after database lock/commit, unknown outcomes without retry, account mismatch, absent/expired credits, explicit worker authorization, read-only pre-write denial, duplicate session denial, and public CLI dry-run/noninteractive gates. Native C fixtures only read stdin/write stdout; they are not Codex and have no network/authentication behavior.

A smoke fixture initially used a 0755 temporary state directory. It now creates 0700 explicitly. The implementation accepts owned 0755 OS definition directories unchanged while requiring private state/log directories; regressions distinguish the cases.

Review tightened HTTPS authority checks and process-group cleanup. The leader stays unreaped until group cleanup so its PID cannot be recycled. No reset authorization bypass was found in review.

### Boundaries

- Full installed Codex reset-to-HTTP-mock integration under Linux kernel isolation remains unverified: this shared host blocks namespace setup. Fake stdio integration does not replace that test.
- Installed macOS 0.166 managed preferences loading conflicts with the previous strict mock sandbox. It was not weakened. The previous standalone debug 0.165 harness is Python-era evidence, not a Rust end-to-end result.
- Sleep/reboot/logout behavior has not been exercised. The Ubuntu user had lingering enabled; no setting was changed.
- The results above are local pre-publication verification. Current target-build results are recorded in https://github.com/jinzer0/codex-reset/actions. Cross-compiled artifacts do not imply native hardware integration coverage.

## Reset diagnostics and readback — 2026-11-05 KST

The working tree now preserves detailed, bounded RPC diagnostics and before/after verification evidence. Server outcomes and usage verification are recorded separately; recovery uses fresh read-only sessions and never retries a reset request. See [reset verification](RESET_VERIFICATION.md).

- `cargo test --locked --all-targets`: **79 passed** (53 library, 4 CLI parser/gate, 12 native-fixture CLI, 10 fake stdio safety).
- `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings` and `git diff --check`: passed.
- Regression cases include unchanged usage after a `reset` response, delayed state reflection, failed/account-mismatched readback, lost response after a synthetic state change, natural usage rollover, credit expiry, missing usage data, zero baselines, evidence persistence failure and old database compatibility.
- Long stderr cannot displace the primary error. Credential-pattern redaction includes camelCase token keys; raw RPC parameters and error data are not logged.
- Regression scenarios use local fake stdio servers or native fixtures.

## Codex 0.167.0 compatibility — 2026-11-07 KST

See [0.167.0 verification](CODEX_0_167_VERIFICATION.md) for the installed binary's offline schema export, version-specific native mock coverage and validation results.

## v0.4.0-alpha.2 release preparation — 2026-11-07 KST

- Merged source: **82 Rust tests passed** (55 library, 4 CLI parser/gate, 13 native-fixture CLI, 10 fake stdio safety).
- Formatting, Clippy with warnings denied, shell syntax and diff checks passed.
- **19 offline installer checks passed**, including the new default release tag and explicit installation of the previous tag.
- macOS arm64 release build and `--help` / `--version` smoke checks passed; the executable reports `0.4.0-alpha.2`.
- Source packaging passed. Platform release builds and their checks run in the tag-triggered GitHub Release workflow.
