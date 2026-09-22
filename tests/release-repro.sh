#!/usr/bin/env bash
# M13.10: two isolated builds of one release artifact must compare byte-for-byte.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-repro.sh"
release="$ROOT/.github/workflows/release.yml"
test -x "$script"
grep -Fq 'BUILD_SCRIPT="${OQUEUE_REPRO_BUILD_SCRIPT:-$ROOT/scripts/release-build.sh}"' "$script"
workspace_version="$(awk '
  { sub(/\r$/, "") }
  $0 == "[workspace.package]" { section = 1; next }
  section && /^\[/ { exit }
  section && /^version = "/ {
    line = $0
    sub(/^version = "/, "", line)
    sub(/"$/, "", line)
    print line
    exit
  }
' "$ROOT/Cargo.toml")"

scratch="$ROOT/target/tmp/release-repro-test.$$"
mkdir -p "$scratch"
trap 'rm -rf "$scratch"' EXIT

fake="$scratch/fake-release-build.sh"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'set -euo pipefail' \
  'artifact="${CARGO_TARGET_DIR:?}/${OQUEUE_RELEASE_TARGET:?}.2.28/release/oqueue"' \
  'mkdir -p "$(dirname "$artifact")"' \
  'printf "%s\\n" "deterministic release artifact" > "$artifact"' \
  'printf "%s\\n" "fake-build ${OQUEUE_REPRO_BUILD_ID:?}"' \
  > "$fake"
chmod +x "$fake"

evidence="$scratch/evidence"
OQUEUE_REPRO_BUILD_SCRIPT="$fake" OQUEUE_M13_EVIDENCE_DIR="$evidence" \
  bash "$script" --completion-gate >/dev/null
line="$(<"$evidence/repro.tsv")"
[[ "$line" =~ ^M13_REPRODUCIBILITY\ run_id=[0-9a-f]{40}\ release_artifact_sha256=[0-9a-f]{64}\ mode=byte-identical\ release_build=a\ build_a=[^[:space:]]+\ sha256_a=[0-9a-f]{64}\ build_b=[^[:space:]]+\ sha256_b=[0-9a-f]{64}\ comparison=equal\ commands_sha256=[0-9a-f]{64}\ outputs_sha256=[0-9a-f]{64}$ ]]
build_a="$(sed -n 's/.* build_a=\([^ ]*\).*/\1/p' <<<"$line")"
build_b="$(sed -n 's/.* build_b=\([^ ]*\).*/\1/p' <<<"$line")"
[[ "$build_a" != "$build_b" ]]

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}
hash_a="$(hash_file "$build_a")"
hash_b="$(hash_file "$build_b")"
sha_a="$(sed -n 's/.* sha256_a=\([^ ]*\).*/\1/p' <<<"$line")"
sha_b="$(sed -n 's/.* sha256_b=\([^ ]*\).*/\1/p' <<<"$line")"
release_sha="$(sed -n 's/.* release_artifact_sha256=\([^ ]*\).*/\1/p' <<<"$line")"
[[ "$hash_a" == "$sha_a" && "$hash_b" == "$sha_b" && "$sha_a" == "$release_sha" ]]
commands_sha="$(hash_file "$evidence/commands.log")"
recorded_commands="$(sed -n 's/.* commands_sha256=\([^ ]*\).*/\1/p' <<<"$line")"
[[ "$commands_sha" == "$recorded_commands" ]]
outputs_sha="$(cat "$evidence/build-a.log" "$evidence/build-b.log" | \
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi)"
recorded_outputs="$(sed -n 's/.* outputs_sha256=\([^ ]*\).*/\1/p' <<<"$line")"
[[ "$outputs_sha" == "$recorded_outputs" ]]
expected_name="oqueue-$workspace_version-linux-x86_64-glibc2.28-default-bin"
[[ "$(basename "$build_a")" == "$expected_name" ]]
[[ "$(basename "$build_b")" == "$expected_name" ]]
[[ "$(basename "$build_a")" == "$(basename "$build_b")" ]]

x86_job="$(awk '
  $0 == "  linux-x86_64:" { in_job = 1 }
  in_job && NR > 1 && $0 ~ /^  [a-z][a-z0-9-]*:/ && $0 != "  linux-x86_64:" { exit }
  in_job { print }
' "$release")"
grep -Fq 'OQUEUE_M13_EVIDENCE_DIR=target/repro scripts/release-repro.sh' <<<"$x86_job"

mutating="$scratch/mutating-build.sh"
sed 's/deterministic release artifact/deterministic release artifact ${OQUEUE_REPRO_BUILD_ID}/' \
  "$fake" > "$mutating"
chmod +x "$mutating"
if OQUEUE_REPRO_BUILD_SCRIPT="$mutating" OQUEUE_M13_EVIDENCE_DIR="$scratch/mismatch" \
    bash "$script" --completion-gate >/dev/null 2>&1; then
  printf '%s\n' 'release-repro accepted non-identical builds' >&2
  exit 1
fi

printf '%s\n' 'release-repro: ok'
