# Codex Reset

[한국어](READE.ko.md) · [Releases](https://github.com/jinzer0/codex-reset/releases)

A command-line tool that schedules **Codex usage resets** at a specified time.

- **Platforms** — macOS (Apple Silicon and Intel), Ubuntu/Linux x86_64
- **Codex CLI** — `0.165.0`, `0.166.0`, or `0.167.0`
- **Account** — A ChatGPT account signed in to Codex CLI with an available reset credit

> [!NOTE]
> This is an unofficial project. Scheduled resets with Codex CLI `0.167.0` have been verified on Ubuntu x86_64 and macOS Apple Silicon, including usage reset and reset-credit consumption. [Verification details →](docs/RUST_VERIFICATION.md)

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0/install.sh
sh install.sh
```

## Usage

```sh
codex-reset
```

### Specify a time

```sh
# Next 04:00 (today or tomorrow)
codex-reset schedule --at '04:00'

# Tomorrow at 04:00
codex-reset schedule --at '내일 04:00'
```

- **Timezone** — Use `--timezone Asia/Seoul` for Korean time.
- **Scheduled time** — Choose a time **at least 60 seconds ahead**.

### Authorize a reservation

`schedule` shows the account and scheduled time. Confirm with `y` or `yes` to register the reservation.

- `--dry-run` previews the account and time.
- `--yes` registers without a confirmation prompt.

### Check and cancel reservations

```sh
# Check the signed-in account and available reset credits
codex-reset check

# List reservations and their outcomes
codex-reset status

# Cancel a reservation
codex-reset cancel JOB_ID
```

Replace `JOB_ID` with the reservation ID in the first column of `status`.

Use `codex-reset status --json` to see recorded usage before and after the attempt and detailed errors. [Result meanings →](docs/RESET_VERIFICATION.md)

## Running and canceling reservations

- **Power and sleep** — Keep your computer on and awake at the scheduled time.
- **Ubuntu logout** — Running after logout requires `lingering`.
- **Delays and retries** — The tool skips the reset request when a job is **more than five seconds late**. Each reservation sends at most one reset request.
- **Codex upgrades** — Cancel and reschedule reservations after upgrading Codex.
- **Cancellation** — Run `cancel` while the reservation is `pending`.

## License

[Unlicense](LICENSE) · [Third-party notices](THIRD_PARTY_NOTICES.txt)
