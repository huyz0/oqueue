#!/usr/bin/env bash
# Verify the documented rolling-upgrade and object-format compatibility contract.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-upgrade.$$}"
mkdir -p "$EVIDENCE_DIR"

while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --completion-gate) shift ;;
    *) printf '%s\n' 'usage: release-upgrade.sh [--completion-gate]' >&2; exit 2 ;;
  esac
done

docs="$ROOT/docs/release/upgrade.md"
[[ -f "$docs" ]] || { printf 'release-upgrade: missing %s\n' "$docs" >&2; exit 1; }
for requirement in 'durable local state' 'readers before writers' 'unknown format' 'refuse'; do
  grep -Fq "$requirement" "$docs" || {
    printf 'release-upgrade: documentation omits required contract: %s\n' "$requirement" >&2
    exit 1
  }
done

if [[ -f /.dockerenv || -n "${CI:-}" ]]; then
  cargo test --locked -p oqueue-core --test release_upgrade --quiet
else
  bash "$ROOT/scripts/docker-test.sh" cargo test --locked -p oqueue-core --test release_upgrade --quiet
fi
bash "$ROOT/tests/release-attest.sh" >/dev/null
bash "$ROOT/tests/release-image.sh" >/dev/null

printf '%s\n' \
  'M13_RELEASE_UPGRADE status=pass durable_local_state=none unknown_bundle=refuse unknown_composite=refuse unknown_partition_manifest=refuse rolling_order=readers-before-writers incompatible=refuse' \
  > "$EVIDENCE_DIR/upgrade.tsv"
printf '%s\n' "$(<"$EVIDENCE_DIR/upgrade.tsv")"
