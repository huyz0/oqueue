#!/usr/bin/env bash
# Aggregate the release contracts that do not require producing a new build.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-verify.$$}"
mkdir -p "$EVIDENCE_DIR"

[[ "$#" -ge 1 && "$1" == verify ]] || {
  printf '%s\n' 'usage: release-verify.sh verify [--completion-gate]' >&2
  exit 2
}
shift
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --completion-gate) shift ;;
    *) printf '%s\n' 'usage: release-verify.sh verify [--completion-gate]' >&2; exit 2 ;;
  esac
done

run_id="$(git -C "$ROOT" rev-parse HEAD)"
[[ "$run_id" =~ ^[0-9a-f]{40}$ ]] || {
  printf 'release-verify: expected a full commit id, got %s\n' "$run_id" >&2
  exit 1
}

build_evidence="${OQUEUE_M13_BUILD_EVIDENCE:-$EVIDENCE_DIR/build.tsv}"
artifact_dir="${OQUEUE_M13_ARTIFACT_DIR:-$EVIDENCE_DIR/artifacts}"
repro_evidence="${OQUEUE_M13_REPRO_EVIDENCE:-$EVIDENCE_DIR/repro.tsv}"
[[ -f "$build_evidence" ]] || { printf 'release-verify: missing build evidence: %s\n' "$build_evidence" >&2; exit 1; }
build_line="$(grep -E '^M13_RELEASE_BUILD ' "$build_evidence" || true)"
[[ "$(printf '%s\n' "$build_line" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  printf '%s\n' 'release-verify: build evidence must contain exactly one release-build record' >&2
  exit 1
}
build_run_id="$(sed -n 's/.* run_id=\([^ ]*\).*/\1/p' <<< "$build_line")"
[[ "$build_run_id" == "$run_id" ]] || { printf '%s\n' 'release-verify: build and verifier run ids differ' >&2; exit 1; }
release_sha="$(sed -n 's/.* default_x86_sha256=\([^ ]*\).*/\1/p' <<< "$build_line")"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'
  else printf '%s\n' 'release-verify: need sha256sum or shasum' >&2; exit 1; fi
}

versioned_artifact() {
  local variant="$1" arch="$2"
  bash "$ROOT/scripts/release-artifact-name.sh" linux "$arch" glibc2.28 "$variant" bin
}

declare -a artifact_lines=()
for identity in 'default x86_64 default_x86_sha256' 'fips x86_64 fips_x86_sha256' \
                'default aarch64 default_aarch64_sha256' 'fips aarch64 fips_aarch64_sha256'; do
  read -r variant arch hash_field <<< "$identity"
  expected_sha="$(sed -n "s/.* $hash_field=\([^ ]*\).*/\1/p" <<< "$build_line")"
  name="$(versioned_artifact "$variant" "$arch")"
  path="$artifact_dir/$name"
  [[ -f "$path" ]] || { printf 'release-verify: missing artifact: %s\n' "$path" >&2; exit 1; }
  actual_sha="$(hash_file "$path")"
  [[ "$actual_sha" == "$expected_sha" ]] || {
    printf 'release-verify: hash mismatch for %s\n' "$name" >&2
    exit 1
  }
  artifact_lines+=("M13_ARTIFACT $variant run_id=$run_id name=$name sha256=$actual_sha")
done

[[ -f "$repro_evidence" ]] || { printf 'release-verify: missing reproducibility evidence: %s\n' "$repro_evidence" >&2; exit 1; }
repro_line="$(grep -E '^M13_REPRODUCIBILITY ' "$repro_evidence" || true)"
[[ "$(printf '%s\n' "$repro_line" | sed '/^$/d' | wc -l | tr -d ' ')" == 1 ]] || {
  printf '%s\n' 'release-verify: reproducibility evidence must contain exactly one record' >&2
  exit 1
}
repro_sha='[0-9a-f]{64}'
if ! grep -Eq "^M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=$release_sha mode=byte-identical release_build=[ab] build_a=[^ ]+ sha256_a=$repro_sha build_b=[^ ]+ sha256_b=$repro_sha comparison=equal commands_sha256=$repro_sha outputs_sha256=$repro_sha$" <<< "$repro_line"; then
  printf '%s\n' 'release-verify: malformed reproducibility evidence' >&2
  exit 1
