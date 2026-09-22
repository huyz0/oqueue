#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASH_BIN="$(command -v bash)"
mkdir -p "$ROOT/target/tmp"
tmp="$(mktemp -d "$ROOT/target/tmp/release-fips-build-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

fake_bin="$tmp/bin"
mkdir -p "$fake_bin"
for tool in cmake go; do
  printf '%s\n' '#!/bin/sh' 'exit 0' > "$fake_bin/$tool"
  chmod +x "$fake_bin/$tool"
done
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s\\n" "$*" > "${FIPS_BUILD_CAPTURE:?}"' \
  > "$fake_bin/cargo"
chmod +x "$fake_bin/cargo"

capture="$tmp/command"
PATH="$fake_bin" FIPS_BUILD_CAPTURE="$capture" \
  "$BASH_BIN" "$ROOT/scripts/release-fips-build.sh"
actual="$(< "$capture")"
[[ "$actual" == 'build --locked --release --no-default-features --features fips -p oqueue' ]]

cargo_only="$tmp/cargo-only"
mkdir -p "$cargo_only"
cp "$fake_bin/cargo" "$cargo_only/cargo"
if PATH="$cargo_only" FIPS_BUILD_CAPTURE="$capture" \
  "$BASH_BIN" "$ROOT/scripts/release-fips-build.sh" > "$tmp/missing.log" 2>&1; then
  printf '%s\n' 'release-fips-build accepted an incomplete FIPS toolchain' >&2
  exit 1
fi

dockerfile="$ROOT/docker/release-fips.Dockerfile"
grep -Fq 'apt-get install -y --no-install-recommends build-essential cmake golang' "$dockerfile"
grep -Fqx 'RUN scripts/release-fips-build.sh' "$dockerfile"
grep -Fqx 'RUN cargo clippy --locked -p oqueue --no-default-features --features fips --all-targets -- -D warnings' "$dockerfile"
grep -Fqx 'RUN cargo clippy --locked -p oqueue-crypto --no-default-features --features fips --all-targets -- -D warnings' "$dockerfile"
grep -Fqx 'RUN cargo clippy --locked -p oqueue-store --no-default-features --features fips --all-targets -- -D warnings' "$dockerfile"
grep -Fqx 'RUN cargo clippy --locked -p oqueue-broker --no-default-features --features fips --all-targets -- -D warnings' "$dockerfile"
grep -Fqx 'RUN cargo test --locked -p oqueue-crypto --no-default-features --features fips --test it' "$dockerfile"
grep -Fqx 'RUN cargo test --locked -p oqueue-store --no-default-features --features fips --lib' "$dockerfile"
grep -Fqx 'RUN cargo test --locked -p oqueue-broker --no-default-features --features fips --lib' "$dockerfile"
grep -Fqx 'RUN cargo test --locked -p oqueue --no-default-features --features fips' "$dockerfile"
grep -Fqx 'RUN /workspace/target/release/oqueue' "$dockerfile"
grep -Fq 'oqueue_crypto::fips_mode_enabled()' "$ROOT/bin/oqueue/src/main.rs"
sed -n '/^fn main() {/,/^}$/p' "$ROOT/bin/oqueue/src/main.rs" > "$tmp/main.rs"
grep -Fq 'assert_fips_runtime();' "$tmp/main.rs"
grep -Fq 'bash tests/release-fips-build.sh' "$ROOT/.github/workflows/release.yml"
grep -Fq 'docker/release-fips.Dockerfile' "$ROOT/.github/workflows/release.yml"

printf '%s\n' 'release-fips-build: ok'
