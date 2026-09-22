#!/usr/bin/env bash
# M13.9: artifact identity includes version, platform, libc, variant, format.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
name_script="$ROOT/scripts/release-artifact-name.sh"
release="$ROOT/.github/workflows/release.yml"

workspace_version="$(awk '
  { sub(/\r$/, "") }
  $0 == "[workspace.package]" { section = 1; next }
  section && /^\[/ { exit }
  section && /^version = "/ {
    line = $0
    sub(/^version = "/, "", line)
    sub(/"$/, "", line)
    print line
    exit
  }
' "$ROOT/Cargo.toml")"

test -x "$name_script"
default_name="$(bash "$name_script" linux x86_64 glibc2.28 default bin)"
fips_name="$(bash "$name_script" linux x86_64 glibc2.28 fips bin)"
archive_name="$(bash "$name_script" linux aarch64 glibc2.28 default tar.gz)"

[[ "$default_name" == "oqueue-$workspace_version-linux-x86_64-glibc2.28-default-bin" ]]
[[ "$fips_name" == "oqueue-$workspace_version-linux-x86_64-glibc2.28-fips-bin" ]]
[[ "$archive_name" == "oqueue-$workspace_version-linux-aarch64-glibc2.28-default-tar.gz" ]]
[[ "$default_name" != "$fips_name" ]]

invalid_cases=(
  'macos x86_64 glibc2.28 default bin'
  'linux riscv64 glibc2.28 default bin'
  'linux x86_64 glibc2.27 default bin'
  'linux x86_64 glibc2.28 debug bin'
  'linux x86_64 glibc2.28 default zip'
)
for invalid in "${invalid_cases[@]}"; do
  read -r -a fields <<<"$invalid"
  if bash "$name_script" "${fields[@]}" >/dev/null 2>&1; then
    printf 'release-artifact-name accepted invalid identity: %s\n' "$invalid" >&2
    exit 1
  fi
done

job_block() {
  local job="$1"
  awk -v wanted="  $job:" '
    $0 == wanted { in_job = 1 }
    in_job && NR > 1 && $0 ~ /^  [a-z][a-z0-9-]*:/ && $0 != wanted { exit }
    in_job { print }
  ' "$release"
}
x86="$(job_block linux-x86_64)"
arm="$(job_block linux-aarch64)"
grep -Fq 'scripts/release-artifact-name.sh linux x86_64 glibc2.28 default bin' <<<"$x86"
grep -Fq 'name: ${{ env.ARTIFACT_NAME }}' <<<"$x86"
grep -Fq 'scripts/release-artifact-name.sh linux aarch64 glibc2.28 default bin' <<<"$arm"
grep -Fq 'name: ${{ env.ARTIFACT_NAME }}' <<<"$arm"

printf '%s\n' 'release-artifact-name: ok'
