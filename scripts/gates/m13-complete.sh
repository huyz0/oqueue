#!/usr/bin/env bash
# M13's completion condition. Release claims are accepted only when the
# build, artifact, FIPS, portability, and upgrade evidence can be rerun.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
cd "$REPO_ROOT" || exit 1

EVIDENCE_DIR="$REPO_ROOT/target/tmp/m13-gate.$$"
mkdir -p "$EVIDENCE_DIR"
trap 'rm -rf "$EVIDENCE_DIR"' EXIT

# Native Linux jobs produce the ARM/FIPS artifacts outside this gate.  A
# completion run may point at that immutable bundle; the release scripts then
# validate and copy its records instead of fabricating cross-build evidence.
EVIDENCE_INPUT_DIR="${OQUEUE_M13_EVIDENCE_INPUT_DIR:-}"
INPUT_ERROR=""
if [[ -n "$EVIDENCE_INPUT_DIR" ]]; then
  if [[ ! -d "$EVIDENCE_INPUT_DIR" ]]; then
    INPUT_ERROR="M13 evidence bundle is not a directory: $EVIDENCE_INPUT_DIR"
  else
    export OQUEUE_M13_BUILD_EVIDENCE="$EVIDENCE_INPUT_DIR/build.tsv"
    export OQUEUE_M13_SMOKE_EVIDENCE="$EVIDENCE_INPUT_DIR/smoke.tsv"
    export OQUEUE_M13_ARTIFACT_DIR="$EVIDENCE_INPUT_DIR/artifacts"
    export OQUEUE_M13_REPRO_EVIDENCE="$EVIDENCE_INPUT_DIR/repro.tsv"
    export OQUEUE_M13_MANIFEST="$EVIDENCE_INPUT_DIR/artifacts/SHA256SUMS"
    export OQUEUE_M13_SIGNATURE="$EVIDENCE_INPUT_DIR/artifacts/SHA256SUMS.sig"
    export OQUEUE_M13_PUBLIC_KEY="$EVIDENCE_INPUT_DIR/artifacts/release-public.pem"
    export OQUEUE_M13_IMAGE_EVIDENCE="$EVIDENCE_INPUT_DIR/artifacts/images.tsv"
  fi
fi

mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M13 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M13 commit is covered by a milestone review"
else
  fail "leg 0: M13's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M13"
fi

require_file() {
  local label="$1" path="$2"
  if [[ -f "$path" ]]; then
    ok "$label: $path"
  else
    fail "$label: required evidence is missing: $path"
  fi
}

require_script() {
  local label="$1" path="$2"
  if [[ -x "$path" ]]; then
    ok "$label: $path"
  else
    fail "$label: required executable evidence is missing: $path"
  fi
}

require_file "release artifact policy" "$REPO_ROOT/docs/release/artifacts.md"
require_file "release workflow" "$REPO_ROOT/.github/workflows/release.yml"
require_script "release build" "$REPO_ROOT/scripts/release-build.sh"
require_script "release verification" "$REPO_ROOT/scripts/release-verify.sh"
require_script "release evidence aggregation" "$REPO_ROOT/scripts/release-aggregate.sh"
require_script "oldest-distribution smoke" "$REPO_ROOT/scripts/release-smoke.sh"
require_script "release workflow checker" "$REPO_ROOT/scripts/check-release-workflow.sh"

require_evidence() {
  local label="$1" path="$2" marker="$3"
  if [[ ! -f "$path" ]]; then
    fail "$label: evidence file is missing: $path"
    return
  fi
  local matches
  matches="$(grep -Ec "$marker" "$path" || true)"
  if [[ "$matches" == "1" ]]; then
    ok "$label: $marker"
  elif [[ "$matches" != "0" ]]; then
    fail "$label: evidence contains $matches matching records; exactly one is required"
  else
    fail "$label: evidence does not contain the required result"
    note "required line: $marker"
  fi
}

require_unique_record() {
  local label="$1" path="$2" prefix="$3" records
  records="$(grep -Ec "$prefix" "$path" 2>/dev/null || true)"
  if [[ "$records" == "1" ]]; then
    ok "$label: one record"
  else
    fail "$label: expected one record, found ${records:-0}"
  fi
}

run_required() {
  local label="$1" path="$2" evidence="$3" marker="$4"
  shift 4
  if [[ ! -x "$path" ]]; then
    return 0
  fi
  local output=""
  if output="$(OQUEUE_M13_EVIDENCE_DIR="$EVIDENCE_DIR" bash "$path" "$@" 2>&1)"; then
    ok "$label exited successfully"
    require_evidence "$label" "$EVIDENCE_DIR/$evidence" "$marker"
  else
    fail "$label failed"
    note "$output"
  fi
}

