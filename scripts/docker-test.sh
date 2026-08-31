#!/usr/bin/env bash
# Runs a command against this repository inside a resource-capped container.
#
# ⚠️ **Containment, not convenience.** On WSL2 a runaway `cargo` exhausts the
# VM, which kills the running session and needs a manual `wsl --shutdown` to
# recover — the VM has no per-process ceiling to hit first. Inside these limits
# the kernel kills something in the container instead, and the host is never at
# risk. `M10.23`.
#
#   scripts/docker-test.sh gate                 # pre-commit run --all-files
#   scripts/docker-test.sh                      # cargo test --workspace
#   scripts/docker-test.sh cargo test -p oqueue-core
#   scripts/docker-test.sh scripts/check-coverage.sh
#   scripts/docker-test.sh bash                 # a shell inside it
#
# ⚠️ **CI does not use this.** GitHub Actions runners are already isolated VMs,
# so containing them again would only cost build time; `.github/workflows/
# gates.yml` runs the same commands natively. This script is the *local*
# answer, and the two must not drift into different definitions of the gate —
# which is why `gate` below expands to the same `pre-commit` invocation CI
# runs rather than to a list maintained here.
#
# Env knobs, all with defensible defaults:
#   MEM=12g CPUS=12 PIDS=1024 TMPFS=2g  scripts/docker-test.sh ...
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# ⚠️ **Tagged by the Dockerfile's own hash, so an edited Dockerfile is a
# different image and rebuilds itself.** The obvious form — a fixed tag built
# only when absent — never rebuilds: `docker image inspect` succeeds for any
# stale `oqueue-test`, so a bumped `RUST_VERSION` or an added client would
# leave every machine that already had one silently running the old image,
# while this file asserts what the new one contains.
# ⚠️ **`sha256sum` OR `shasum`**, which is `lib.sh`'s `sha256_stdin` inlined:
# macOS ships only the second, `portability.md` rule 2 makes it a first-class
# development platform, and `review.sh` already records this exact defect being
# fixed once. Inlined rather than sourced because `lib.sh` brings a whole gate
# harness this script has no use for.
if command -v sha256sum >/dev/null 2>&1; then
  DOCKERFILE_SHA="$(sha256sum < "$REPO/docker/Dockerfile" | cut -c1-12)"
elif command -v shasum >/dev/null 2>&1; then
  DOCKERFILE_SHA="$(shasum -a 256 < "$REPO/docker/Dockerfile" | cut -c1-12)"
else
  echo "docker-test: need sha256sum or shasum to tag the image by content" >&2
  exit 1
fi
IMAGE="${IMAGE:-oqueue-test:$DOCKERFILE_SHA}"

# ⚠️ **`--memory-swap` equal to `--memory` disables swap for the container.**
# Without it a runaway allocates into swap and drags the host down slowly
# instead of failing fast, which is the exact behaviour this script exists to
# prevent — a slow degradation is harder to recognise than an OOM kill.
MEM="${MEM:-12g}"
# ⚠️ **12, not 8, and the number was measured rather than chosen.**
# `check-budget.sh` times the pre-commit suite against a 10 s ceiling that
# non-negotiable 2 forbids raising, and the container's CPU share moves that
# measurement: 8 CPUs gave 8153 ms on a warm cache and 9328 ms on a colder one
# — 93% of a ceiling, so the default would have failed the budget
# intermittently and the tempting repair would have been the constant. 12 gives
# 7577-8904 ms over repeated runs and 16 gives 7508 — ⚠️ **the ranges overlap**,
# so 12 buys about a tenth of the ceiling rather than the quarter one pair of
# readings suggested, and 16 is not worth the host's cores. 12 on that basis,
# and it still leaves this 20-CPU host 8.
CPUS="${CPUS:-12}"
# ⚠️ **`docker --cpus` takes a fraction and `cargo --jobs` does not.** Passing
# `CPUS=1.5` straight through made every cargo invocation in the container die
# with "Number of parallel jobs should be `default` or a number", naming
# neither this script nor the knob the developer set.
case "$CPUS" in
  *[!0-9.]* | '' ) echo "docker-test: CPUS must be a number, got '$CPUS'" >&2; exit 1 ;;
