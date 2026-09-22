#!/usr/bin/env bash
# M13.13: the documented rolling upgrade and object-format refusal contract.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-upgrade.sh"
docs="$ROOT/docs/release/upgrade.md"
test -x "$script"
test -x "$ROOT/scripts/release-verify.sh"
test -f "$docs"
test -f "$ROOT/crates/oqueue-core/tests/release_upgrade.rs"

grep -Fq 'durable local state' "$docs"
grep -Fq 'readers before writers' "$docs"
grep -Fq 'unknown format' "$docs"
grep -Fq 'refuse' "$docs"
grep -Fq 'accepted a different image binary' "$ROOT/tests/release-image.sh"
grep -Fq 'accepted a mutated artifact' "$ROOT/tests/release-attest.sh"

bash "$script" --completion-gate
bash "$ROOT/tests/release-attest.sh" >/dev/null
bash "$ROOT/tests/release-image.sh" >/dev/null
printf '%s\n' 'release-upgrade: ok'
