#!/usr/bin/env bash
# M13.14 contract: aggregate only real native release-job outputs.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-aggregate.sh"
test -x "$script"

scratch="$ROOT/target/tmp/release-aggregate-test.$$"
mkdir -p "$scratch/artifacts"
trap 'rm -rf "$scratch"' EXIT

run_id="$(git -C "$ROOT" rev-parse HEAD)"
hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}
declare -a names=(
  "$(bash "$ROOT/scripts/release-artifact-name.sh" linux x86_64 glibc2.28 default bin)"
  "$(bash "$ROOT/scripts/release-artifact-name.sh" linux x86_64 glibc2.28 fips bin)"
  "$(bash "$ROOT/scripts/release-artifact-name.sh" linux aarch64 glibc2.28 default bin)"
  "$(bash "$ROOT/scripts/release-artifact-name.sh" linux aarch64 glibc2.28 fips bin)"
)
declare -a hashes=()
for name in "${names[@]}"; do
  printf '%s\n' "$name" > "$scratch/artifacts/$name"
  hashes+=("$(hash_file "$scratch/artifacts/$name")")
done
mkdir -p \
  "$scratch/repro/build-a/x86_64-unknown-linux-gnu.2.28/release" \
  "$scratch/repro/build-b/x86_64-unknown-linux-gnu.2.28/release"
cp "$scratch/artifacts/${names[0]}" \
  "$scratch/repro/build-a/x86_64-unknown-linux-gnu.2.28/release/oqueue"
cp "$scratch/artifacts/${names[0]}" \
  "$scratch/repro/build-b/x86_64-unknown-linux-gnu.2.28/release/oqueue"
printf '%s\n' \
  "M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=${hashes[0]} mode=byte-identical release_build=a build_a=$scratch/repro/build-a/x86_64-unknown-linux-gnu.2.28/release/oqueue sha256_a=${hashes[0]} build_b=$scratch/repro/build-b/x86_64-unknown-linux-gnu.2.28/release/oqueue sha256_b=${hashes[0]} comparison=equal commands_sha256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa outputs_sha256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" \
  > "$scratch/repro/repro.tsv"
printf '%s\n' \
  "M13_RELEASE_PROVENANCE status=pass run_id=$run_id native_x86=pass native_aarch64=pass clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass" \
  > "$scratch/provenance.tsv"

bash "$script" --input "$scratch" --output "$scratch" >/dev/null
expected="M13_RELEASE_BUILD status=pass run_id=$run_id default_x86_sha256=${hashes[0]} fips_x86_sha256=${hashes[1]} default_aarch64_sha256=${hashes[2]} fips_aarch64_sha256=${hashes[3]} clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass"
grep -Fqx "$expected" "$scratch/build.tsv"
grep -Fq 'build_a=repro/build-a/x86_64-unknown-linux-gnu.2.28/release/oqueue' "$scratch/repro.tsv"
grep -Fq "sha256_b=${hashes[0]}" "$scratch/repro.tsv"

sed 's/native_aarch64=pass/native_aarch64=fail/' "$scratch/provenance.tsv" > "$scratch/bad-provenance.tsv"
if bash "$script" --input "$scratch" --provenance "$scratch/bad-provenance.tsv" --output "$scratch/bad" >/dev/null 2>&1; then
  printf '%s\n' 'release-aggregate accepted a non-native ARM provenance record' >&2
  exit 1
fi

# Exercise the handoff boundary end to end with a tiny isolated gate. The
# stubs stand in for the expensive release legs; the real gate still parses
# every record and proves that a mounted bundle is consumed, while an absent
# bundle cannot silently trigger local fabrication.
gate_root="$scratch/gate-repo"
mkdir -p "$gate_root/scripts/gates" "$gate_root/docs/release" \
  "$gate_root/.github/workflows" "$gate_root/target/tmp"
cp "$ROOT/scripts/lib.sh" "$gate_root/scripts/lib.sh"
cp "$ROOT/scripts/gates/m13-complete.sh" "$gate_root/scripts/gates/m13-complete.sh"
printf '%s\n' policy > "$gate_root/docs/release/artifacts.md"
printf '%s\n' workflow > "$gate_root/.github/workflows/release.yml"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$gate_root/scripts/check-milestone-review.sh"
printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
  'mkdir -p "$OQUEUE_M13_EVIDENCE_DIR"' \
  'printf "%s\\n" "M13_RELEASE_WORKFLOW status=pass matrix_linux_x86_64=pass matrix_linux_aarch64=pass macos_fast=pass glibc_floor=2.28 fips_job=pass" > "$OQUEUE_M13_EVIDENCE_DIR/workflow.tsv"' \
  > "$gate_root/scripts/check-release-workflow.sh"
printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
  '[[ -f "${OQUEUE_M13_BUILD_EVIDENCE:-}" ]]' \
  'mkdir -p "$OQUEUE_M13_EVIDENCE_DIR"' \
  'cp "$OQUEUE_M13_BUILD_EVIDENCE" "$OQUEUE_M13_EVIDENCE_DIR/build.tsv"' \
  > "$gate_root/scripts/release-build.sh"
printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
  '[[ -f "${OQUEUE_M13_SMOKE_EVIDENCE:-}" ]]' \
  'mkdir -p "$OQUEUE_M13_EVIDENCE_DIR"' \
  'cp "$OQUEUE_M13_SMOKE_EVIDENCE" "$OQUEUE_M13_EVIDENCE_DIR/smoke.tsv"' \
  > "$gate_root/scripts/release-smoke.sh"
printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
  '[[ -f "${OQUEUE_M13_EVIDENCE_INPUT_DIR:-}/verify.tsv" ]]' \
  'mkdir -p "$OQUEUE_M13_EVIDENCE_DIR"' \
  'cp "$OQUEUE_M13_EVIDENCE_INPUT_DIR/verify.tsv" "$OQUEUE_M13_EVIDENCE_DIR/verify.tsv"' \
  > "$gate_root/scripts/release-verify.sh"
printf '%s\n' '#!/usr/bin/env bash' 'exit 0' > "$gate_root/scripts/release-aggregate.sh"
chmod +x "$gate_root/scripts"/*.sh "$gate_root/scripts/gates/m13-complete.sh"

bundle="$gate_root/target/m13-evidence"
mkdir -p "$bundle/artifacts"
run_id=0123456789abcdef0123456789abcdef01234567
hash_a=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
hash_b=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
hash_c=cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
hash_d=dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd
printf '%s\n' "M13_RELEASE_BUILD status=pass run_id=$run_id default_x86_sha256=$hash_a fips_x86_sha256=$hash_b default_aarch64_sha256=$hash_c fips_aarch64_sha256=$hash_d clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass" > "$bundle/build.tsv"
printf '%s\n' 'M13_RELEASE_SMOKE status=pass glibc_floor=2.28 request=pass' > "$bundle/smoke.tsv"
printf '%s\n' \
  "M13_RELEASE_VERIFY status=pass run_id=$run_id naming=pass musl=pass checksums=pass signatures=pass images=pass upgrade=pass" \
  "M13_ARTIFACT default run_id=$run_id name=oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin sha256=$hash_a" \
  "M13_ARTIFACT fips run_id=$run_id name=oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin sha256=$hash_b" \
  "M13_ARTIFACT default run_id=$run_id name=oqueue-0.0.0-linux-aarch64-glibc2.28-default-bin sha256=$hash_c" \
  "M13_ARTIFACT fips run_id=$run_id name=oqueue-0.0.0-linux-aarch64-glibc2.28-fips-bin sha256=$hash_d" \
  "M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=$hash_a mode=byte-identical release_build=a build_a=a sha256_a=$hash_a build_b=b sha256_b=$hash_a comparison=equal commands_sha256=$hash_a outputs_sha256=$hash_a" \
  > "$bundle/verify.tsv"

if (cd "$gate_root" && bash scripts/gates/m13-complete.sh >/dev/null 2>&1); then
  printf '%s\n' 'm13 completion gate passed without a native evidence bundle' >&2
  exit 1
fi
if ! (cd "$gate_root" && OQUEUE_M13_EVIDENCE_INPUT_DIR="$bundle" bash scripts/gates/m13-complete.sh >/dev/null); then
  printf '%s\n' 'm13 completion gate rejected a mounted native evidence bundle' >&2
  exit 1
fi

# Run the actual containment wrapper with a stub Docker CLI and inspect the
# final argv. This catches a wrong host path, missing read-only flag, or a
# missing container-side environment handoff that a source grep cannot catch.
docker_stub_dir="$scratch/docker-bin"
docker_capture="$scratch/docker-argv"
mkdir -p "$docker_stub_dir"
printf '%s\n' '#!/usr/bin/env bash' 'set -euo pipefail' \
  'case "${1:-}" in' \
  '  run) if [[ "$*" == *"--memory"* ]]; then printf "%s\\n" "$*" > "${DOCKER_CAPTURE:?}"; fi; exit 0 ;;' \
  '  image|images|volume) ;;' \
  '  *) exit 64 ;;' \
  'esac' \
  > "$docker_stub_dir/docker"
chmod +x "$docker_stub_dir/docker"
mounted="$scratch/mounted-evidence"
mkdir -p "$mounted"
wrapper_log="$scratch/docker-test.log"
if ! (OQUEUE_M13_EVIDENCE_INPUT_DIR="$mounted" IMAGE=fixture \
    DOCKER_CAPTURE="$docker_capture" PATH="$docker_stub_dir:$PATH" \
    bash "$ROOT/scripts/docker-test.sh" bash scripts/gates/m13-complete.sh 2>"$wrapper_log"); then
  cat "$wrapper_log" >&2
  printf '%s\n' 'docker-test wrapper probe failed' >&2
  exit 1
fi
expected_mount="$mounted"
if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == mingw* || "${OSTYPE:-}" == cygwin* ]]; then
  expected_mount="$(cygpath -m "$mounted")"
fi
grep -Fq -- "-v $expected_mount:/work/target/m13-input:ro" "$docker_capture"
grep -Fq -- '-e OQUEUE_M13_EVIDENCE_INPUT_DIR=/work/target/m13-input' "$docker_capture"

printf '%s\n' 'release-aggregate: ok'
