#!/bin/sh
# Download a published standalone binary. This script never runs codex-reset.
set -eu

usage() {
    cat <<'EOF'
Usage: sh install.sh [--repo OWNER/REPO] [--version vVERSION] [--bin-dir DIRECTORY]

Installs codex-reset from GitHub Releases without Rust, Python or sudo.
--repo      GitHub repository (default: jinzer0/codex-reset).
            Alternatively set CODEX_RESET_REPO.
--version   Release tag (default: v0.4.0-alpha.1).
--bin-dir   Destination directory (default: $HOME/.local/bin).
EOF
}
fail() { printf 'Error: %s\n' "$*" >&2; exit 1; }
need_value() { [ "$#" -ge 2 ] && [ -n "$2" ] || fail "$1 requires a value"; }
repo=${CODEX_RESET_REPO:-jinzer0/codex-reset}
version=v0.4.0-alpha.1
bin_dir=${HOME:?HOME is required}/.local/bin
while [ "$#" -gt 0 ]; do
    case "$1" in
        --repo) need_value "$@"; repo=$2; shift 2 ;;
        --version) need_value "$@"; version=$2; shift 2 ;;
        --bin-dir) need_value "$@"; bin_dir=$2; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) fail "unknown argument: $1" ;;
    esac
done
case "$repo" in
    */*) owner=${repo%%/*}; project=${repo#*/} ;;
    *) fail 'set --repo OWNER/REPO or CODEX_RESET_REPO' ;;
esac
for segment in "$owner" "$project"; do
    case "$segment" in ''|.|..|*[!A-Za-z0-9_.-]*) fail 'invalid GitHub repository' ;; esac
done
case "$version" in *[!A-Za-z0-9._-]*) fail 'invalid release tag' ;; esac
if [ -n "$version" ]; then
    case "$version" in v[0-9]*) ;; *) fail 'release tag must start with v and a version number' ;; esac
