#!/usr/bin/env bash
# Build and exercise a runtime image from an existing release binary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-image.$$}"
mkdir -p "$EVIDENCE_DIR"

usage() {
  printf '%s\n' \
    'usage: release-image.sh [--completion-gate] --artifact PATH --tag TAG --expected-architecture amd64|arm64' >&2
  exit 2
}

artifact=""
tag=""
expected_arch=""
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --completion-gate) shift ;;
    --artifact) [[ "$#" -ge 2 ]] || usage; artifact="$2"; shift 2 ;;
    --tag) [[ "$#" -ge 2 ]] || usage; tag="$2"; shift 2 ;;
    --expected-architecture) [[ "$#" -ge 2 ]] || usage; expected_arch="$2"; shift 2 ;;
    *) usage ;;
  esac
done
[[ -n "$artifact" && -n "$tag" && -n "$expected_arch" ]] || usage
[[ -f "$artifact" ]] || { printf 'release-image: artifact not found: %s\n' "$artifact" >&2; exit 1; }
case "$expected_arch" in
  amd64|arm64) ;;
  *) printf 'release-image: unsupported architecture: %s\n' "$expected_arch" >&2; exit 1 ;;
esac
[[ "$tag" != *[[:space:]]* ]] || { printf '%s\n' 'release-image: tag contains whitespace' >&2; exit 1; }

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    printf '%s\n' 'release-image: need sha256sum or shasum' >&2
    exit 1
  fi
}

docker_exec() {
  if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == mingw* || "${OSTYPE:-}" == cygwin* ]]; then
    MSYS_NO_PATHCONV=1 docker "$@"
  else
    docker "$@"
  fi
}

context="$EVIDENCE_DIR/context"
if [[ -e "$context" ]]; then
  # The context is script-owned evidence state; remove stale entries before copying
  # the one release binary so a rerun cannot silently broaden the Docker build input.
  rm -rf "$context"
fi
mkdir -p "$context"
trap 'rm -rf "$context"' EXIT
cp "$artifact" "$context/oqueue"

artifact_sha256="$(hash_file "$artifact")"
docker_exec build --platform "linux/$expected_arch" \
  --file "$ROOT/docker/release-runtime.Dockerfile" --tag "$tag" "$context" >/dev/null

read -r image_id image_arch image_os < <(
  docker_exec image inspect --format '{{.Id}} {{.Architecture}} {{.Os}}' "$tag"
)
[[ -n "$image_id" && -n "$image_arch" && -n "$image_os" ]] || {
  printf '%s\n' 'release-image: image inspection returned incomplete metadata' >&2
  exit 1
}
[[ "$image_arch" == "$expected_arch" && "$image_os" == linux ]] || {
  printf 'release-image: expected linux/%s, got %s/%s\n' \
    "$expected_arch" "$image_os" "$image_arch" >&2
  exit 1
}

image_sha256="$(docker_exec run --rm --entrypoint /bin/sh "$tag" \
  -c 'sha256sum /usr/local/bin/oqueue' | awk '{print $1}')"
[[ "$image_sha256" == "$artifact_sha256" ]] || {
  printf 'release-image: image binary hash differs from artifact\n' >&2
  exit 1
}
docker_exec run --rm "$tag" >/dev/null

printf '%s\n' \
  "M13_RELEASE_IMAGE status=pass architecture=$image_arch startup=pass binary_match=pass artifact_sha256=$artifact_sha256 image_id=$image_id" \
  > "$EVIDENCE_DIR/image.tsv"
printf '%s\n' "$(<"$EVIDENCE_DIR/image.tsv")"
