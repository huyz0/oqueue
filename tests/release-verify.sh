#!/usr/bin/env bash
# M13.14 contract: aggregate verification consumes real artifact files and hashes.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-verify.sh"
test -x "$script"
scratch="$ROOT/target/tmp/release-verify-test.$$"
mkdir -p "$scratch/artifacts"
trap 'rm -rf "$scratch"' EXIT

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
hash_text() {
  if command -v sha256sum >/dev/null 2>&1; then printf '%s' "$1" | sha256sum | awk '{print $1}'
  else printf '%s' "$1" | shasum -a 256 | awk '{print $1}'; fi
}
run_id="$(git -C "$ROOT" rev-parse HEAD)"
declare -a fields=()
for identity in 'default x86_64 default_x86_sha256' 'fips x86_64 fips_x86_sha256' \
                'default aarch64 default_aarch64_sha256' 'fips aarch64 fips_aarch64_sha256'; do
  read -r variant arch field <<< "$identity"
  name="$(bash "$ROOT/scripts/release-artifact-name.sh" linux "$arch" glibc2.28 "$variant" bin)"
  printf '%s %s\n' "$variant" "$arch" > "$scratch/artifacts/$name"
  fields+=("$field=$(hash_file "$scratch/artifacts/$name")")
done
openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$scratch/private.pem" 2>/dev/null
openssl pkey -in "$scratch/private.pem" -pubout -out "$scratch/artifacts/release-public.pem" 2>/dev/null
bash "$ROOT/scripts/release-attest.sh" create --key "$scratch/private.pem" --manifest "$scratch/artifacts/SHA256SUMS" \
  "$scratch/artifacts/oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin" \
  "$scratch/artifacts/oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin" \
  "$scratch/artifacts/oqueue-0.0.0-linux-aarch64-glibc2.28-default-bin" \
  "$scratch/artifacts/oqueue-0.0.0-linux-aarch64-glibc2.28-fips-bin" >/dev/null
printf '%s\n' \
  "M13_RELEASE_IMAGE status=pass architecture=amd64 startup=pass binary_match=pass artifact=oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin artifact_sha256=${fields[0]#*=} image_id=sha256:test-x86-default" \
  "M13_RELEASE_IMAGE status=pass architecture=amd64 startup=pass binary_match=pass artifact=oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin artifact_sha256=${fields[1]#*=} image_id=sha256:test-x86-fips" \
  "M13_RELEASE_IMAGE status=pass architecture=arm64 startup=pass binary_match=pass artifact=oqueue-0.0.0-linux-aarch64-glibc2.28-default-bin artifact_sha256=${fields[2]#*=} image_id=sha256:test-arm-default" \
  "M13_RELEASE_IMAGE status=pass architecture=arm64 startup=pass binary_match=pass artifact=oqueue-0.0.0-linux-aarch64-glibc2.28-fips-bin artifact_sha256=${fields[3]#*=} image_id=sha256:test-arm-fips" \
  > "$scratch/artifacts/images.tsv"
cp "$scratch/artifacts/oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin" "$scratch/build-a"
cp "$scratch/artifacts/oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin" "$scratch/build-b"
printf '%s\n' "M13_RELEASE_BUILD status=pass run_id=$run_id ${fields[*]} clean_container=pass fips=pass fips_mode=pass fips_cross_read=pass nonfips_cross_read=pass isa=pass allocator=pass" > "$scratch/build.tsv"
printf '%s\n' "M13_REPRODUCIBILITY run_id=$run_id release_artifact_sha256=${fields[0]#*=} mode=byte-identical release_build=a build_a=$scratch/build-a sha256_a=${fields[0]#*=} build_b=$scratch/build-b sha256_b=${fields[0]#*=} comparison=equal commands_sha256=$(hash_text commands) outputs_sha256=$(hash_text outputs)" > "$scratch/repro.tsv"

OQUEUE_M13_EVIDENCE_DIR="$scratch/out" OQUEUE_M13_BUILD_EVIDENCE="$scratch/build.tsv" \
  OQUEUE_M13_ARTIFACT_DIR="$scratch/artifacts" OQUEUE_M13_REPRO_EVIDENCE="$scratch/repro.tsv" \
  bash "$script" verify --completion-gate >/dev/null
test "$(grep -Ec '^M13_ARTIFACT ' "$scratch/out/verify.tsv")" = 4
grep -Fq "run_id=$run_id" "$scratch/out/verify.tsv"
grep -Fq 'M13_REPRODUCIBILITY ' "$scratch/out/verify.tsv"

cp "$scratch/artifacts/images.tsv" "$scratch/duplicate-images.tsv"
tail -n 1 "$scratch/artifacts/images.tsv" >> "$scratch/duplicate-images.tsv"
if OQUEUE_M13_EVIDENCE_DIR="$scratch/duplicate-out" OQUEUE_M13_BUILD_EVIDENCE="$scratch/build.tsv" \
    OQUEUE_M13_ARTIFACT_DIR="$scratch/artifacts" OQUEUE_M13_REPRO_EVIDENCE="$scratch/repro.tsv" \
    OQUEUE_M13_IMAGE_EVIDENCE="$scratch/duplicate-images.tsv" bash "$script" verify --completion-gate >/dev/null 2>&1; then
  printf '%s\n' 'release-verify accepted duplicate image evidence' >&2
  exit 1
fi

printf '%s\n' changed >> "$scratch/build-b"
if OQUEUE_M13_EVIDENCE_DIR="$scratch/mismatch-out" OQUEUE_M13_BUILD_EVIDENCE="$scratch/build.tsv" \
    OQUEUE_M13_ARTIFACT_DIR="$scratch/artifacts" OQUEUE_M13_REPRO_EVIDENCE="$scratch/repro.tsv" \
    bash "$script" verify --completion-gate >/dev/null 2>&1; then
  printf '%s\n' 'release-verify accepted mismatched reproducibility output' >&2
  exit 1
fi
printf '%s\n' 'release-verify: ok'
