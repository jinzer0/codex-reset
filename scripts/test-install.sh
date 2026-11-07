#!/bin/sh
# Offline installer regression tests. curl is replaced by a fixture copier.
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
work_dir=$(mktemp -d "${TMPDIR:-/tmp}/codex-reset-install-tests.XXXXXXXX")
trap 'rm -rf "$work_dir"' EXIT HUP INT TERM
mkdir -p "$work_dir/tools" "$work_dir/fixture" "$work_dir/input" "$work_dir/home"
cat > "$work_dir/tools/uname" <<'EOF'
#!/bin/sh
case "$1" in -s) printf '%s\n' "${TEST_SYSTEM:-Linux}" ;; -m) printf '%s\n' "${TEST_ARCH:-x86_64}" ;; *) exit 2 ;; esac
EOF
cat > "$work_dir/tools/curl" <<'EOF'
#!/bin/sh
set -eu
url=
output=
while [ "$#" -gt 0 ]; do
    case "$1" in
        --output) output=$2; shift 2 ;;
        https://*) url=$1; shift ;;
        *) shift ;;
    esac
done
[ -n "$url" ] && [ -n "$output" ] || exit 2
case "$url" in https://github.com/test-owner/test-repo/releases/*|https://github.com/jinzer0/codex-reset/releases/*) ;; *) exit 2 ;; esac
printf '%s\n' "$url" >> "$TEST_REQUEST_LOG"
cp "$TEST_FIXTURE/${url##*/}" "$output"
EOF
chmod +x "$work_dir/tools/uname" "$work_dir/tools/curl"
cat > "$work_dir/input/codex-reset" <<'EOF'
#!/bin/sh
: > "$TEST_EXECUTION_MARKER"
EOF
chmod +x "$work_dir/input/codex-reset"
printf 'Synthetic third-party notices\n' > "$work_dir/input/THIRD_PARTY_NOTICES.txt"
printf 'Synthetic project license\n' > "$work_dir/input/LICENSE"
export TEST_FIXTURE=$work_dir/fixture
export TEST_REQUEST_LOG=$work_dir/requests
export TEST_EXECUTION_MARKER=$work_dir/was-executed
passed=0
run_installer() {
    env HOME="$work_dir/home" PATH="$work_dir/tools:$PATH" sh "$root/install.sh" --repo test-owner/test-repo "$@"
}
pass() { passed=$((passed + 1)); printf 'PASS %s\n' "$1"; }
expect_failure() {
    name=$1; shift
    if run_installer "$@" > "$work_dir/stdout" 2> "$work_dir/stderr"; then
        printf 'FAIL %s: unexpectedly succeeded\n' "$name" >&2; exit 1
    fi
    pass "$name"
}
package() {
    (cd "$work_dir/input"; sh "$root/scripts/package-release.sh" "$1" "$work_dir/input/codex-reset" "$TEST_FIXTURE") >/dev/null
    cat "$TEST_FIXTURE"/*.sha256 > "$TEST_FIXTURE/SHA256SUMS"
}
archive_fixture() {
    COPYFILE_DISABLE=1 tar -czf "$TEST_FIXTURE/codex-reset-x86_64-unknown-linux-musl.tar.gz" -C "$work_dir/input" "$@"
    if command -v sha256sum >/dev/null 2>&1; then
        digest=$(sha256sum "$TEST_FIXTURE/codex-reset-x86_64-unknown-linux-musl.tar.gz" | awk '{print $1}')
    else
        digest=$(shasum -a 256 "$TEST_FIXTURE/codex-reset-x86_64-unknown-linux-musl.tar.gz" | awk '{print $1}')
    fi
    printf '%s  codex-reset-x86_64-unknown-linux-musl.tar.gz\n' "$digest" > "$TEST_FIXTURE/SHA256SUMS"
}
package x86_64-unknown-linux-musl
package aarch64-apple-darwin
package x86_64-apple-darwin
run_installer --version v0.4.0-alpha.1 --bin-dir "$work_dir/path with spaces" > "$work_dir/stdout"
cmp "$work_dir/input/codex-reset" "$work_dir/path with spaces/codex-reset"
[ -x "$work_dir/path with spaces/codex-reset" ]
grep -q 'Synthetic third-party notices' "$work_dir/path with spaces/codex-reset.LICENSES"
grep -q 'Synthetic project license' "$work_dir/path with spaces/codex-reset.LICENSES"
[ ! -e "$TEST_EXECUTION_MARKER" ]
[ -z "$(find "$work_dir/path with spaces" -name '.codex-reset*' -print)" ]
pass 'explicit alpha, notices and license, spaces, atomic staging, no execution'
grep -q '/download/v0.4.0-alpha.1/' "$TEST_REQUEST_LOG"
env HOME="$work_dir/home" PATH="$work_dir/tools:$PATH" CODEX_RESET_REPO= sh "$root/install.sh" > "$work_dir/stdout"
[ -x "$work_dir/home/.local/bin/codex-reset" ]
grep -q 'https://github.com/jinzer0/codex-reset/releases/download/v0.4.0/' "$TEST_REQUEST_LOG"
pass 'default repository, pinned stable release and user directory'
TEST_SYSTEM=Darwin TEST_ARCH=arm64 run_installer --bin-dir "$work_dir/mac-arm" > "$work_dir/stdout"
grep -q 'codex-reset-aarch64-apple-darwin.tar.gz' "$TEST_REQUEST_LOG"
TEST_SYSTEM=Darwin TEST_ARCH=x86_64 run_installer --bin-dir "$work_dir/mac-intel" > "$work_dir/stdout"
grep -q 'codex-reset-x86_64-apple-darwin.tar.gz' "$TEST_REQUEST_LOG"
pass 'macOS arm64 and Intel target selection'
cp "$TEST_FIXTURE/SHA256SUMS" "$work_dir/checksums"
printf '%064d  codex-reset-x86_64-unknown-linux-musl.tar.gz\n' 0 > "$TEST_FIXTURE/SHA256SUMS"
expect_failure 'checksum mismatch preserves existing files' --bin-dir "$work_dir/path with spaces"
cmp "$work_dir/input/codex-reset" "$work_dir/path with spaces/codex-reset"
grep -q 'Synthetic project license' "$work_dir/path with spaces/codex-reset.LICENSES"
cp "$work_dir/checksums" "$TEST_FIXTURE/SHA256SUMS"
cat "$work_dir/checksums" >> "$TEST_FIXTURE/SHA256SUMS"
expect_failure 'duplicate checksum rejected' --bin-dir "$work_dir/duplicate"
cp "$work_dir/checksums" "$TEST_FIXTURE/SHA256SUMS"
mkdir "$work_dir/symlink-destination"
ln -s "$work_dir/input/codex-reset" "$work_dir/symlink-destination/codex-reset"
expect_failure 'symlink binary rejected' --bin-dir "$work_dir/symlink-destination"
mkdir "$work_dir/notice-link"
ln -s "$work_dir/input/THIRD_PARTY_NOTICES.txt" "$work_dir/notice-link/codex-reset.LICENSES"
expect_failure 'symlink notices destination rejected before binary install' --bin-dir "$work_dir/notice-link"
[ ! -e "$work_dir/notice-link/codex-reset" ]
ln -s "$work_dir/symlink-destination" "$work_dir/directory-link"
expect_failure 'symlink directory rejected' --bin-dir "$work_dir/directory-link"
TEST_SYSTEM=Linux TEST_ARCH=aarch64 expect_failure 'unsupported platform rejected' --bin-dir "$work_dir/unsupported"
unset TEST_SYSTEM TEST_ARCH
expect_failure 'relative destination rejected' --bin-dir relative
expect_failure 'invalid repository rejected' --repo '../escape'
expect_failure 'invalid tag rejected' --version '../../escape'
# Valid checksums do not authorize unexpected, duplicate, linked or missing entries.
printf 'unexpected\n' > "$work_dir/input/extra"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt extra
expect_failure 'unexpected archive entry rejected' --bin-dir "$work_dir/archive-extra"
archive_fixture codex-reset
expect_failure 'missing notices rejected' --bin-dir "$work_dir/archive-missing-notices"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt THIRD_PARTY_NOTICES.txt
expect_failure 'duplicate archive entry rejected' --bin-dir "$work_dir/archive-duplicate"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt
run_installer --bin-dir "$work_dir/no-project-license" > "$work_dir/stdout"
cmp "$work_dir/input/THIRD_PARTY_NOTICES.txt" "$work_dir/no-project-license/codex-reset.LICENSES"
pass 'optional project license may be absent while third-party notices remain'
rm "$work_dir/input/codex-reset"
ln -s extra "$work_dir/input/codex-reset"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt
expect_failure 'archive symlink rejected' --bin-dir "$work_dir/archive-symlink"
rm "$work_dir/input/codex-reset"
printf 'fixture binary\n' > "$work_dir/input/codex-reset"
rm "$work_dir/input/THIRD_PARTY_NOTICES.txt"
ln -s extra "$work_dir/input/THIRD_PARTY_NOTICES.txt"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt
expect_failure 'archive notices symlink rejected' --bin-dir "$work_dir/archive-notices-symlink"
rm "$work_dir/input/THIRD_PARTY_NOTICES.txt"
: > "$work_dir/input/THIRD_PARTY_NOTICES.txt"
archive_fixture codex-reset THIRD_PARTY_NOTICES.txt
expect_failure 'empty notices rejected before install' --bin-dir "$work_dir/archive-empty-notices"
[ ! -e "$work_dir/archive-empty-notices/codex-reset" ]
[ ! -e "$TEST_EXECUTION_MARKER" ]
printf '%s offline installer checks passed; no real download or installed-program execution.\n' "$passed"
