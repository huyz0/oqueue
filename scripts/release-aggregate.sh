#!/usr/bin/env bash
# Aggregate the outputs of the native Linux release jobs into gate evidence.
# This script never builds or invents an artifact; every hash is recomputed from
# the four files retained by the release matrix.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
input=""
output=""
provenance=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --input) [[ "$#" -ge 2 ]] || exit 2; input="$2"; shift 2 ;;
    --output) [[ "$#" -ge 2 ]] || exit 2; output="$2"; shift 2 ;;
    --provenance) [[ "$#" -ge 2 ]] || exit 2; provenance="$2"; shift 2 ;;
    *) printf '%s\n' 'usage: release-aggregate.sh --input DIR --output DIR [--provenance FILE]' >&2; exit 2 ;;
  esac
done
[[ -n "$input" && -n "$output" ]] || {
  printf '%s\n' 'usage: release-aggregate.sh --input DIR --output DIR [--provenance FILE]' >&2
  exit 2
}
[[ -d "$input" ]] || { printf 'release-aggregate: input directory not found: %s\n' "$input" >&2; exit 1; }
mkdir -p "$output"
artifact_dir="$input/artifacts"
provenance="${provenance:-$input/provenance.tsv}"
[[ -d "$artifact_dir" ]] || { printf 'release-aggregate: artifact directory not found: %s\n' "$artifact_dir" >&2; exit 1; }

run_id="$(git -C "$ROOT" rev-parse HEAD)"
[[ "$run_id" =~ ^[0-9a-f]{40}$ ]] || {
  printf 'release-aggregate: expected a full commit id, got %s\n' "$run_id" >&2
  exit 1
}
provenance_line="$(grep -E '^M13_RELEASE_PROVENANCE ' "$provenance" 2>/dev/null || true)"
[[ "$(printf '%s\n' "$provenance_line" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  printf '%s\n' 'release-aggregate: provenance must contain exactly one record' >&2
  exit 1
}
expected_provenance="M13_RELEASE_PROVENANCE status=pass run_id=$run_id native_x86=pass native_aarch64=pass clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass"
[[ "$provenance_line" == "$expected_provenance" ]] || {
  printf '%s\n' 'release-aggregate: native release provenance is incomplete or does not match HEAD' >&2
  exit 1
}

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    printf '%s\n' 'release-aggregate: need sha256sum or shasum' >&2
    exit 1
  fi
}

declare -a artifact_lines=()
for identity in 'default x86_64 default_x86_sha256' 'fips x86_64 fips_x86_sha256' \
                'default aarch64 default_aarch64_sha256' 'fips aarch64 fips_aarch64_sha256'; do
  read -r variant arch hash_field <<< "$identity"
  name="$(bash "$ROOT/scripts/release-artifact-name.sh" linux "$arch" glibc2.28 "$variant" bin)"
  path="$artifact_dir/$name"
  [[ -f "$path" ]] || { printf 'release-aggregate: missing native artifact: %s\n' "$path" >&2; exit 1; }
  sha="$(hash_file "$path")"
  artifact_lines+=("$hash_field=$sha")
done

release_sha="${artifact_lines[0]#default_x86_sha256=}"
source_repro="$input/repro/repro.tsv"
[[ -f "$source_repro" ]] || {
  printf 'release-aggregate: missing reproducibility evidence: %s\n' "$source_repro" >&2
  exit 1
}
source_repro_line="$(grep -E '^M13_REPRODUCIBILITY ' "$source_repro" 2>/dev/null || true)"
[[ "$(printf '%s\n' "$source_repro_line" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  printf '%s\n' 'release-aggregate: reproducibility evidence must contain exactly one record' >&2
  exit 1
}
repro_a="$input/repro/build-a/x86_64-unknown-linux-gnu/release/oqueue"
repro_b="$input/repro/build-b/x86_64-unknown-linux-gnu/release/oqueue"
[[ -f "$repro_a" && -f "$repro_b" ]] || {
  printf '%s\n' 'release-aggregate: native reproducibility build outputs are incomplete' >&2
  exit 1
}
repro_sha_a="$(hash_file "$repro_a")"
repro_sha_b="$(hash_file "$repro_b")"
[[ "$repro_sha_a" == "$release_sha" && "$repro_sha_b" == "$release_sha" ]] || {
  printf '%s\n' 'release-aggregate: reproducibility outputs do not match the release artifact' >&2
  exit 1
}
source_commands_sha="$(sed -n 's/.* commands_sha256=\([^ ]*\).*/\1/p' <<< "$source_repro_line")"
source_outputs_sha="$(sed -n 's/.* outputs_sha256=\([^ ]*\).*/\1/p' <<< "$source_repro_line")"
[[ "$source_repro_line" == *"run_id=$run_id"* && "$source_repro_line" == *'mode=byte-identical'* && \
   "$source_commands_sha" =~ ^[0-9a-f]{64}$ && "$source_outputs_sha" =~ ^[0-9a-f]{64}$ ]] || {
  printf '%s\n' 'release-aggregate: reproducibility evidence is malformed or belongs to another commit' >&2
  exit 1
}

build_tmp="$output/build.tsv.tmp.$$"
trap 'rm -f "$build_tmp"' EXIT
printf '%s\n' \
  "M13_RELEASE_BUILD status=pass run_id=$run_id ${artifact_lines[*]} clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass" \
  > "$build_tmp"
mv "$build_tmp" "$output/build.tsv"
printf '%s\n' \
  "M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=$release_sha mode=byte-identical release_build=a build_a=repro/build-a/x86_64-unknown-linux-gnu/release/oqueue sha256_a=$repro_sha_a build_b=repro/build-b/x86_64-unknown-linux-gnu/release/oqueue sha256_b=$repro_sha_b comparison=equal commands_sha256=$source_commands_sha outputs_sha256=$source_outputs_sha" \
  > "$output/repro.tsv"
printf '%s\n' "$(<"$output/build.tsv")"
