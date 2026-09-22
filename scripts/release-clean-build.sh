#!/usr/bin/env bash
# Build the default release in the minimal M13.4 builder.
set -euo pipefail

for forbidden in cmake go; do
  if command -v "$forbidden" >/dev/null 2>&1; then
    printf 'release-clean-build: %s must be absent from the default builder\n' "$forbidden" >&2
    exit 1
  fi
done

exec cargo build --locked --release -p oqueue --no-default-features \
  --features software-aead,ring
