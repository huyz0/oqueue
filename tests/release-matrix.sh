#!/usr/bin/env bash
# M13.8: release CI tests both Linux architectures and the macOS fast tier.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
release="$ROOT/.github/workflows/release.yml"
os_smoke="$ROOT/.github/workflows/os-smoke.yml"
checker="$ROOT/scripts/check-release-workflow.sh"

job_block() {
  local workflow="$1" job="$2"
  awk -v wanted="  $job:" '
    $0 == wanted { in_job = 1 }
    in_job && NR > 1 && $0 ~ /^  [a-z][a-z0-9-]*:/ && $0 != wanted { exit }
    in_job { print }
  ' "$workflow"
}

x86="$(job_block "$release" linux-x86_64)"
arm="$(job_block "$release" linux-aarch64)"
smoke="$(job_block "$os_smoke" smoke)"
fast="$(job_block "$os_smoke" fast)"
aggregate="$(job_block "$release" aggregate)"

grep -Fq 'runs-on: ubuntu-latest' <<<"$x86"
grep -Fq 'cargo test --locked --workspace' <<<"$x86"
grep -Fq 'runs-on: ubuntu-24.04-arm' <<<"$arm"
grep -Fq 'cargo test --locked --workspace' <<<"$arm"
for block in "$x86" "$arm"; do
  [[ "$(grep -Fc 'target/${RELEASE_TARGET}/release/oqueue' <<<"$block")" -eq 4 ]]
  if grep -Fq 'target/${RELEASE_TARGET}.2.28/release/oqueue' <<<"$block"; then
    printf '%s\n' 'release jobs must use Cargo target directories without the glibc suffix' >&2
    exit 1
  fi
done
grep -Fq 'os: [ubuntu-latest, macos-latest, windows-latest]' <<<"$smoke"
grep -Fq 'bash scripts/os-smoke.sh' <<<"$smoke"
if grep -Fq 'cargo ' <<<"$smoke"; then
  printf '%s\n' 'Windows smoke job must not run the native Cargo tier' >&2
  exit 1
fi
grep -Fq 'os: [ubuntu-latest, macos-latest]' <<<"$fast"
grep -Fq 'rustup show active-toolchain' <<<"$fast"
grep -Fq 'rustup toolchain install "$toolchain" --profile minimal' <<<"$fast"
grep -Fq 'cargo test --locked --workspace' <<<"$fast"
grep -Fq 'bash scripts/os-smoke.sh' <<<"$fast"

gate_line="$(awk '/scripts\/docker-test\.sh scripts\/gates\/m13-complete\.sh/{print NR; exit}' <<<"$aggregate")"
upload_line="$(awk '/uses: actions\/upload-artifact@v4/{print NR; exit}' <<<"$aggregate")"
[[ -n "$gate_line" && -n "$upload_line" && "$upload_line" -gt "$gate_line" ]]
grep -Fq 'name: m13-release-evidence' <<<"$aggregate"
grep -Fq 'path: target/m13-evidence' <<<"$aggregate"
grep -Fq 'if-no-files-found: error' <<<"$aggregate"
grep -Fq 'retention-days: 90' <<<"$aggregate"

for block in "$x86" "$arm" "$fast"; do
  toolchain_line="$(awk '/rustup show active-toolchain/{print NR; exit}' <<<"$block")"
  test_line="$(awk '/cargo test --locked --workspace/{print NR; exit}' <<<"$block")"
  [[ -n "$toolchain_line" && -n "$test_line" && "$test_line" -gt "$toolchain_line" ]]
done

if grep -Eq '^  [a-z0-9-]*macos[a-z0-9-]*:' "$release" || \
   grep -Fq 'runs-on: macos-latest' "$release" || \
   grep -Fq 'runs-on: ${{ matrix.' "$release"; then
  printf '%s\n' 'release workflow must not produce a macOS release artifact' >&2
  exit 1
fi

if [[ "${OQUEUE_RELEASE_MATRIX_NO_CHECKER:-0}" != 1 ]]; then
  test -x "$checker"
  checker_evidence="$ROOT/target/tmp/release-matrix.$$"
  mkdir -p "$checker_evidence"
  trap 'rm -rf "$checker_evidence"' EXIT
  OQUEUE_M13_EVIDENCE_DIR="$checker_evidence" bash "$checker"
  grep -Fxq \
    'M13_RELEASE_WORKFLOW status=pass matrix_linux_x86_64=pass matrix_linux_aarch64=pass macos_fast=pass glibc_floor=2.28 fips_job=pass' \
    "$checker_evidence/workflow.tsv"
fi

printf '%s\n' 'release-matrix: ok'
