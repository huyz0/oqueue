#!/usr/bin/env bash
# M13.11: checksum manifests and signatures fail closed on mutation.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-attest.sh"
test -x "$script"
release="$ROOT/.github/workflows/release.yml"

scratch="$ROOT/target/tmp/release-attest-test.$$"
mkdir -p "$scratch"
trap 'rm -rf "$scratch"' EXIT

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$scratch/private.pem" 2>/dev/null
openssl pkey -in "$scratch/private.pem" -pubout -out "$scratch/public.pem" 2>/dev/null
artifact_a="$scratch/oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin"
artifact_b="$scratch/oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin"
printf '%s\n' default > "$artifact_a"
printf '%s\n' fips > "$artifact_b"

bash "$script" create --key "$scratch/private.pem" --manifest "$scratch/SHA256SUMS" \
  "$artifact_b" "$artifact_a"
test -s "$scratch/SHA256SUMS"
test -s "$scratch/SHA256SUMS.sig"
grep -Fq 'oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin' "$scratch/SHA256SUMS"
grep -Fq 'oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin' "$scratch/SHA256SUMS"
first_entry="$(awk 'NR == 1 { print; exit }' "$scratch/SHA256SUMS")"
second_entry="$(awk 'NR == 2 { print; exit }' "$scratch/SHA256SUMS")"
[[ "$first_entry" == *oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin ]]
[[ "$second_entry" == *oqueue-0.0.0-linux-x86_64-glibc2.28-fips-bin ]]

bash "$script" verify --key "$scratch/public.pem" --manifest "$scratch/SHA256SUMS" \
  --signature "$scratch/SHA256SUMS.sig" "$artifact_a" "$artifact_b"

printf '%s\n' changed > "$artifact_b"
if bash "$script" verify --key "$scratch/public.pem" --manifest "$scratch/SHA256SUMS" \
    --signature "$scratch/SHA256SUMS.sig" "$artifact_a" "$artifact_b" >/dev/null 2>&1; then
  printf '%s\n' 'release-attest accepted a mutated artifact' >&2
  exit 1
fi
printf '%s\n' fips > "$artifact_b"

cp "$scratch/SHA256SUMS" "$scratch/SHA256SUMS.good"
awk 'NR == 1 { sub(/^[0-9a-f]*/, "0000000000000000000000000000000000000000000000000000000000000000") } { print }' \
  "$scratch/SHA256SUMS" > "$scratch/SHA256SUMS.mutated"
mv "$scratch/SHA256SUMS.mutated" "$scratch/SHA256SUMS"
if bash "$script" verify --key "$scratch/public.pem" --manifest "$scratch/SHA256SUMS" \
    --signature "$scratch/SHA256SUMS.sig" "$artifact_a" "$artifact_b" >/dev/null 2>&1; then
  printf '%s\n' 'release-attest accepted a mutated manifest' >&2
  exit 1
fi
cp "$scratch/SHA256SUMS.good" "$scratch/SHA256SUMS"
cp "$scratch/SHA256SUMS.sig" "$scratch/SHA256SUMS.sig.good"
signature_first_byte="$(od -An -N1 -tu1 "$scratch/SHA256SUMS.sig" | tr -d '[:space:]')"
[[ "$signature_first_byte" =~ ^[0-9]+$ ]]
signature_mutated_byte=$(((signature_first_byte + 1) % 256))
printf -v signature_mutated_octal '\\%03o' "$signature_mutated_byte"
printf '%b' "$signature_mutated_octal" | \
  dd of="$scratch/SHA256SUMS.sig" bs=1 seek=0 conv=notrunc >/dev/null 2>&1
if bash "$script" verify --key "$scratch/public.pem" --manifest "$scratch/SHA256SUMS" \
    --signature "$scratch/SHA256SUMS.sig" "$artifact_a" "$artifact_b" >/dev/null 2>&1; then
  printf '%s\n' 'release-attest accepted a mutated signature' >&2
  exit 1
fi
cp "$scratch/SHA256SUMS.sig.good" "$scratch/SHA256SUMS.sig"

mkdir -p "$scratch/duplicate-a" "$scratch/duplicate-b"
printf '%s\n' first > "$scratch/duplicate-a/same-name"
printf '%s\n' second > "$scratch/duplicate-b/same-name"
if bash "$script" create --key "$scratch/private.pem" --manifest "$scratch/duplicate.SHA256SUMS" \
    "$scratch/duplicate-a/same-name" "$scratch/duplicate-b/same-name" >/dev/null 2>&1; then
  printf '%s\n' 'release-attest accepted duplicate artifact basenames' >&2
  exit 1
fi

openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out "$scratch/wrong-private.pem" 2>/dev/null
openssl pkey -in "$scratch/wrong-private.pem" -pubout -out "$scratch/wrong-public.pem" 2>/dev/null
if bash "$script" verify --key "$scratch/wrong-public.pem" --manifest "$scratch/SHA256SUMS" \
    --signature "$scratch/SHA256SUMS.sig" "$artifact_a" "$artifact_b" >/dev/null 2>&1; then
  printf '%s\n' 'release-attest accepted the wrong verification key' >&2
  exit 1
fi

job_block() {
  local job="$1"
  awk -v wanted="  $job:" '
    $0 == wanted { in_job = 1 }
    in_job && NR > 1 && $0 ~ /^  [a-z][a-z0-9-]*:/ && $0 != wanted { exit }
    in_job { print }
  ' "$release"
}
for job in linux-x86_64 linux-aarch64; do
  job_text="$(job_block "$job")"
  grep -Fq 'scripts/release-attest.sh create' <<<"$job_text"
  grep -Fq 'scripts/release-attest.sh verify' <<<"$job_text"
  grep -Fq 'target/attest/SHA256SUMS' <<<"$job_text"
  grep -Fq 'OQUEUE_RELEASE_PRIVATE_KEY' <<<"$job_text"
done

printf '%s\n' 'release-attest: ok'
