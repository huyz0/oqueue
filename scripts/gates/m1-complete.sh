#!/usr/bin/env bash
# M1's own completion condition. `M1.21`.
#
#   scripts/gates/m1-complete.sh
#
# ## What "complete" means here
#
# `m-1-complete.sh` asks a question about *documents*; `m0-complete.sh` asks
# one about a *workspace*. M1's question is about a **seam and its backends**:
# does the conformance suite actually pass against the in-memory fake and
# against MinIO, and does the recorded matrix say so honestly -- including
# about the backends it has *not* run against.
#
# `milestones/M1.md`'s completion condition, in as many words: "Asserts the
# conformance suite passes against the in-memory fake and against MinIO, and
# records which backends it has been run against."
#
# ## ⚠️ Why this starts a container instead of trusting the artifact
#
# The roster under `target/conformance/` is gitignored and written by a test
# run. Reading it without re-running the suite would assert only that somebody
# once ran something -- the "fact that was true once" shape `m0-complete.sh`'s
# own header warns about, and the exact failure `M1.21` found twice while
# writing this file:
#
#   1. `tests/it/s3_minio.rs` said "CI's T2 step starts MinIO and sets every
#      AWS_* variable" -- there is no T2 step in `.github/workflows/gates.yml`
#      and never was. ⚠️ Not that those tests had never run: `M1.15` and
#      `M1.16` record running them against real MinIO by hand. Nothing
#      *re-ran* them, which is what a gate is for.
#   2. The conformance suite passed against MinIO once and failed on the
#      second run: `conditional_write_if_absent` asserted a key was absent and
#      then created it without cleaning up, so it was green only against a
#      virgin bucket.
#
# Neither is visible to a script that reads a file. Both are visible to one
# that runs the suite twice, which is what this does.
#
# ## ⚠️ Where this runs, and why not in pre-commit
#
# **Standalone, at the milestone boundary** -- same as `m0-complete.sh` and
# `m-1-complete.sh`. NFR-56 gives the whole pre-commit suite 10 s; this pulls
# a container image and runs an integration suite twice.
#
# ## A missing Docker is a skip, never a pass
#
# The same discipline `check-mutants.sh` states for a missing `cargo-mutants`.
# A developer without Docker gets an honest "this gate did not run", not a
# green tick for a backend nothing talked to.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

MATRIX="baselines/conformance-matrix.txt"
ROSTER="target/conformance/backends.txt"
CONTAINER="oqueue-m1-complete-minio"
# ⚠️ Pinned, not `:latest`. `docker run` does not re-pull an image that is
# already local, so `:latest` means two machines can run different MinIO
# builds while both record `s3  verified` — and the day a MinIO release
# changes conditional-write behaviour, this gate goes red on an unchanged
# tree and reads exactly like an `S3Store` regression.
MINIO_IMAGE="minio/minio:RELEASE.2025-04-22T22-12-26Z"
MC_IMAGE="minio/mc:RELEASE.2025-04-16T18-13-26Z"
BUCKET="oqueue-conformance"

cleanup() {
  if [[ -n "${STARTED_CONTAINER:-}" ]]; then
    docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

if ! has_rust; then
  skip "M1 completion (no Cargo.toml yet)"
  finish
fi
require_tool cargo "install Rust via https://rustup.rs" || finish

if [[ ! -f "$MATRIX" ]]; then
  fail "$MATRIX not found -- the recorded backend matrix is the artifact this gate checks"
  finish
fi

# ---------------------------------------------------------------------------
# 1. The fake, at T1. No container, no network.
# ---------------------------------------------------------------------------
rm -f "$ROSTER"
if cargo test -p oqueue-store --test it >/dev/null 2>&1; then
  ok "conformance suite passes against the in-memory fake"
else
  fail "conformance suite does not pass against the in-memory fake"
  note "run: cargo test -p oqueue-store --test it"
  finish
fi

# ---------------------------------------------------------------------------
# 2. MinIO, at T2. Started here so the gate does not depend on a human having
#    exported the right AWS_* variables into the right shell.
# ---------------------------------------------------------------------------
if ! docker info >/dev/null 2>&1; then
  skip "conformance suite against MinIO (Docker not available)"
  note "⚠️ this gate proved nothing about the S3 backend -- start Docker and re-run"
  note "the matrix's 's3  verified' row is unverified in this run"
  finish
fi

# ⚠️ Checked here rather than discovered at the health poll, and ⚠️ **after**
# the fake half above rather than at the top: `lib.sh`'s header says a missing
# tool is a skip with a named remedy, but skipping the *whole* gate for a tool
# only the container half needs would throw away a check that had already
# passed. Without this, an absent `curl` spends 30 s timing out and then
# reports a perfectly healthy MinIO as broken.
require_tool curl "install curl" || finish

docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
# ⚠️ `127.0.0.1::9000` — loopback, and a **kernel-assigned** port read back
# below. `testing.md` rule 8: no fixed ports. A hardcoded one collides with a
# second gate run or any local service and reports it as "could not start the
# MinIO container". The loopback half matters separately: a bare publish binds
# 0.0.0.0, putting a read-write MinIO with well-known credentials on the LAN.
if ! docker run -d --name "$CONTAINER" -p "127.0.0.1::9000" \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  "$MINIO_IMAGE" server /data >/dev/null 2>&1; then
  fail "could not start the MinIO container"
  finish
fi
STARTED_CONTAINER=1

# ⚠️ `|| true`, and no pipeline. `docker port` exits 1 when it cannot read the
# mapping — a container that started and immediately exited, which is exactly
# when a diagnostic matters — and under `set -e` with `pipefail` that killed
# this script *at this line*, printing one `ok` and nothing else: no FAIL, no
# summary, no remedy, and the guard below unreachable. `portability.md`
# rules 21-22, the same pattern this commit cites for `awk`.
port_out="$(docker port "$CONTAINER" 9000/tcp 2>/dev/null || true)"
MINIO_PORT="$(sed -n '1s/.*://p' <<< "$port_out")"
if [[ -z "$MINIO_PORT" ]]; then
  fail "could not read back the port Docker assigned to MinIO"
  finish
fi

# ⚠️ `portability.md` rule 17: the tooling silently falls back to an x86-64
# image where no arm64 variant exists and runs it under emulation at roughly
# 85% slower, which presents as flaky tests rather than as a portability
# problem. Named, not failed on -- an emulated MinIO is still a correct MinIO,
# and this gate's job is conformance, not speed.
image_arch="$(docker image inspect --format '{{.Architecture}}' "$MINIO_IMAGE" 2>/dev/null || true)"
host_arch="$(uname -m)"
if [[ -z "$image_arch" ]]; then
  # ⚠️ Not "under emulation": nothing established that. An unreadable
  # architecture and a mismatched one are different facts, and saying the
  # second when only the first is known sends a reader after a cause that
  # may not exist.
  note "could not read the MinIO image architecture -- emulation is unruled-out, not established"
else
  case "$host_arch:$image_arch" in
    x86_64:amd64 | aarch64:arm64 | arm64:arm64) ;;
    *) note "⚠️ MinIO image is $image_arch on a $host_arch host -- running under emulation, expect it to be slow" ;;
  esac
