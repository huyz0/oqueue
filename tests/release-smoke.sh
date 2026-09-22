#!/usr/bin/env bash
# M13.7: the oldest supported glibc image must build and exercise the artifact.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-smoke.sh"
dockerfile="$ROOT/docker/release-smoke.Dockerfile"
workflow="$ROOT/.github/workflows/release.yml"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

grep -Fq 'FROM rockylinux:8' "$dockerfile"
grep -Fq 'python39' "$dockerfile"
grep -Fq 'cargo build --locked --release -p oqueue --no-default-features --features software-aead,ring' "$dockerfile"
grep -Fq 'ENV OQUEUE_BIN=/work/target/release/oqueue' "$dockerfile"
grep -Fq 'ENTRYPOINT ["python3.9", "scripts/harness/role_smoke.py"]' "$dockerfile"
grep -Fq 'docker_exec build --pull --file docker/release-smoke.Dockerfile' "$script"
grep -Fq 'MSYS_NO_PATHCONV=1 docker' "$script"
grep -Fq 'docker_exec run --rm --entrypoint /usr/bin/ldd' "$script"
grep -Fq 'docker_exec run --rm "$IMAGE"' "$script"
grep -Fq 'docker_exec image inspect --format' "$script"
grep -Fq 'M13_RELEASE_IMAGE' "$script"
grep -Fq 'expected_arch=' "$script"
grep -Fq 'GNU libc.*2\.28' "$script"
grep -Fq 'M13_RELEASE_SMOKE status=pass glibc_floor=2.28 request=pass' "$script"
grep -Fq 'bash tests/release-smoke.sh' "$workflow"
grep -Fq 'scripts/release-smoke.sh' "$workflow"

test -x "$script"

stub_dir="$scratch/bin"
evidence="$scratch/evidence"
mkdir -p "$stub_dir" "$evidence"
stub="$stub_dir/docker"
stub_lines=(
  '#!/usr/bin/env bash'
  'set -euo pipefail'
  'printf "%s\\n" "$*" >> "${STUB_LOG:?}"'
  'case "${1:-}" in'
  '  build) exit 0 ;;'
  '  image) printf "%s\\n" "sha256:fixture ${STUB_ARCH:?} linux" ;;'
  '  run)'
  '    if [[ "${STUB_FAIL_RUNTIME:-0}" == 1 && "$*" != *"/usr/bin/ldd"* ]]; then exit 17; fi'
  '    if [[ "$*" == *"/usr/bin/ldd"* ]]; then printf "%s\\n" "ldd (GNU libc) 2.28"; else printf "%s\\n" "ok role combined: request passed"; fi'
  '    ;;'
  '  *) exit 64 ;;'
  'esac'
)
printf '%s\n' "${stub_lines[@]}" > "$stub"
chmod +x "$stub"
case "$(uname -m)" in
  x86_64) stub_arch=amd64 ;;
  aarch64|arm64) stub_arch=arm64 ;;
  *) printf '%s\n' 'unsupported test architecture' >&2; exit 1 ;;
esac
STUB_ARCH="$stub_arch" STUB_LOG="$scratch/docker.log" PATH="$stub_dir:$PATH" \
  OQUEUE_M13_EVIDENCE_DIR="$evidence" OQUEUE_RELEASE_SMOKE_IMAGE=fixture \
  bash "$script" >/dev/null
grep -Fq 'build --pull --file docker/release-smoke.Dockerfile' "$scratch/docker.log"
grep -Fq 'image inspect --format' "$scratch/docker.log"
grep -Fq 'run --rm --entrypoint /usr/bin/ldd' "$scratch/docker.log"
grep -Fq 'run --rm fixture' "$scratch/docker.log"
test -f "$evidence/image.tsv"
test -f "$evidence/glibc.txt"
test -f "$evidence/role-smoke.log"
grep -Fxq 'M13_RELEASE_SMOKE status=pass glibc_floor=2.28 request=pass' "$evidence/smoke.tsv"

failed_evidence="$scratch/failed-evidence"
if STUB_ARCH="$stub_arch" STUB_LOG="$scratch/failing-docker.log" PATH="$stub_dir:$PATH" \
    STUB_FAIL_RUNTIME=1 OQUEUE_M13_EVIDENCE_DIR="$failed_evidence" \
    OQUEUE_RELEASE_SMOKE_IMAGE=fixture bash "$script" >/dev/null 2>&1; then
  printf '%s\n' 'release-smoke contract: runtime failure was swallowed' >&2
  exit 1
fi
test ! -f "$failed_evidence/smoke.tsv"

missing_gate_evidence="$scratch/missing-gate-evidence"
if OQUEUE_M13_EVIDENCE_DIR="$missing_gate_evidence" bash "$script" smoke.tsv --completion-gate \
    >/dev/null 2>&1; then
  printf '%s\n' 'release-smoke completion gate ran without native evidence' >&2
  exit 1
fi
test ! -e "$missing_gate_evidence/smoke.tsv"

native_evidence="$scratch/native-smoke.tsv"
printf '%s\n' 'M13_RELEASE_SMOKE status=pass glibc_floor=2.28 request=pass' > "$native_evidence"
named_evidence="$scratch/named-evidence"
OQUEUE_M13_SMOKE_EVIDENCE="$native_evidence" OQUEUE_M13_EVIDENCE_DIR="$named_evidence" \
  bash "$script" smoke.tsv --completion-gate >/dev/null
cmp -s "$native_evidence" "$named_evidence/smoke.tsv"

printf '%s\n' 'release-smoke: ok'
