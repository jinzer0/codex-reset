# v0.4.1

- Add support for Codex CLI `0.167.1`, `0.167.2`, `0.168.0`, `0.169.0`, `0.169.1`, `0.170.0`, `0.171.0`, and `0.172.0`.
- Run the native stdio mock scenarios against all 11 supported releases, including reservation previews, authorization checks, single dispatch, and read-only recovery. [Verification](RUST_VERIFICATION.md#codex-01720-compatibility)
- Install with the versioned `install.sh` from the [v0.4.1 release](https://github.com/jinzer0/codex-reset/releases/tag/v0.4.1).

# v0.4.0

## Changes

- Support Codex CLI `0.165.0`, `0.166.0` and `0.167.0`.
- Show reset outcomes and verification in `status`, with usage and credit details in `status --json`.
- Switch the project license to Unlicense.

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0/install.sh
sh install.sh
```

Available for macOS Apple Silicon, macOS Intel and Linux x86_64.

# v0.4.0-alpha.2 — Codex 0.167 support and reset verification

## Changes

- Support Codex CLI `0.167.0`, alongside `0.165.0` and `0.166.0`.
- Add usage and reset-credit evidence to `status --json` and show verification results in `status`.
- Improve RPC error details and credential redaction.

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.2/install.sh
sh install.sh
```

Available for macOS Apple Silicon, macOS Intel and Linux x86_64.

# v0.4.0-alpha.1 — Rust standalone CLI

## Changes

- Standalone Rust CLI with interactive scheduling, `check`, `status` and `cancel`.
- Schedule resets through macOS launchd and Ubuntu systemd.
- Support Codex CLI `0.165.0` and `0.166.0`.

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.1/install.sh
sh install.sh
```

Available for macOS Apple Silicon, macOS Intel and Linux x86_64.
