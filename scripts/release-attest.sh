#!/usr/bin/env bash
# Create or verify a deterministic checksum manifest and OpenSSL signature.
set -euo pipefail

usage() {
  printf '%s\n' \
    'usage: release-attest.sh create --key PRIVATE --manifest MANIFEST ARTIFACT...' \
    '       release-attest.sh verify --key PUBLIC --manifest MANIFEST --signature SIG ARTIFACT...' >&2
  exit 2
}

[[ "$#" -ge 1 ]] || usage
mode="$1"
shift
key=""
manifest=""
signature=""
artifacts=()
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --key) [[ "$#" -ge 2 ]] || usage; key="$2"; shift 2 ;;
    --manifest) [[ "$#" -ge 2 ]] || usage; manifest="$2"; shift 2 ;;
    --signature) [[ "$#" -ge 2 ]] || usage; signature="$2"; shift 2 ;;
    --) shift; artifacts+=("$@"); break ;;
    -*) usage ;;
    *) artifacts+=("$1"); shift ;;
  esac
done
[[ -n "$key" && -n "$manifest" && "${#artifacts[@]}" -gt 0 ]] || usage
[[ "$mode" == create || "$mode" == verify ]] || usage
if [[ "$mode" == verify && -z "$signature" ]]; then
  usage
fi
[[ -f "$key" ]] || { printf 'release-attest: key not found: %s\n' "$key" >&2; exit 1; }

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    printf '%s\n' 'release-attest: need sha256sum or shasum' >&2
    exit 1
  fi
}

manifest_tmp="$manifest.tmp.$$"
trap 'rm -f "$manifest_tmp"' EXIT
mkdir -p "$(dirname "$manifest")"
: > "$manifest_tmp"
for artifact in "${artifacts[@]}"; do
  [[ -f "$artifact" ]] || {
    printf 'release-attest: artifact not found: %s\n' "$artifact" >&2
    exit 1
  }
  name="$(basename "$artifact")"
  [[ "$name" != *' '* ]] || {
    printf 'release-attest: artifact name contains whitespace: %s\n' "$name" >&2
    exit 1
  }
  printf '%s  %s\n' "$(hash_file "$artifact")" "$name" >> "$manifest_tmp"
done
LC_ALL=C sort -k2,2 -k1,1 "$manifest_tmp" -o "$manifest_tmp"
duplicates="$(awk '{print $2}' "$manifest_tmp" | uniq -d)"
[[ -z "$duplicates" ]] || {
  printf 'release-attest: duplicate artifact basename: %s\n' "$duplicates" >&2
  exit 1
}

if [[ "$mode" == create ]]; then
  [[ -z "$signature" ]] || {
    printf '%s\n' 'release-attest: --signature is only used by verify' >&2
    exit 2
  }
  mv "$manifest_tmp" "$manifest"
  openssl dgst -sha256 -sign "$key" -out "$manifest.sig" "$manifest" >/dev/null 2>&1
  printf '%s\n' "created $manifest and $manifest.sig"
  exit 0
fi

[[ -f "$manifest" && -f "$signature" ]] || {
  printf '%s\n' 'release-attest: manifest or signature not found' >&2
  exit 1
}
openssl dgst -sha256 -verify "$key" -signature "$signature" "$manifest" >/dev/null 2>&1 || {
  printf '%s\n' 'release-attest: signature verification failed' >&2
  exit 1
}
if ! cmp -s "$manifest_tmp" "$manifest"; then
  printf '%s\n' 'release-attest: checksum manifest does not match artifacts' >&2
  exit 1
fi
printf '%s\n' "verified $manifest"