if [[ -n "$INPUT_ERROR" ]]; then
  fail "$INPUT_ERROR"
fi

# Presence, exit status, and raw workflow text are not evidence: an executable
# no-op script, a skipped leg, or a requirement hidden in a comment must not
# close the milestone. Each command must write a run-scoped, exact evidence
# line, so stale artifacts cannot satisfy this gate.
run_required "release workflow evidence" \
  "$REPO_ROOT/scripts/check-release-workflow.sh" workflow.tsv \
  '^M13_RELEASE_WORKFLOW status=pass matrix_linux_x86_64=pass matrix_linux_aarch64=pass macos_fast=pass glibc_floor=2\.28 fips_job=pass$' \
  --completion-gate
require_unique_record "release workflow evidence" "$EVIDENCE_DIR/workflow.tsv" '^M13_RELEASE_WORKFLOW '
run_required "release build evidence" \
  "$REPO_ROOT/scripts/release-build.sh" build.tsv \
  '^M13_RELEASE_BUILD status=pass run_id=[0-9a-f]{40} default_x86_sha256=[0-9a-f]{64} fips_x86_sha256=[0-9a-f]{64} default_aarch64_sha256=[0-9a-f]{64} fips_aarch64_sha256=[0-9a-f]{64} clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass$' \
  --completion-gate

build_line=""
if [[ -f "$EVIDENCE_DIR/build.tsv" ]]; then
  build_records="$(grep -Ec '^M13_RELEASE_BUILD ' "$EVIDENCE_DIR/build.tsv" || true)"
  if [[ "$build_records" == "1" ]]; then
    build_line="$(grep -E '^M13_RELEASE_BUILD ' "$EVIDENCE_DIR/build.tsv")"
  else
    fail "release build evidence must contain exactly one release-build record"
  fi
fi
BUILD_RUN_ID="$(sed -n 's/.* run_id=\([^ ]*\).*/\1/p' <<< "$build_line")"
BUILD_DEFAULT_X86_SHA="$(sed -n 's/.* default_x86_sha256=\([^ ]*\).*/\1/p' <<< "$build_line")"
BUILD_FIPS_X86_SHA="$(sed -n 's/.* fips_x86_sha256=\([^ ]*\).*/\1/p' <<< "$build_line")"
BUILD_DEFAULT_AARCH64_SHA="$(sed -n 's/.* default_aarch64_sha256=\([^ ]*\).*/\1/p' <<< "$build_line")"
BUILD_FIPS_AARCH64_SHA="$(sed -n 's/.* fips_aarch64_sha256=\([^ ]*\).*/\1/p' <<< "$build_line")"
if [[ -z "$BUILD_RUN_ID" || -z "$BUILD_DEFAULT_X86_SHA" || -z "$BUILD_FIPS_X86_SHA" ||
      -z "$BUILD_DEFAULT_AARCH64_SHA" || -z "$BUILD_FIPS_AARCH64_SHA" ]]; then
  fail "release build evidence does not expose one run id and four artifact hashes"
fi

run_required "oldest-distribution smoke evidence" \
  "$REPO_ROOT/scripts/release-smoke.sh" smoke.tsv \
  '^M13_RELEASE_SMOKE status=pass glibc_floor=2\.28 request=pass$' \
  --completion-gate
require_unique_record "oldest-distribution smoke evidence" "$EVIDENCE_DIR/smoke.tsv" '^M13_RELEASE_SMOKE '
run_required "artifact, supply-chain, image, and upgrade evidence" \
  "$REPO_ROOT/scripts/release-verify.sh" verify.tsv \
  '^M13_RELEASE_VERIFY status=pass run_id=[0-9a-f]{40} naming=pass musl=pass checksums=pass signatures=pass images=pass upgrade=pass$' \
  --completion-gate

