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
