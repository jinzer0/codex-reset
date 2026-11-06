# v0.4.0-alpha.2 — Codex 0.167 support and reset verification

Accept Codex CLI `0.167.0` alongside `0.165.0` and `0.166.0`. Native fixtures cover version discovery, read-only commands and one-attempt scheduling without contacting Codex or a real account. See [0.167.0 verification](CODEX_0_167_VERIFICATION.md) for the offline protocol checks and validation boundary.

Reset attempts now save before/after usage and credit counts, server outcomes and detailed errors. Read-only verification makes up to three observations without retrying the reset request. `status` displays verification separately from the server outcome; `status --json` exposes the recorded evidence. See [reset verification](RESET_VERIFICATION.md).

Diagnostics retain bounded error context and redact credential patterns, including their continuation lines. Existing reservation records remain readable. English and Korean installation instructions and the installer default point to this release.

Available builds: macOS arm64, macOS x86_64 and Linux x86_64-musl. This remains an experimental prerelease; production account redemption has not been tested.

# v0.4.0-alpha.1 — Rust standalone preview

The public CLI is a native Rust executable. Users no longer need Python or pipx. An interactive menu and `check`, `schedule`, `status`, `cancel` commands support macOS launchd and Ubuntu systemd scheduling.

Account checks and previews cannot send consume requests. Scheduling requires confirmation or `--yes`. Workers bind account/workspace and Codex binary hash, claim durably before dispatch, and never retry unknown outcomes. Installation only downloads/verifies/places the executable.

Rust uses a separate `rust-jobs.sqlite3` database. It does not migrate, cancel, or execute Python reservations. Manage old jobs with the old tool before switching.

Release configuration targets macOS arm64/x86_64 and Linux x86_64-musl. Linux arm64 is not included. Accepted installed Codex versions are 0.165.0 and 0.166.0; this project's independent version number does not track Codex's version.

See RUST_VERIFICATION.md for limits. Real-account redemption is intentionally untested. Source: https://github.com/jinzer0/codex-reset. Project license: MIT. Third-party notices accompany each release archive.
