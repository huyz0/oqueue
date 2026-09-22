#!/usr/bin/env bash
# Build the isolated FIPS release in the M13.5 builder.
set -euo pipefail

for required in cmake go; do
  if ! command -v "$required" >/dev/null 2>&1; then
    printf 'release-fips-build: %s is required by the FIPS provider\n' "$required" >&2
    exit 1
  fi
done

exec cargo build --locked --release --no-default-features --features fips -p oqueue
