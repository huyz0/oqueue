#!/usr/bin/env bash
# Build the default Linux release artifacts against the pinned glibc floor.
# M13.2 deliberately keeps this command separate from the native runner matrix:
# the runner supplies the architecture, while Zig supplies the older sysroot.
set -euo pipefail

readonly GLIBC_FLOOR=2.28
readonly CARGO_ZIGBUILD_VERSION=0.20.1
readonly ZIG_VERSION=0.14.1
readonly TARGETS=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)

selected_targets=("${TARGETS[@]}")
if [[ "$#" -gt 1 ]]; then
  printf 'usage: release-build.sh [target]\n' >&2
  exit 2
elif [[ "$#" -eq 1 ]]; then
  selected_targets=("$1")
elif [[ -n "${OQUEUE_RELEASE_TARGET:-}" ]]; then
  selected_targets=("$OQUEUE_RELEASE_TARGET")
fi

for target in "${selected_targets[@]}"; do
  case "$target" in
    x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
    *)
      printf 'release-build: unsupported target %s\n' "$target" >&2
      exit 2
      ;;
  esac
  target_with_floor="$target.$GLIBC_FLOOR"
  printf 'M13_RELEASE_COMMAND target=%s command=cargo zigbuild --locked --release --target %s\n' \
    "$target_with_floor" "$target_with_floor"
done

if [[ "${OQUEUE_RELEASE_PLAN_ONLY:-0}" == 1 ]]; then
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
if ! cargo_zigbuild_version="$(cargo zigbuild --version 2>/dev/null)" || \
  [[ "$cargo_zigbuild_version" != "cargo-zigbuild $CARGO_ZIGBUILD_VERSION" ]]; then
  printf 'release-build: expected cargo-zigbuild %s, got %s\n' \
    "$CARGO_ZIGBUILD_VERSION" "${cargo_zigbuild_version:-missing}" >&2
  exit 1
fi

for target in "${selected_targets[@]}"; do
  target_with_floor="$target.$GLIBC_FLOOR"
  cargo zigbuild --locked --release --target "$target_with_floor"
done
