# Codex 0.167.0 verification — 2026-11-07 KST

Codex Reset accepts exactly `0.165.0`, `0.166.0` and `0.167.0`. Other versions, including prereleases and unverified patch versions, remain rejected before starting an app-server session.

## Protocol evidence

- Official tag: `rust-v0.167.0`, commit `086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a` in [openai/codex](https://github.com/openai/codex/tree/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a).
- The [account protocol definitions](https://github.com/openai/codex/blob/086ccc833bc9cf01024ad2dd6af7dbd421b6bf9a/codex-rs/app-server-protocol/src/protocol/v2/account.rs) are byte-identical to `0.166.0`. The relevant request and response contracts for `account/read`, `account/rateLimits/read` and `account/rateLimitResetCredit/consume` are also unchanged from `0.165.0`.
- The installed macOS arm64 executable reported `codex-cli 0.167.0`. Its SHA-256 was `030316ecfbb41f4e7946e584b0063ef333b7a36cd312ba349f2f6869e39b62cc`.
- That executable exported its JSON Schema with `app-server generate-json-schema --out <directory> --experimental`. This subcommand generates files without starting an app-server session. The export ran with empty temporary home/config directories, network access denied, and reads of the user's Codex directory and keychains denied. No authentication file was created.

The exported schema confirms the required `requiresOpenaiAuth`, workspace routing fields, `rateLimits`, credit `grantedAt`, consume `idempotencyKey` and response `outcome` fields. The permitted outcomes remain `reset`, `nothingToReset`, `noCredit` and `alreadyRedeemed`. Native fixture responses include these account and usage fields.

## Mock coverage

`tests/cli_mock.rs` compiles a small C executable inside each temporary test directory. Its version is selected at compile time. It has no network or credential-reading code and never invokes the installed Codex binary. Request traces and synthetic state stay under the temporary home directory.

- Supported-version checks exercise `0.165.0`, `0.166.0` and `0.167.0` through the public CLI.
- The `0.167.0` cases cover read-only checking, reservation previews, authorization gates, account mismatch and a single synthetic reset attempt without redispatch.
- Rejected versions are checked before any app-server traffic or reservation state is created.

## Validation results

- Before the allowlist change, the version-matrix test rejected `0.167.0` with the expected unsupported-version error. After the change, all three supported versions passed.
- `cargo test --locked --all-targets`: **58 passed** (40 library, 4 CLI parser/gate, 7 native-fixture CLI, 7 fake stdio safety tests). The native version tests include multiple versions within each test.
- `cargo fmt --all -- --check`, `cargo clippy --locked --all-targets -- -D warnings`, `git diff --check` and changed-document link checks: passed.
- `sh scripts/test-install.sh`: **19 offline installer checks passed**.
- `cargo build --locked --release`: passed on macOS arm64.
- Release executable `--help` / `--version` and source-archive packaging: passed. The archive includes both READMEs and this verification document; its local Markdown links resolve.

## Live system verification

Scheduled production resets with `codex-reset 0.4.0-alpha.2` and the installed Codex CLI `0.167.0` were verified on Ubuntu x86_64 and macOS Apple Silicon on 2026-11-07 KST. Both jobs completed with server outcome `reset` and verification `confirmed`. Usage was observed at zero and the available reset-credit count decreased by one on each account. See [live system verification](RUST_VERIFICATION.md#live-system-verification--2026-11-07-kst) for the results.
