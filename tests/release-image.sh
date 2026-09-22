#!/usr/bin/env bash
# M13.12: runtime images must contain and execute the supplied release binary.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
script="$ROOT/scripts/release-image.sh"
dockerfile="$ROOT/docker/release-runtime.Dockerfile"
test -x "$script"
test -f "$dockerfile"
grep -Fq 'COPY oqueue /usr/local/bin/oqueue' "$dockerfile"
grep -Fq 'ENTRYPOINT ["/usr/local/bin/oqueue"]' "$dockerfile"

scratch="$ROOT/target/tmp/release-image-test.$$"
mkdir -p "$scratch/bin"
trap 'rm -rf "$scratch"' EXIT

artifact="$scratch/oqueue-0.0.0-linux-x86_64-glibc2.28-default-bin"
printf '%s\n' 'release binary bytes' > "$artifact"

hash_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  elif command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{print $1}'
  else
    printf '%s\n' 'release-image test: need sha256sum or shasum' >&2
    exit 1
  fi
}

artifact_sha256="$(hash_file "$artifact")"

log="$scratch/docker.log"
cat > "$scratch/bin/docker" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "${OQUEUE_DOCKER_LOG:?}"
case "$1 ${2:-}" in
  build\ *)
    context="${!#}"
    : > "${OQUEUE_CONTEXT_FILES:?}"
    for entry in "$context"/*; do
      [[ -e "$entry" ]] || continue
      basename "$entry"
    done | LC_ALL=C sort > "${OQUEUE_CONTEXT_FILES:?}"
    exit 0
    ;;
  image\ inspect) printf 'sha256:image %s linux\n' "${OQUEUE_IMAGE_ARCH:-amd64}" ;;
  run\ *)
    if [[ "$*" == *"sha256sum /usr/local/bin/oqueue"* ]]; then
      printf '%s\n' "${OQUEUE_IMAGE_HASH:?}  /usr/local/bin/oqueue"
    else
      printf '%s\n' 'oqueue 0.0.0'
    fi
    ;;
  *) printf 'unexpected docker command: %s\n' "$*" >&2; exit 1 ;;
esac
STUB
chmod +x "$scratch/bin/docker"

context_files="$scratch/context-files"
PATH="$scratch/bin:$PATH" OQUEUE_DOCKER_LOG="$log" OQUEUE_CONTEXT_FILES="$context_files" \
  OQUEUE_M13_EVIDENCE_DIR="$scratch/evidence" OQUEUE_IMAGE_HASH="$artifact_sha256" \
  bash "$script" --artifact "$artifact" --tag oqueue-runtime:test --expected-architecture amd64

grep -Fq 'M13_RELEASE_IMAGE status=pass architecture=amd64 startup=pass binary_match=pass' \
  "$scratch/evidence/image.tsv"
[[ "$(<"$context_files")" == oqueue ]]
grep -Fq 'build --platform linux/amd64' "$log"
grep -Fq 'image inspect --format' "$log"
grep -Fq 'run --rm --entrypoint /bin/sh oqueue-runtime:test' "$log"
grep -Fq 'run --rm oqueue-runtime:test' "$log"
if grep -Eq 'cargo|git|\.github|Cargo\.toml' "$log"; then
  printf '%s\n' 'release-image introduced a source-build path' >&2
  exit 1
fi

for expected in \
  'scripts/release-image.sh --artifact "target/${RELEASE_TARGET}.2.28/release/oqueue"' \
  '--expected-architecture amd64' \
  '--expected-architecture arm64'; do
  grep -Fq -- "$expected" "$ROOT/.github/workflows/release.yml"
done

mkdir -p "$scratch/reused/context"
printf '%s\n' stale > "$scratch/reused/context/stale"
PATH="$scratch/bin:$PATH" OQUEUE_DOCKER_LOG="$log" OQUEUE_CONTEXT_FILES="$context_files" \
  OQUEUE_M13_EVIDENCE_DIR="$scratch/reused" OQUEUE_IMAGE_HASH="$artifact_sha256" \
  bash "$script" --artifact "$artifact" --tag oqueue-runtime:test --expected-architecture amd64 >/dev/null
[[ "$(<"$context_files")" == oqueue ]]

PATH="$scratch/bin:$PATH" OQUEUE_DOCKER_LOG="$log" OQUEUE_CONTEXT_FILES="$context_files" \
    OQUEUE_IMAGE_ARCH=arm64 OQUEUE_M13_EVIDENCE_DIR="$scratch/arm64" \
    OQUEUE_IMAGE_HASH="$artifact_sha256" \
    bash "$script" --artifact "$artifact" --tag oqueue-runtime:test-arm64 --expected-architecture arm64
grep -Fq 'M13_RELEASE_IMAGE status=pass architecture=arm64 startup=pass binary_match=pass' \
  "$scratch/arm64/image.tsv"

if PATH="$scratch/bin:$PATH" OQUEUE_DOCKER_LOG="$log" OQUEUE_CONTEXT_FILES="$context_files" \
    OQUEUE_IMAGE_ARCH=amd64 OQUEUE_M13_EVIDENCE_DIR="$scratch/wrong-arch" \
    OQUEUE_IMAGE_HASH="$artifact_sha256" \
    bash "$script" --artifact "$artifact" --tag oqueue-runtime:test --expected-architecture arm64 \
    >/dev/null 2>&1; then
  printf '%s\n' 'release-image accepted a mismatched image architecture' >&2
  exit 1
fi
if PATH="$scratch/bin:$PATH" OQUEUE_DOCKER_LOG="$log" OQUEUE_CONTEXT_FILES="$context_files" \
    OQUEUE_IMAGE_ARCH=amd64 OQUEUE_M13_EVIDENCE_DIR="$scratch/wrong-hash" \
    OQUEUE_IMAGE_HASH=0000000000000000000000000000000000000000000000000000000000000000 \
    bash "$script" --artifact "$artifact" --tag oqueue-runtime:test --expected-architecture amd64 \
    >/dev/null 2>&1; then
  printf '%s\n' 'release-image accepted a different image binary' >&2
  exit 1
fi

printf '%s\n' 'release-image: ok'
