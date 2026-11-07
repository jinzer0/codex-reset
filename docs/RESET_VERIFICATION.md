# Reset verification

An authorized reset attempt records the server outcome and then checks account state through new read-only sessions. Verification never sends another reset request.

## Reading the result

```sh
codex-reset status
codex-reset status --json
```

Each job's `evidence` contains:

- `before`: the account fingerprint, masked email, available reset-credit count and usage limits observed before dispatch.
- `request_started_at` and `response_received_at`: local timestamps for the request attempt and successful RPC response. A request-start timestamp does not prove delivery. A JSON-RPC error is recorded in the error details instead of a successful-response timestamp.
- `server_outcome`: the server's reported outcome, including an unrecognized outcome when present. A local no-credit check does not fabricate a server response.
- `dispatch_error`: the failure phase and detailed RPC error, including method, request ID, server error code/message and available app-server stderr diagnostics. Credentials and token patterns are redacted; diagnostic output is bounded.
- `verification`: every post-request observation or lookup error, the usage and credit-count comparisons, and the verification status.

| Verification status | Meaning |
| --- | --- |
| `confirmed` | The server reported `reset`, previously positive usage windows were observed at zero, the available credit count decreased, and no scheduled usage rollover or selected-credit expiry explains the change. |
| `not_observed` | Comparable usage data was read, but at least one previously positive window was not at zero. |
| `inconclusive` | The evidence cannot confirm a reset: for example, usage was already zero, fields are missing, natural rollover is possible, the credit count did not decrease, or the reset response was lost. |
| `read_failed` | The latest verification lookup failed, including an account mismatch. The server outcome is preserved separately. |
| `not_requested` | No post-request verification was started, for example because preflight failed or no reset request was attempted. |

`completed` means the job received a recognized outcome; check `outcome` and verification separately. `nothingToReset`, `noCredit` and `alreadyRedeemed` do not mean this attempt performed a reset.

## Evidence and recovery

The before-state is saved before dispatch. The response or detailed dispatch failure is saved before verification begins. The job's terminal result is also saved before verification, so a lookup failure does not erase the response.

Verification makes at most three attempts, with waits of 250 ms and 750 ms before the second and third attempts. It stops early on confirmation or a non-reset server outcome. Each attempt rechecks account identity and records its result. Delayed reflection beyond these checks remains unconfirmed; simultaneous account activity can also prevent a clean zero-usage observation.

If the response is lost, the job stays `unknown`. Read-only recovery may observe lower usage and fewer credits, but does not relabel the missing server response as successful. The reset request is never retried.

The local state directory contains `rust-jobs.sqlite3` and per-job `logs/JOB_ID.out.log` / `logs/JOB_ID.err.log`. Defaults are `~/Library/Application Support/codex-reset` on macOS and `${XDG_STATE_HOME:-~/.local/state}/codex-reset` on Linux. JSON status includes local paths and account fingerprints; inspect it before sharing. Existing job records remain available after upgrading; older jobs have no verification evidence.

## Validation

Scheduled production resets with `0.4.0-alpha.2` and Codex CLI `0.167.0` were verified on Ubuntu x86_64 and macOS Apple Silicon on 2026-11-07 KST. Both jobs recorded server outcome `reset`, verification `confirmed`, usage reduced to zero and one fewer available reset credit. See [live system verification](RUST_VERIFICATION.md#live-system-verification--2026-11-07-kst).

The Rust test suite uses local stdio fixtures to simulate server replies and account-state changes.