fi
case "$bin_dir" in /*) ;; *) fail '--bin-dir must be an absolute path' ;; esac
case "$bin_dir" in *'
'*) fail 'destination must not contain a newline' ;; esac

system=$(uname -s)
arch=$(uname -m)
case "$system/$arch" in
    Darwin/arm64|Darwin/aarch64) target=aarch64-apple-darwin ;;
    Darwin/x86_64) target=x86_64-apple-darwin ;;
    Linux/x86_64|Linux/amd64) target=x86_64-unknown-linux-musl ;;
    *) fail "unsupported platform: $system/$arch (available: macOS arm64/x86_64, Linux x86_64)" ;;
esac
for command in curl tar awk mktemp chmod mv mkdir cut cat rm; do
    command -v "$command" >/dev/null 2>&1 || fail "$command is required"
done
if command -v sha256sum >/dev/null 2>&1; then
    hash_command=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    hash_command=shasum
else
    fail 'sha256sum or shasum is required'
fi
umask 077
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/codex-reset-install.XXXXXXXX")
stage=
notice_stage=
cleanup() {
    [ -z "$stage" ] || rm -f "$stage"
    [ -z "$notice_stage" ] || rm -f "$notice_stage"
    rm -rf "$work_dir"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' HUP TERM
asset=codex-reset-$target.tar.gz
base=https://github.com/$repo/releases
if [ -n "$version" ]; then
    base=$base/download/$version
else
    base=$base/latest/download
fi
printf 'Downloading %s from %s...\n' "$asset" "$repo"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 180 \
    "$base/$asset" --output "$work_dir/$asset"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 --connect-timeout 15 --max-time 60 \
    "$base/SHA256SUMS" --output "$work_dir/SHA256SUMS"
expected=$(awk -v name="$asset" '$2 == name {count++; value=$1} END {if (count == 1) print value; else exit 1}' "$work_dir/SHA256SUMS") || fail 'missing or duplicate archive checksum'
[ "${#expected}" -eq 64 ] || fail 'invalid SHA256 checksum'
case "$expected" in *[!0-9a-f]*) fail 'invalid SHA256 checksum' ;; esac
if [ "$hash_command" = sha256sum ]; then
    actual=$(sha256sum "$work_dir/$asset" | awk '{print $1}')
else
    actual=$(shasum -a 256 "$work_dir/$asset" | awk '{print $1}')
fi
[ "$expected" = "$actual" ] || fail 'SHA256 checksum mismatch; nothing was installed'
listing=$(tar -tzf "$work_dir/$asset") || fail 'cannot read release archive'
printf '%s\n' "$listing" | awk '
    $0 == "codex-reset" {binary++; next}
    $0 == "THIRD_PARTY_NOTICES.txt" {notices++; next}
    $0 == "LICENSE" {license++; next}
    {exit 1}
    END {if (binary != 1 || notices != 1 || license > 1) exit 1}
' || fail 'unexpected archive contents; expected binary, notices, and optional LICENSE exactly once'
entry_count=$(printf '%s\n' "$listing" | awk 'END {print NR}')
types=$(tar -tvzf "$work_dir/$asset" | cut -c 1) || fail 'cannot inspect release archive'
printf '%s\n' "$types" | awk -v count="$entry_count" '
    $0 != "-" {exit 1}
    END {if (NR != count) exit 1}
' || fail 'release archive entries must be regular files'
[ ! -L "$bin_dir" ] || fail 'destination directory must not be a symlink'
mkdir -p "$bin_dir"
[ -d "$bin_dir" ] || fail 'destination is not a directory'
destination=$bin_dir/codex-reset
notice_destination=$bin_dir/codex-reset.LICENSES
for path in "$destination" "$notice_destination"; do
    [ ! -L "$path" ] || fail 'refusing to replace a symlink'
    [ ! -e "$path" ] || [ -f "$path" ] || fail 'destination is not a regular file'
done
stage=$(mktemp "$bin_dir/.codex-reset.XXXXXXXX")
notice_stage=$(mktemp "$bin_dir/.codex-reset-notices.XXXXXXXX")
# Stream named entries into fixed paths; never extract archive paths to disk.
tar -xOzf "$work_dir/$asset" codex-reset > "$stage" || fail 'cannot extract codex-reset'
[ -s "$stage" ] || fail 'release binary is empty'
tar -xOzf "$work_dir/$asset" THIRD_PARTY_NOTICES.txt > "$notice_stage" || fail 'cannot extract notices'
[ -s "$notice_stage" ] || fail 'release notices are empty'
if printf '%s\n' "$listing" | awk '$0 == "LICENSE" {found=1} END {exit !found}'; then
    printf '\n\nProject license\n===============\n' >> "$notice_stage"
    tar -xOzf "$work_dir/$asset" LICENSE > "$work_dir/project-license" || fail 'cannot extract project license'
    [ -s "$work_dir/project-license" ] || fail 'project license is empty'
    cat "$work_dir/project-license" >> "$notice_stage"
fi
chmod 755 "$stage"
chmod 644 "$notice_stage"
# Recheck both fixed destinations before replacing either. Final symlinks are not followed.
for path in "$destination" "$notice_destination"; do
    [ ! -L "$path" ] || fail 'refusing to replace a symlink'
    [ ! -e "$path" ] || [ -f "$path" ] || fail 'destination is not a regular file'
done
# Notices are placed before the executable so every newly installed executable has them.
mv -f "$notice_stage" "$notice_destination"
notice_stage=
mv -f "$stage" "$destination"
stage=
printf 'Installed: %s\nNotices: %s\n' "$destination" "$notice_destination"
case ":${PATH:-}:" in
    *":$bin_dir:"*) printf 'Run: codex-reset\n' ;;
    *) printf 'Run: "%s"\nAdd "%s" to PATH to use the codex-reset command.\n' "$destination" "$bin_dir" ;;
esac
printf 'No reservation was created. The installed program was not executed.\n'
