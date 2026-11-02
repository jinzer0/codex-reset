# Codex Reset

A command-line tool for scheduling a one-time Codex usage reset on macOS or Ubuntu.

Requires Codex CLI **0.165.0 or 0.166.0** and a ChatGPT login with an available reset credit. Other Codex versions are rejected.

This is an experimental, unofficial project. Real-account redemption has not been tested; see [verification details](docs/RUST_VERIFICATION.md).

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.1/install.sh
sh install.sh
```

Supports Apple Silicon Mac, Intel Mac, and Linux x86_64. See [releases](https://github.com/jinzer0/codex-reset/releases) for downloads.

## Usage

Open the interactive menu:

```sh
codex-reset
```

Or schedule from the command line:

```sh
# Next 04:00
codex-reset schedule --at '04:00'

# Tomorrow at 04:00 (also accepts 'tomorrow 04:00')
codex-reset schedule --at '내일 04:00'

# Today at 04:00
codex-reset schedule --at 'today 04:00'

# A specific date and time
codex-reset schedule --at '2026-01-01 04:00'
```

Times use your detected timezone, or UTC if detection fails. Add `--timezone Asia/Seoul` to choose one explicitly. Replace the example date with a future date and schedule at least 60 seconds ahead.

**Scheduling asks for confirmation to perform one real reset at the displayed time.** Add `--dry-run` to preview without reserving, or `--yes` to confirm in scripts.

Check usage, view reservations, or cancel:

```sh
codex-reset check
codex-reset status
codex-reset cancel JOB_ID
```

## Scheduling

Keep your computer running and awake at the scheduled time. On Ubuntu, running after logout requires lingering; the tool warns if it cannot confirm this setting.

Each reservation allows at most one reset attempt. Jobs over five seconds late expire. Failed attempts and unknown outcomes are not retried. Cancel and reschedule after upgrading Codex.

Cancel while a job is pending. Once a reset attempt begins, cancellation is not guaranteed. Logging out or switching accounts does not guarantee cancellation either.

## License

[MIT](LICENSE). Third-party components retain their respective licenses; notices are included in [THIRD_PARTY_NOTICES.txt](THIRD_PARTY_NOTICES.txt) and the release archives.
