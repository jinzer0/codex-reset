# Codex Reset

[한국어](READE.ko.md) · [Releases](https://github.com/jinzer0/codex-reset/releases)

A command-line tool that schedules one attempt to **reset Codex usage** at a specified time.

- **Platforms** — macOS (Apple Silicon and Intel), Ubuntu/Linux x86_64
- **Codex CLI** — `0.165.0` or `0.166.0`
- **Account** — A ChatGPT account signed in to Codex CLI with an available reset credit

> This is an experimental, unofficial project. Production reset redemption remains unverified. [Verification details →](docs/RUST_VERIFICATION.md)

## Installation

```sh
curl -fsSLo install.sh https://github.com/jinzer0/codex-reset/releases/download/v0.4.0-alpha.1/install.sh
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

# Tomorrow at 04:00 (also accepts 'tomorrow 04:00')
codex-reset schedule --at '내일 04:00'

# Today at 04:00 (the time must still be ahead)
codex-reset schedule --at 'today 04:00'

# A specific date and time
codex-reset schedule --at '2026-01-01 04:00'
```

- **Timezone** — When `--timezone` is omitted, the tool uses the detected system timezone or falls back to UTC. Use `--timezone Asia/Seoul` for Korean time.
- **Scheduled time** — Choose a time **at least 60 seconds ahead**. Replace the example date with a future date.

### Authorize a reservation

1. **Review the details** — `schedule` shows the account and scheduled time before asking for confirmation.
2. **Confirm the reservation** — Answer `y` or `yes` to `Authorize one real reset at this time? [y/N]:` to **authorize the reset at the displayed time** and register the reservation. Pressing Enter or giving any other answer exits before registration.
3. **Run at the scheduled time** — The tool automatically attempts one reset for the displayed account.

| Mode | Reservation | Reset attempt |
| --- | --- | --- |
| `schedule` | Registers after confirmation | One attempt at the scheduled time |
| `--dry-run` | Previews the account and time only | None |
| `--yes` | Registers without a prompt, suitable for scripts | One attempt at the scheduled time |

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

> **`completed` alone does not confirm a successful reset.**
> Check the server outcome and `Usage verification` in `status`.

Use `codex-reset status --json` to see recorded usage before and after the attempt and detailed errors. [Result meanings →](docs/RESET_VERIFICATION.md)

## Running and canceling reservations

- **Power and sleep** — Keep your computer on and awake at the scheduled time.
- **Ubuntu logout** — Running after logout requires `lingering`. The tool displays a warning after registration if it cannot confirm the setting.
- **Delays and retries** — The tool skips the reset request when a job is **more than five seconds late**. Each reservation sends at most one reset request. Errors and missing responses do not trigger retries.
- **Codex upgrades** — An upgrade can prevent existing reservations from running. Cancel and reschedule them after upgrading.
- **Cancellation** — Run `cancel` while the reservation is `pending`. A reservation may still run after you sign out or switch accounts.

## License

[MIT](LICENSE). Third-party components retain their respective licenses. See the release archives and [third-party notices](THIRD_PARTY_NOTICES.txt).