esac
CARGO_JOBS="${CPUS%%.*}"
[ -n "$CARGO_JOBS" ] && [ "$CARGO_JOBS" != "0" ] || CARGO_JOBS=1
PIDS="${PIDS:-1024}"
# ⚠️ **`--tmpfs` is what makes `/tmp` RAM at all** — a container's `/tmp` is
# otherwise the overlay filesystem — so this both bounds it and charges it
# against `--memory` above. `tests/gates/negative.sh` creates a scratch git
# repository per case through `mktemp -d`; those hold *sources*, because both
# compiling scaffolds set `target-dir` under `target/tmp/` for `build.md`
# rule 19, and `mutants.sh` redirects `TMPDIR` to `target/mutants-tmp` for the
# same reason. So the bound is cheap insurance against something new writing
# there, not a fix for a known consumer — a distinction worth keeping, or the
# next reader raises it hunting a build-output problem the fixtures prevent.
#
# ⚠️ **`exec` in the mount options below is load-bearing**, and `docker run`
# does not default to it: a `--tmpfs` is mounted `noexec` unless told
# otherwise. Every `negative.sh` fixture writes a script into its scratch
# directory and runs it, so without `exec` they exit 126 — "found, not
# executable" — and the case fails for a reason that has nothing to do with
# what it plants. The suite caught this on the first containerised run, which
# is the whole argument for `testing.md` rule 20a.
TMPFS="${TMPFS:-2g}"

