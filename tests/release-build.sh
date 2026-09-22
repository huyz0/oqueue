#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
mkdir -p "$ROOT/target/tmp"
tmp="$(mktemp -d "$ROOT/target/tmp/release-build-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

plan="$(OQUEUE_RELEASE_PLAN_ONLY=1 "$ROOT/scripts/release-build.sh")"

expected_x86='M13_RELEASE_COMMAND target=x86_64-unknown-linux-gnu.2.28 rustflags=-C target-cpu=x86-64-v2 command=cargo zigbuild --locked --release -p oqueue --no-default-features --features software-aead,ring --target x86_64-unknown-linux-gnu.2.28'
expected_arm='M13_RELEASE_COMMAND target=aarch64-unknown-linux-gnu.2.28 rustflags=-C target-feature=+lse,+crc command=cargo zigbuild --locked --release -p oqueue --no-default-features --features software-aead,ring --target aarch64-unknown-linux-gnu.2.28'

grep -Fqx "$expected_x86" <<< "$plan"
grep -Fqx "$expected_arm" <<< "$plan"

fake_bin="$tmp/bin"
mkdir -p "$fake_bin"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "${1:-}" == version ]]; then printf "%s\n" "${FAKE_ZIG_VERSION:-0.14.1}"; else exit 2; fi' \
  > "$fake_bin/zig"
printf '%s\n' \
  '#!/usr/bin/env bash' \
  'if [[ "$*" == "zigbuild --version" ]]; then printf "%s\n" "${FAKE_CARGO_ZIGBUILD_VERSION:-cargo-zigbuild 0.20.1}"; exit 0; fi' \
  'if [[ "${1:-}" == zigbuild ]]; then printf "%s|%s\n" "${RUSTFLAGS:-}" "$*" > "${RELEASE_BUILD_CAPTURE:?}"; exit 0; fi' \
  'exit 2' \
  > "$fake_bin/cargo"
chmod +x "$fake_bin/zig" "$fake_bin/cargo"

capture="$tmp/command"
PATH="$fake_bin:$PATH" RELEASE_BUILD_CAPTURE="$capture" \
  OQUEUE_RELEASE_TARGET=x86_64-unknown-linux-gnu \
  "$ROOT/scripts/release-build.sh"
actual="$(< "$capture")"
expected_actual='--cfg tokio_unstable -C target-cpu=x86-64-v2|zigbuild --locked --release -p oqueue --no-default-features --features software-aead,ring --target x86_64-unknown-linux-gnu.2.28'
[[ "$actual" == "$expected_actual" ]]

capture="$tmp/arm-command"
PATH="$fake_bin:$PATH" RELEASE_BUILD_CAPTURE="$capture" \
  OQUEUE_RELEASE_TARGET=aarch64-unknown-linux-gnu \
  "$ROOT/scripts/release-build.sh"
actual="$(< "$capture")"
expected_actual='--cfg tokio_unstable -C target-feature=+lse,+crc|zigbuild --locked --release -p oqueue --no-default-features --features software-aead,ring --target aarch64-unknown-linux-gnu.2.28'
[[ "$actual" == "$expected_actual" ]]

if PATH="$fake_bin:$PATH" FAKE_ZIG_VERSION=0.14.0 \
  OQUEUE_RELEASE_TARGET=x86_64-unknown-linux-gnu \
  "$ROOT/scripts/release-build.sh" > "$tmp/wrong-zig.log" 2>&1; then
  printf '%s\n' 'release-build accepted an unexpected Zig version' >&2
  exit 1
fi
if PATH="$fake_bin:$PATH" FAKE_CARGO_ZIGBUILD_VERSION='cargo-zigbuild 0.20.0' \
  OQUEUE_RELEASE_TARGET=x86_64-unknown-linux-gnu \
  "$ROOT/scripts/release-build.sh" > "$tmp/wrong-cargo-zigbuild.log" 2>&1; then
  printf '%s\n' 'release-build accepted an unexpected cargo-zigbuild version' >&2
  exit 1
fi
printf '%s\n' 'release-build planner: ok'