fi
repro_a="$(sed -n 's/.* build_a=\([^ ]*\).*/\1/p' <<< "$repro_line")"
repro_b="$(sed -n 's/.* build_b=\([^ ]*\).*/\1/p' <<< "$repro_line")"
repro_sha_a="$(sed -n 's/.* sha256_a=\([^ ]*\).*/\1/p' <<< "$repro_line")"
repro_sha_b="$(sed -n 's/.* sha256_b=\([^ ]*\).*/\1/p' <<< "$repro_line")"
[[ -f "$repro_a" && -f "$repro_b" && "$repro_a" != "$repro_b" ]] || {
  printf '%s\n' 'release-verify: reproducibility build outputs are missing or identical' >&2
  exit 1
}
[[ "$(hash_file "$repro_a")" == "$repro_sha_a" && "$(hash_file "$repro_b")" == "$repro_sha_b" && \
   "$repro_sha_a" == "$repro_sha_b" && "$repro_sha_a" == "$release_sha" ]] || {
  printf '%s\n' 'release-verify: reproducibility evidence does not prove two equal builds' >&2
  exit 1
}

manifest="${OQUEUE_M13_MANIFEST:-$artifact_dir/SHA256SUMS}"
signature="${OQUEUE_M13_SIGNATURE:-$manifest.sig}"
public_key="${OQUEUE_M13_PUBLIC_KEY:-$artifact_dir/release-public.pem}"
image_evidence="${OQUEUE_M13_IMAGE_EVIDENCE:-$artifact_dir/images.tsv}"
[[ -f "$manifest" && -f "$signature" && -f "$public_key" && -f "$image_evidence" ]] || {
  printf '%s\n' 'release-verify: checksum/signature/image evidence is incomplete' >&2
  exit 1
}

bash "$ROOT/tests/release-artifact-name.sh" >/dev/null
bash "$ROOT/scripts/release-attest.sh" verify --key "$public_key" --manifest "$manifest" \
  --signature "$signature" -- "$artifact_dir/$(versioned_artifact default x86_64)" \
  "$artifact_dir/$(versioned_artifact fips x86_64)" \
  "$artifact_dir/$(versioned_artifact default aarch64)" \
  "$artifact_dir/$(versioned_artifact fips aarch64)" >/dev/null
for identity in 'default x86_64 amd64 default_x86_sha256' 'fips x86_64 amd64 fips_x86_sha256' \
                'default aarch64 arm64 default_aarch64_sha256' 'fips aarch64 arm64 fips_aarch64_sha256'; do
  read -r variant arch image_arch hash_field <<< "$identity"
  name="$(versioned_artifact "$variant" "$arch")"
  expected_sha="$(sed -n "s/.* $hash_field=\([^ ]*\).*/\1/p" <<< "$build_line")"
  image_matches="$(grep -Ec "^M13_RELEASE_IMAGE status=pass architecture=$image_arch startup=pass binary_match=pass artifact=$name artifact_sha256=$expected_sha image_id=sha256:[^ ]+$" "$image_evidence" || true)"
  [[ "$image_matches" == 1 ]] || {
    printf 'release-verify: missing image evidence for %s/%s\n' "$variant" "$arch" >&2
    exit 1
  }
done
bash "$ROOT/scripts/release-upgrade.sh" --completion-gate >/dev/null

printf '%s\n' \
  "M13_RELEASE_VERIFY status=pass run_id=$run_id naming=pass musl=pass checksums=pass signatures=pass images=pass upgrade=pass" \
  > "$EVIDENCE_DIR/verify.tsv"
printf '%s\n' "${artifact_lines[@]}" >> "$EVIDENCE_DIR/verify.tsv"
printf '%s\n' "$repro_line" >> "$EVIDENCE_DIR/verify.tsv"
printf '%s\n' "$(<"$EVIDENCE_DIR/verify.tsv")"