# ⚠️ **`gate` expands to the same command CI runs**, so the definition lives in
# `.pre-commit-config.yaml` alone and a developer cannot run a subset of it by
# accident.
#
# ⚠️ **`SKIP=check-reviewed`, for the reason CI skips it and no other.** A
# review verdict is written to `target/review/`, which is gitignored, and the
# container's `target/` is a named volume rather than the host's — so the
# verdict recorded on the host is not visible here. Non-negotiable 4 is
# enforced by the local commit hook, which runs on the host and does see it.
#
# ⚠️ **The commit hook is NOT contained, and that is a gap rather than a
# design.** This said reviewing and committing "need no toolchain", which is
# false and was the wrong reason for a true sentence: `check-crate`,
# `check-coverage` and `check-mutants` are all `stages: [pre-commit]`, so every
# `git commit` runs `cargo test`, a full instrumented `cargo llvm-cov` rebuild
# and cargo-mutants on the **host**, uncontained — the heaviest recurring cargo
# path in the repository, and exactly the hazard this script exists for.
# `M10.24` owns closing it; it needs the CI interaction decided first, because
# the same hooks run in Actions where a container would only cost build time.
# ⚠️ **And running the gate here first does not fix it** — this said the hook
# would "re-run it warm", which is false for the reason twenty lines below:
# the container's `target/` is a named volume, so a container run leaves the
# host's untouched and the hook still builds cold. What a container run buys is
# knowing the gate *passes* before paying for it uncontained. That is worth
# something and it is not containment.
if [ "${1:-}" = "gate" ]; then
  # ⚠️ Refused rather than ignored: `docker-test.sh gate check-crate` is the
  # obvious guess given `pre-commit run check-crate`, and silently running the
  # whole suite tells the developer one hook passed when seventeen did.
  if [ $# -gt 1 ]; then
    echo "docker-test: 'gate' takes no arguments (got: ${*:2})" >&2
    echo "docker-test: for one hook, run: scripts/docker-test.sh pre-commit run <id> --all-files" >&2
    exit 1
  fi
  CMD=(env SKIP=check-reviewed pre-commit run --all-files)
elif [ $# -eq 0 ]; then
  # ⚠️ **Not `"${@:-cargo test --workspace}"`**, which is what this was and
  # which never worked: with no positional parameters that expansion yields a
  # *single* field, so the container was handed one argv word named
  # "cargo test --workspace" and exited 127. Every other documented form was
  # exercised; the no-argument one was not, until review ran it.
  CMD=(cargo test --workspace)
else
  CMD=("$@")
fi

# ⚠️ **The Dockerfile's `RUST_VERSION` is a second copy of a fact
# `rust-toolchain.toml` owns**, and `build.md` rule 1 is about exactly that —
# `.github/workflows/gates.yml` names no version for this reason. It cannot be
# read from the file at build time (a `docker build` context holds only
# `docker/`), so it is checked here instead: a bump to one and not the other
# means rustup silently downloads a second toolchain on the first `cargo` call,
# over a network the image was built not to need.
want="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$REPO/rust-toolchain.toml" | head -1)"
have="$(sed -n 's/^ARG RUST_VERSION=\(.*\)/\1/p' "$REPO/docker/Dockerfile" | head -1)"
if [ -n "$want" ] && [ "$want" != "$have" ]; then
  echo "docker-test: docker/Dockerfile pins Rust $have, rust-toolchain.toml pins $want" >&2
  echo "docker-test: bump both together -- they are one fact in two files" >&2
  exit 1
fi

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  echo "docker-test: building $IMAGE (first run only, several minutes)" >&2
  docker build -t "$IMAGE" -f "$REPO/docker/Dockerfile" "$REPO/docker"
fi

# ⚠️ **Superseded images are named, never deleted here.** Content-tagging means
# every Dockerfile edit builds a new ~4 GB image and strands the old one, and on
# WSL2 the ext4 vhdx does not shrink when they are finally pruned — so the
# containment story acquires a disk-exhaustion route if nobody says anything.
# ⚠️ Printing rather than pruning because an image is cheap to rebuild and
# expensive to lose mid-task, and because deleting things the caller did not ask
# about is not this script's business.
stale="$(docker images --format '{{.Repository}}:{{.Tag}}' oqueue-test 2>/dev/null | grep -v "^${IMAGE}$" || true)"
if [ -n "$stale" ]; then
  echo "docker-test: $(printf '%s\n' "$stale" | wc -l | tr -d ' ') superseded image(s) from earlier Dockerfiles:" >&2
  printf '%s\n' "$stale" | sed 's/^/  /' >&2
  echo "docker-test: reclaim with: docker rmi $(printf '%s ' $stale)" >&2
fi

# Named volumes for the cargo registry and the target directory. Without them
# every run recompiles the world, which is itself a memory spike worth
# avoiding.
#
# ⚠️ **The target volume is mounted at `/work/target`, and `CARGO_TARGET_DIR`
# is deliberately NOT exported.** An environment `CARGO_TARGET_DIR` overrides a
# `.cargo/config.toml` `[build] target-dir`, which is how
# `tests/gates/negative.sh`'s scratch fixtures satisfy `build.md` rule 19 —
# both `check-coverage.sh` and `mutants.sh` carry a comment recording that
# exact defect being found by review, in each case after someone exported it
# for a reason as good as this one. Mounting the volume where the scripts
# already look costs nothing and reintroduces nothing.
#
# ⚠️ The consequence is that the container's `target/` is **not** the host's:
# artifacts do not mix, a host and a container build never fight over one lock,
# and `target/review` inside here is empty (see `SKIP` above).
docker volume create oqueue-cargo-registry >/dev/null
docker volume create oqueue-target >/dev/null

# `-it` only when there is a terminal: an agent or a CI job invokes this with
# no TTY, and `docker run -it` fails outright there rather than degrading.
TTY_FLAGS=()
[ -t 0 ] && [ -t 1 ] && TTY_FLAGS=(-it)

exec docker run --rm "${TTY_FLAGS[@]}" \
  --memory="$MEM" --memory-swap="$MEM" \
  --cpus="$CPUS" --pids-limit="$PIDS" \
  --tmpfs "/tmp:rw,exec,size=$TMPFS,mode=1777" \
  -v "$REPO:/work" \
  -v oqueue-cargo-registry:/usr/local/cargo/registry \
  -v oqueue-target:/work/target \
  -e CARGO_BUILD_JOBS="$CARGO_JOBS" \
  -w /work \
  "$IMAGE" \
  "${CMD[@]}"
