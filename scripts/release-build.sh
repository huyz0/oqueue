#!/usr/bin/env bash
# Build the default Linux release artifacts against the pinned glibc floor.
# M13.2 deliberately keeps this command separate from the native runner matrix:
# the runner supplies the architecture, while Zig supplies the older sysroot.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly GLIBC_FLOOR=2.28
readonly CARGO_ZIGBUILD_VERSION=0.20.1
readonly ZIG_VERSION=0.14.1
readonly TARGETS=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)
release_features="${OQUEUE_RELEASE_FEATURES:-software-aead,ring}"
case "$release_features" in
  software-aead,ring|fips) ;;
  *) printf 'release-build: unsupported feature set %s\n' "$release_features" >&2; exit 2 ;;
esac

selected_targets=("${TARGETS[@]}")
completion_gate=0
output_name=build.tsv
positional=()
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --completion-gate) completion_gate=1; shift ;;
    build.tsv) output_name=build.tsv; shift ;;
    *) positional+=("$1"); shift ;;
  esac
done
if [[ "${#positional[@]}" -gt 1 ]]; then
  printf 'usage: release-build.sh [target|build.tsv] [--completion-gate]\n' >&2
  exit 2
elif [[ "${#positional[@]}" -eq 1 ]]; then
  selected_targets=("${positional[0]}")
elif [[ -n "${OQUEUE_RELEASE_TARGET:-}" ]]; then
  selected_targets=("$OQUEUE_RELEASE_TARGET")
fi

EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-build.$$}"

for target in "${selected_targets[@]}"; do
  case "$target" in
    x86_64-unknown-linux-gnu) rustflags='-C target-cpu=x86-64-v2' ;;
    aarch64-unknown-linux-gnu) rustflags='-C target-feature=+lse,+crc' ;;
    *)
      printf 'release-build: unsupported target %s\n' "$target" >&2
      exit 2
      ;;
  esac
  target_with_floor="$target.$GLIBC_FLOOR"
  printf 'M13_RELEASE_COMMAND target=%s rustflags=%s command=cargo zigbuild --locked --release -p oqueue --no-default-features --features %s --target %s\n' \
    "$target_with_floor" "$rustflags" "$release_features" "$target_with_floor"
done

if [[ "${OQUEUE_RELEASE_PLAN_ONLY:-0}" == 1 ]]; then
  exit 0
fi

if [[ "$completion_gate" == 1 ]]; then
  source_evidence="${OQUEUE_M13_BUILD_EVIDENCE:-}"
  [[ -n "$source_evidence" && -f "$source_evidence" ]] || {
    printf '%s\n' 'release-build: completion gate requires native release evidence via OQUEUE_M13_BUILD_EVIDENCE' >&2
    exit 1
  }
  [[ "$(grep -Ec '^M13_RELEASE_BUILD ' "$source_evidence" || true)" == 1 ]] || {
    printf '%s\n' 'release-build: native build evidence must contain exactly one release-build record' >&2
    exit 1
  }
  mkdir -p "$EVIDENCE_DIR"
  destination="$EVIDENCE_DIR/$output_name"
  if [[ "$source_evidence" != "$destination" ]]; then
    cp "$source_evidence" "$destination"
  fi
  cat "$destination"
  exit 0
fi

if ! command -v cargo >/dev/null 2>&1; then
  printf 'release-build: cargo is required\n' >&2
  exit 1
fi
if ! command -v zig >/dev/null 2>&1; then
  printf 'release-build: Zig %s is required\n' "$ZIG_VERSION" >&2
  exit 1
fi
if ! zig_version="$(zig version)" || [[ "$zig_version" != "$ZIG_VERSION" ]]; then
  printf 'release-build: expected Zig %s, got %s\n' "$ZIG_VERSION" "${zig_version:-missing}" >&2
  exit 1
fi
if command -v cargo-zigbuild >/dev/null 2>&1; then
  cargo_zigbuild_version="$(cargo-zigbuild --version 2>/dev/null)"
else
  cargo_zigbuild_version="$(cargo zigbuild --version 2>/dev/null)"
fi
if [[ -z "${cargo_zigbuild_version:-}" ]] || \
  [[ "$cargo_zigbuild_version" != "cargo-zigbuild $CARGO_ZIGBUILD_VERSION" ]]; then
  printf 'release-build: expected cargo-zigbuild %s, got %s\n' \
    "$CARGO_ZIGBUILD_VERSION" "${cargo_zigbuild_version:-missing}" >&2
  exit 1
fi

for target in "${selected_targets[@]}"; do
  case "$target" in
    x86_64-unknown-linux-gnu) rustflags='-C target-cpu=x86-64-v2' ;;
    aarch64-unknown-linux-gnu) rustflags='-C target-feature=+lse,+crc' ;;
  esac
  target_with_floor="$target.$GLIBC_FLOOR"
  RUSTFLAGS="--cfg tokio_unstable $rustflags" cargo zigbuild --locked --release \
    -p oqueue --no-default-features --features "$release_features" --target "$target_with_floor"
done
