#!/usr/bin/env bash
# Verify the release workflow's Linux architecture and macOS fast-test matrix.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-workflow.$$}"
mkdir -p "$EVIDENCE_DIR"

if [[ "$#" -gt 1 || ( "$#" -eq 1 && "$1" != --completion-gate ) ]]; then
  printf '%s\n' 'usage: check-release-workflow.sh [--completion-gate]' >&2
  exit 2
fi

OQUEUE_RELEASE_MATRIX_NO_CHECKER=1 bash "$ROOT/tests/release-matrix.sh" >/dev/null
release="$ROOT/.github/workflows/release.yml"
os_smoke="$ROOT/.github/workflows/os-smoke.yml"
grep -Fq 'target/${{ env.RELEASE_TARGET }}.2.28/release/oqueue' "$release"
grep -Fq 'docker build --file docker/release-default.Dockerfile' "$release"
grep -Fq 'docker build --file docker/release-fips.Dockerfile' "$release"
grep -Fq 'run: scripts/release-smoke.sh' "$release"
grep -Fq 'os: [ubuntu-latest, macos-latest]' "$os_smoke"
grep -Fq 'os: [ubuntu-latest, macos-latest, windows-latest]' "$os_smoke"
if grep -Eq '^  [a-z0-9-]*macos[a-z0-9-]*:' "$release" || \
   grep -Fq 'runs-on: macos-latest' "$release" || \
   grep -Fq 'runs-on: ${{ matrix.' "$release"; then
  printf '%s\n' 'check-release-workflow: macOS must not be a release target' >&2
  exit 1
fi

printf '%s\n' \
  'M13_RELEASE_WORKFLOW status=pass matrix_linux_x86_64=pass matrix_linux_aarch64=pass macos_fast=pass glibc_floor=2.28 fips_job=pass' \
  > "$EVIDENCE_DIR/workflow.tsv"
printf '%s\n' "$(<"$EVIDENCE_DIR/workflow.tsv")"
