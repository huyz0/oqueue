#!/usr/bin/env bash
# Emit the trusted identity for one shipped Linux release artifact.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if [[ "$#" -ne 5 ]]; then
  printf '%s\n' \
    'usage: release-artifact-name.sh <os> <arch> <libc> <variant> <format>' >&2
  exit 2
fi

os="$1"
arch="$2"
libc="$3"
variant="$4"
format="$5"
version="$(awk '
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

[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'release-artifact-name: invalid workspace version %s\n' "$version" >&2
  exit 1
}
[[ "$os" == linux ]] || {
  printf 'release-artifact-name: only Linux artifacts are shipped\n' >&2
  exit 1
}
case "$arch" in
  x86_64|aarch64) ;;
  *) printf 'release-artifact-name: unsupported Linux architecture %s\n' "$arch" >&2; exit 1 ;;
esac
[[ "$libc" == glibc2.28 ]] || {
  printf 'release-artifact-name: unsupported libc identity %s\n' "$libc" >&2
  exit 1
}
case "$variant" in
  default|fips) ;;
  *) printf 'release-artifact-name: unsupported artifact variant %s\n' "$variant" >&2; exit 1 ;;
esac
case "$format" in
  bin|tar.gz) ;;
  *) printf 'release-artifact-name: unsupported artifact format %s\n' "$format" >&2; exit 1 ;;
esac

printf 'oqueue-%s-%s-%s-%s-%s-%s\n' \
  "$version" "$os" "$arch" "$libc" "$variant" "$format"
