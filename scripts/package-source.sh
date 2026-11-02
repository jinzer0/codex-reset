#!/bin/sh
# Stage an explicit public-source allowlist; never archive the working directory.
set -eu
output=${1:-dist/source}
mkdir -p "$output"
work=$(mktemp -d "${TMPDIR:-/tmp}/codex-reset-source.XXXXXXXX")
trap 'rm -rf "$work"' EXIT HUP INT TERM
root=$work/codex-reset
mkdir -p "$root/src" "$root/tests" "$root/examples" "$root/scripts" "$root/docs" "$root/.github/workflows"
cp Cargo.toml Cargo.lock README.md install.sh .gitignore THIRD_PARTY_NOTICES.txt "$root/"
cp src/*.rs "$root/src/"
cp tests/*.rs "$root/tests/"
cp examples/*.rs "$root/examples/"
cp scripts/package-release.sh scripts/package-source.sh scripts/test-install.sh "$root/scripts/"
cp docs/RUST_VERIFICATION.md docs/RELEASE_NOTES.md "$root/docs/"
cp .github/workflows/tests.yml .github/workflows/release.yml "$root/.github/workflows/"
if [ -f LICENSE ]; then cp LICENSE "$root/"; fi
COPYFILE_DISABLE=1 tar -czf "$output/codex-reset-source.tar.gz" -C "$work" codex-reset
printf '%s\n' "$output/codex-reset-source.tar.gz"
