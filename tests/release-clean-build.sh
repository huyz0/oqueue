#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BASH_BIN="$(command -v bash)"
mkdir -p "$ROOT/target/tmp"
tmp="$(mktemp -d "$ROOT/target/tmp/release-clean-build-test.XXXXXX")"
trap 'rm -rf "$tmp"' EXIT

fake_bin="$tmp/bin"
mkdir -p "$fake_bin"
printf '%s\n' \
  '#!/bin/sh' \
  'printf "%s\\n" "$*" > "${CLEAN_BUILD_CAPTURE:?}"' \
  > "$fake_bin/cargo"
chmod +x "$fake_bin/cargo"

capture="$tmp/command"
PATH="$fake_bin" CLEAN_BUILD_CAPTURE="$capture" \
  "$BASH_BIN" "$ROOT/scripts/release-clean-build.sh"
actual="$(< "$capture")"
[[ "$actual" == 'build --locked --release -p oqueue --no-default-features --features software-aead,ring' ]]

forbidden_bin="$tmp/forbidden-bin"
mkdir -p "$forbidden_bin"
for forbidden in cmake go; do
  printf '%s\n' '#!/bin/sh' 'exit 0' > "$forbidden_bin/$forbidden"
  chmod +x "$forbidden_bin/$forbidden"
  if PATH="$forbidden_bin:$fake_bin" CLEAN_BUILD_CAPTURE="$capture" \
    "$BASH_BIN" "$ROOT/scripts/release-clean-build.sh" > "$tmp/$forbidden.log" 2>&1; then
    printf 'release-clean-build accepted forbidden tool %s\n' "$forbidden" >&2
    exit 1
  fi
done

dockerfile="$ROOT/docker/release-default.Dockerfile"
if grep -Fq 'apt-get install -y --no-install-recommends cmake' "$dockerfile" || \
  grep -Fq 'apt-get install -y --no-install-recommends golang' "$dockerfile"; then
  printf '%s\n' 'release-default.Dockerfile installs forbidden FIPS tooling' >&2
  exit 1
fi
grep -Fqx 'RUN scripts/release-clean-build.sh' "$dockerfile"
grep -Fq 'bash tests/release-clean-build.sh' "$ROOT/.github/workflows/release.yml"

printf '%s\n' 'release-clean-build: ok'
