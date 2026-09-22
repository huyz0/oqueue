#!/usr/bin/env bash
# Run the release artifact on the oldest supported glibc distribution.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
IMAGE="${OQUEUE_RELEASE_SMOKE_IMAGE:-oqueue-release-smoke:glibc-2.28}"
EVIDENCE_DIR="${OQUEUE_M13_EVIDENCE_DIR:-$ROOT/target/tmp/release-smoke.$$}"
mkdir -p "$EVIDENCE_DIR"

completion_gate=0
output_name=smoke.tsv
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --completion-gate) completion_gate=1; shift ;;
    smoke.tsv) output_name=smoke.tsv; shift ;;
    *) printf '%s\n' 'usage: release-smoke.sh [smoke.tsv] [--completion-gate]' >&2; exit 2 ;;
  esac
done

if [[ "$completion_gate" == 1 ]]; then
  source_evidence="${OQUEUE_M13_SMOKE_EVIDENCE:-}"
  [[ -n "$source_evidence" && -f "$source_evidence" ]] || {
    printf '%s\n' 'release-smoke: completion gate requires native smoke evidence via OQUEUE_M13_SMOKE_EVIDENCE' >&2
    exit 1
  }
  [[ "$(grep -Ec '^M13_RELEASE_SMOKE ' "$source_evidence" || true)" == 1 ]] || {
    printf '%s\n' 'release-smoke: native smoke evidence must contain exactly one smoke record' >&2
    exit 1
  }
  destination="$EVIDENCE_DIR/$output_name"
  if [[ "$source_evidence" != "$destination" ]]; then
    cp "$source_evidence" "$destination"
  fi
  cat "$destination"
  exit 0
fi

docker_exec() {
  if [[ "${OSTYPE:-}" == msys* || "${OSTYPE:-}" == mingw* || "${OSTYPE:-}" == cygwin* ]]; then
    MSYS_NO_PATHCONV=1 docker "$@"
  else
    docker "$@"
  fi
}

cd "$ROOT"
docker_exec build --pull --file docker/release-smoke.Dockerfile --tag "$IMAGE" .

read -r image_id image_arch image_os < <(
  docker_exec image inspect --format '{{.Id}} {{.Architecture}} {{.Os}}' "$IMAGE"
)
case "$(uname -m)" in
  x86_64) expected_arch=amd64 ;;
  aarch64|arm64) expected_arch=arm64 ;;
  *) printf 'release-smoke: unsupported host architecture %s\n' "$(uname -m)" >&2; exit 1 ;;
esac
if [[ "$image_arch" != "$expected_arch" ]]; then
  printf 'release-smoke: expected image architecture %s, got %s\n' \
    "$expected_arch" "$image_arch" >&2
  exit 1
fi
printf '%s\n' "M13_RELEASE_IMAGE id=$image_id architecture=$image_arch expected_arch=$expected_arch os=$image_os" \
  > "$EVIDENCE_DIR/image.tsv"

docker_exec run --rm --entrypoint /usr/bin/ldd "$IMAGE" --version \
  > "$EVIDENCE_DIR/glibc.txt" 2>&1
glibc_line="$(head -n 1 "$EVIDENCE_DIR/glibc.txt")"
if ! grep -Eq 'GNU libc.*2\.28' "$EVIDENCE_DIR/glibc.txt"; then
  printf 'release-smoke: expected glibc 2.28, got %s\n' "$glibc_line" >&2
  exit 1
fi

docker_exec run --rm "$IMAGE" 2>&1 | tee "$EVIDENCE_DIR/role-smoke.log"

printf '%s\n' 'M13_RELEASE_SMOKE status=pass glibc_floor=2.28 request=pass' \
  > "$EVIDENCE_DIR/$output_name"
printf '%s\n' "$(<"$EVIDENCE_DIR/$output_name")"
