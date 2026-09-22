#!/usr/bin/env bash
# Compare two isolated builds of one Linux release artifact.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-repro.$$}"
BUILD_SCRIPT="${OQUEUE_REPRO_BUILD_SCRIPT:-$ROOT/scripts/release-build.sh}"
TARGET="${OQUEUE_RELEASE_TARGET:-x86_64-unknown-linux-gnu}"
mkdir -p "$EVIDENCE_DIR"

if [[ "$#" -gt 1 || ( "$#" -eq 1 && "$1" != --completion-gate ) ]]; then
  printf '%s\n' 'usage: release-repro.sh [--completion-gate]' >&2
  exit 2
fi
case "$TARGET" in
  x86_64-unknown-linux-gnu) ARCH=x86_64 ;;
  aarch64-unknown-linux-gnu) ARCH=aarch64 ;;
  *) printf 'release-repro: unsupported target %s\n' "$TARGET" >&2; exit 1 ;;
esac
[[ -x "$BUILD_SCRIPT" ]] || {
  printf 'release-repro: build script is not executable: %s\n' "$BUILD_SCRIPT" >&2
  exit 1
}

hash_file() {
  local path="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$path" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$path" | awk '{print $1}'
  else
    printf '%s\n' 'release-repro: need sha256sum or shasum' >&2
    exit 1
  fi
}

hash_text() {
  local text="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    printf '%s' "$text" | sha256sum | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    printf '%s' "$text" | shasum -a 256 | awk '{print $1}'
  else
    printf '%s\n' 'release-repro: need sha256sum or shasum' >&2
    exit 1
  fi
}

run_id="$(git -C "$ROOT" rev-parse HEAD)"
[[ "$run_id" =~ ^[0-9a-f]{40}$ ]] || {
  printf 'release-repro: expected a full commit id, got %s\n' "$run_id" >&2
  exit 1
}
: > "$EVIDENCE_DIR/commands.log"

for pass in a b; do
  build_dir="$EVIDENCE_DIR/build-$pass"
  log="$EVIDENCE_DIR/build-$pass.log"
  mkdir -p "$build_dir"
  printf -v command_line \
    'env CARGO_TARGET_DIR=%q OQUEUE_RELEASE_TARGET=%q OQUEUE_REPRO_BUILD_ID=%q bash %q' \
    "$build_dir" "$TARGET" "$pass" "$BUILD_SCRIPT"
  printf '%s\n' "$command_line" >> "$EVIDENCE_DIR/commands.log"
  if ! env CARGO_TARGET_DIR="$build_dir" OQUEUE_RELEASE_TARGET="$TARGET" \
      OQUEUE_REPRO_BUILD_ID="$pass" bash "$BUILD_SCRIPT" > "$log" 2>&1; then
    cat "$log" >&2
    exit 1
  fi
  # cargo-zigbuild uses the Cargo target triple as the output directory;
  # the glibc floor is a linker/build constraint, not part of that directory.
  built_artifact="$build_dir/$TARGET/release/oqueue"
  [[ -f "$built_artifact" ]] || {
    printf 'release-repro: build %s did not produce %s\n' "$pass" "$built_artifact" >&2
    exit 1
  }
  artifact_name="$(bash "$ROOT/scripts/release-artifact-name.sh" linux "$ARCH" glibc2.28 default bin)"
  canonical_artifact="$build_dir/$artifact_name"
  cp "$built_artifact" "$canonical_artifact"
  printf '%s\n' "$canonical_artifact" > "$EVIDENCE_DIR/build-$pass.path"
done

commands_sha256="$(hash_file "$EVIDENCE_DIR/commands.log")"
artifact_a="$(<"$EVIDENCE_DIR/build-a.path")"
artifact_b="$(<"$EVIDENCE_DIR/build-b.path")"
sha_a="$(hash_file "$artifact_a")"
sha_b="$(hash_file "$artifact_b")"
outputs_sha256="$(cat "$EVIDENCE_DIR/build-a.log" "$EVIDENCE_DIR/build-b.log" | \
  if command -v sha256sum >/dev/null 2>&1; then sha256sum | awk '{print $1}'; else shasum -a 256 | awk '{print $1}'; fi)"

if [[ "$sha_a" != "$sha_b" ]]; then
  printf 'release-repro: build bytes differ (%s != %s)\n' "$sha_a" "$sha_b" >&2
  exit 1
fi

printf '%s\n' \
  "M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=$sha_a mode=byte-identical release_build=a build_a=$artifact_a sha256_a=$sha_a build_b=$artifact_b sha256_b=$sha_b comparison=equal commands_sha256=$commands_sha256 outputs_sha256=$outputs_sha256" \
  > "$EVIDENCE_DIR/repro.tsv"
printf '%s\n' "$(<"$EVIDENCE_DIR/repro.tsv")"