if [[ -n "$BUILD_RUN_ID" ]]; then
  verify_records="$(grep -Ec '^M13_RELEASE_VERIFY ' "$EVIDENCE_DIR/verify.tsv" 2>/dev/null || true)"
  if [[ "$verify_records" != "1" ]]; then
    fail "release verification evidence must contain exactly one verification record"
  fi
  artifact_records="$(grep -Ec '^M13_ARTIFACT ' "$EVIDENCE_DIR/verify.tsv" 2>/dev/null || true)"
  if [[ "$artifact_records" != "4" ]]; then
    fail "release verification evidence must contain exactly four Linux artifact records"
  fi
  require_evidence "verification uses the release-build run" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_RELEASE_VERIFY status=pass run_id=${BUILD_RUN_ID} naming=pass musl=pass checksums=pass signatures=pass images=pass upgrade=pass$"
  require_evidence "default x86_64 artifact identity" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_ARTIFACT default run_id=${BUILD_RUN_ID} name=oqueue-[0-9]+\\.[0-9]+\\.[0-9]+-linux-x86_64-glibc2\\.28-default-[^ ]+ sha256=${BUILD_DEFAULT_X86_SHA}$"
  require_evidence "FIPS x86_64 artifact identity" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_ARTIFACT fips run_id=${BUILD_RUN_ID} name=oqueue-[0-9]+\\.[0-9]+\\.[0-9]+-linux-x86_64-glibc2\\.28-fips-[^ ]+ sha256=${BUILD_FIPS_X86_SHA}$"
  require_evidence "default aarch64 artifact identity" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_ARTIFACT default run_id=${BUILD_RUN_ID} name=oqueue-[0-9]+\\.[0-9]+\\.[0-9]+-linux-aarch64-glibc2\\.28-default-[^ ]+ sha256=${BUILD_DEFAULT_AARCH64_SHA}$"
  require_evidence "FIPS aarch64 artifact identity" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_ARTIFACT fips run_id=${BUILD_RUN_ID} name=oqueue-[0-9]+\\.[0-9]+\\.[0-9]+-linux-aarch64-glibc2\\.28-fips-[^ ]+ sha256=${BUILD_FIPS_AARCH64_SHA}$"
  require_evidence "reproducibility evidence" "$EVIDENCE_DIR/verify.tsv" \
    "^M13_REPRODUCIBILITY run_id=${BUILD_RUN_ID} release_artifact_sha256=${BUILD_DEFAULT_X86_SHA} mode=byte-identical release_build=a build_a=[^ ]+ sha256_a=${BUILD_DEFAULT_X86_SHA} build_b=[^ ]+ sha256_b=${BUILD_DEFAULT_X86_SHA} comparison=equal commands_sha256=[0-9a-f]{64} outputs_sha256=[0-9a-f]{64}$|^M13_REPRODUCIBILITY run_id=${BUILD_RUN_ID} release_artifact_sha256=${BUILD_DEFAULT_X86_SHA} mode=behavioral release_build=a build_a=[^ ]+ sha256_a=${BUILD_DEFAULT_X86_SHA} build_b=[^ ]+ sha256_b=[0-9a-f]{64} startup=pass kafka=pass object_format=pass fips=pass comparison=equivalent commands_sha256=[0-9a-f]{64} outputs_sha256=[0-9a-f]{64}$|^M13_REPRODUCIBILITY run_id=${BUILD_RUN_ID} release_artifact_sha256=${BUILD_DEFAULT_X86_SHA} mode=behavioral release_build=b build_a=[^ ]+ sha256_a=[0-9a-f]{64} build_b=[^ ]+ sha256_b=${BUILD_DEFAULT_X86_SHA} startup=pass kafka=pass object_format=pass fips=pass comparison=equivalent commands_sha256=[0-9a-f]{64} outputs_sha256=[0-9a-f]{64}$"
  reproducibility_records="$(grep -Ec '^M13_REPRODUCIBILITY ' "$EVIDENCE_DIR/verify.tsv" 2>/dev/null || true)"
  if [[ "$reproducibility_records" != "1" ]]; then
    fail "release verification evidence must contain exactly one reproducibility record"
  fi
  reproducibility_line="$(grep -E '^M13_REPRODUCIBILITY ' "$EVIDENCE_DIR/verify.tsv" || true)"
  reproducibility_mode="$(sed -n 's/.* mode=\([^ ]*\).*/\1/p' <<< "$reproducibility_line")"
  reproducibility_a_id="$(sed -n 's/.* build_a=\([^ ]*\).*/\1/p' <<< "$reproducibility_line")"
  reproducibility_b_id="$(sed -n 's/.* build_b=\([^ ]*\).*/\1/p' <<< "$reproducibility_line")"
  if [[ -n "$reproducibility_a_id" && "$reproducibility_a_id" == "$reproducibility_b_id" ]]; then
    fail "reproducibility compares the same build identity twice"
  fi
  if [[ "$reproducibility_mode" == "byte-identical" ]]; then
    reproducibility_a="$(sed -n 's/.* sha256_a=\([^ ]*\).*/\1/p' <<< "$reproducibility_line")"
    reproducibility_b="$(sed -n 's/.* sha256_b=\([^ ]*\).*/\1/p' <<< "$reproducibility_line")"
    if [[ -n "$reproducibility_a" && "$reproducibility_a" == "$reproducibility_b" ]]; then
      ok "byte-identical reproducibility hashes agree"
    else
      fail "byte-identical reproducibility hashes do not agree"
    fi
  fi
fi

finish
