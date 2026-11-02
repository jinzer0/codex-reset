#!/bin/sh
# Package the native executable and required notices; never include credentials or test artifacts.
set -eu
[ "$#" -ge 1 ] && [ "$#" -le 3 ] || { printf 'Usage: %s TARGET [BINARY_PATH] [OUTPUT_DIR]\n' "$0" >&2; exit 2; }
target=$1
case "$target" in aarch64-apple-darwin|x86_64-apple-darwin|x86_64-unknown-linux-musl) ;; *) printf 'Unsupported target: %s\n' "$target" >&2; exit 2 ;; esac
binary=${2:-target/$target/release/codex-reset}
output=${3:-dist/release}
[ -f "$binary" ] && [ ! -L "$binary" ] && [ -x "$binary" ] || { printf 'Expected an executable regular binary: %s\n' "$binary" >&2; exit 2; }
[ -f THIRD_PARTY_NOTICES.txt ] && [ ! -L THIRD_PARTY_NOTICES.txt ] && [ -s THIRD_PARTY_NOTICES.txt ] || {
    printf 'THIRD_PARTY_NOTICES.txt must be a nonempty regular file\n' >&2; exit 2;
}
if [ -e LICENSE ] || [ -L LICENSE ]; then
    [ -f LICENSE ] && [ ! -L LICENSE ] && [ -s LICENSE ] || {
        printf 'LICENSE must be a nonempty regular file when present\n' >&2; exit 2;
    }
fi
umask 077
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/codex-reset-package.XXXXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
mkdir -p "$output"
cp "$binary" "$work_dir/codex-reset"
chmod 755 "$work_dir/codex-reset"
cp THIRD_PARTY_NOTICES.txt "$work_dir/THIRD_PARTY_NOTICES.txt"
set -- codex-reset THIRD_PARTY_NOTICES.txt
if [ -f LICENSE ]; then cp LICENSE "$work_dir/LICENSE"; set -- "$@" LICENSE; fi
asset=codex-reset-$target.tar.gz
COPYFILE_DISABLE=1 tar -czf "$output/$asset" -C "$work_dir" "$@"
if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$output/$asset" | awk '{print $1}')
else
    checksum=$(shasum -a 256 "$output/$asset" | awk '{print $1}')
fi
printf '%s  %s\n' "$checksum" "$asset" > "$output/$asset.sha256"
printf '%s\n' "$output/$asset"