fi

# Poll rather than sleep a fixed span. ⚠️ Not because of the pull — `docker
# run -d` above does not return until the image is pulled and the container
# created, so that is already over. This waits for MinIO's own startup, which
# varies with disk and load, and a fixed sleep is either too short (flake) or
# too long (waste) for it.
for _ in $(seq 1 30); do
  if curl -fsS "http://127.0.0.1:$MINIO_PORT/minio/health/live" >/dev/null 2>&1; then
    break
  fi
  sleep 1
done
if ! curl -fsS "http://127.0.0.1:$MINIO_PORT/minio/health/live" >/dev/null 2>&1; then
  fail "MinIO did not become healthy within 30s"
  finish
fi

# ⚠️ `--network container:` rather than `--network host`: this shares the
# MinIO container's own network namespace, reaching port 9000 directly rather
# than the published one, so it works on Docker Desktop (where host
# networking is off by default) and rootless daemons instead of only where
# the daemon shares this kernel. ⚠️ **This step only** — the health poll and
# `AWS_ENDPOINT` below still address this machine's loopback, so a genuinely
# remote `DOCKER_HOST` remains out of scope for the gate as a whole.
if ! docker run --rm --network "container:$CONTAINER" --entrypoint sh "$MC_IMAGE" -c \
  "mc alias set local http://localhost:9000 minioadmin minioadmin >/dev/null &&
   mc mb --ignore-existing local/$BUCKET >/dev/null" >/dev/null 2>&1; then
  fail "could not create the $BUCKET bucket in MinIO"
  finish
fi

export AWS_ENDPOINT="http://127.0.0.1:$MINIO_PORT"
export AWS_ACCESS_KEY_ID=minioadmin
export AWS_SECRET_ACCESS_KEY=minioadmin
export AWS_BUCKET="$BUCKET"
export AWS_REGION=us-east-1
export AWS_ALLOW_HTTP=true

# ⚠️ **Twice, against the same bucket, deliberately.** A conformance suite
# that is green exactly once per bucket is not green -- see this file's
# header. The second run is the one that catches a case which asserts a key
# is absent and then leaves it behind.
for run in 1 2; do
  if ! cargo test -p oqueue-store --test it -- --include-ignored --test-threads=1 \
    >/dev/null 2>&1; then
    fail "conformance suite run $run of 2 failed against MinIO"
    note "a failure on run 2 only means the suite is not idempotent against a"
    note "persistent backend -- some case leaves state its own precondition needs"
    note "run: cargo test -p oqueue-store --test it -- --include-ignored"
    finish
  fi
done
ok "conformance suite passes against MinIO, twice against the same bucket"

# ---------------------------------------------------------------------------
# 3. The matrix and the roster must agree — delegated, so the comparison has
#    negative-suite cases of its own that need neither Docker nor cargo.
#    ⚠️ `build.md` rule 22: a check implemented twice drifts, and the version
#    that matters is whichever one was not run.
# ---------------------------------------------------------------------------
if [[ ! -f "$ROSTER" ]]; then
  fail "$ROSTER was not written -- the suite ran but recorded no backend"
  finish
fi

if bash "$REPO_ROOT/scripts/check-conformance-matrix.sh" --against-roster; then
  ok "M1 completion condition holds"
else
  fail "the recorded backend matrix does not agree with what ran"
  finish
fi
finish
