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
# ⚠️ **`SKIP=check-reviewed` here, and it is about `--all-files` rather than
# about the container.** `M10.24` bind-mounts the host's `target/review` in, so
# the verdicts *are* visible now — but `gate` runs the whole suite against the
# tree rather than against a staged change, and `check-reviewed` answers a
# question about staged bytes. With nothing staged it skips anyway; with
# something staged it would report on a diff `gate` never looked at. CI skips
# it for the older reason: there are no verdicts in a runner at all.
#
# ⚠️ **The commit hook comes through here too** (`M10.24`): `.githooks/pre-commit`
# runs `pre-commit run --hook-stage pre-commit` through this script, so
# `check-crate`, `check-coverage` and `check-mutants` — `cargo test`, an
# instrumented `cargo llvm-cov`, and cargo-mutants — are inside the caps rather
# than on the host. That is what the extra mounts and the `--user` flag below
# are all for: a commit-path run writes to `.git` and reads the developer's
# git configuration, and a `--all-files` gate run does neither.
if [ "${1:-}" = "gate" ]; then
  # ⚠️ Refused rather than ignored: `docker-test.sh gate check-crate` is the
  # obvious guess given `pre-commit run check-crate`, and silently running the
  # whole suite tells the developer one hook passed when seventeen did.
  if [ $# -gt 1 ]; then
    echo "docker-test: 'gate' takes no arguments (got: ${*:2})" >&2
    echo "docker-test: for one hook, run: scripts/docker-test.sh pre-commit run <id> --all-files" >&2
    exit 1
  fi
  # ⚠️ Appended to the caller's `SKIP` rather than replacing it: the variable
  # is forwarded into the container now, and `env SKIP=...` here would discard
  # what the caller asked for while running the suite anyway.
  CMD=(env SKIP="check-reviewed${SKIP:+,$SKIP}" pre-commit run --all-files)
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


# ⚠️ **As the invoking user, not root, and `M10.24` is why.** The commit path
# runs `pre-commit run --hook-stage pre-commit`, whose first act is
# `staged_files_only` — `git write-tree` against the bind-mounted `.git`. As
# root that writes root-owned loose objects and fan-out directories into the
# host repository; measured, seven of them, after three hook runs. This clone
# survived only because all 256 fan-outs already existed. A fresh clone has
# none, so the first contained commit creates them root-owned and the
# developer's next `git add` fails with "insufficient permission for adding an
# object" until somebody runs `chown -R`.
#
# ⚠️ The volumes are created root-owned by docker, so they are chowned once,
# below, or nothing the container builds can be written.
HOST_UID="$(id -u)"; HOST_GID="$(id -g)"

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
# artifacts do not mix and a host and a container build never fight over one
# lock.
#
# ⚠️ **One exception, bind-mounted back over the volume: `target/review`.**
# `M10.24` put the commit hook in here, and `check-reviewed` is a pre-commit
# gate that reads the verdict `scripts/review.sh` wrote on the host — non-
# negotiable 4, the one rule enforced by the local hook alone. Without this
# mount the contained hook would find an empty directory and refuse every
# commit. The verdicts are small JSON and the review path stays on the host,
# so this shares the artifact rather than moving the work.
docker volume create oqueue-cargo-registry >/dev/null
docker volume create oqueue-target >/dev/null
# ⚠️ Created host-side first: a bind source docker has to invent is created
# root-owned, and a root-owned `target/review` breaks `review.sh` afterwards.
# ⚠️ `target/seeds` for the same reason as `target/review` (`M10.11`): the
# sweep prints the path it filed a failing seed at, and inside the named volume
# that path is empty on the host — a developer follows the message and
# concludes nothing was filed.
mkdir -p "$REPO/target/review" "$REPO/target/seeds" "$REPO/target/pre-commit-home"

# ⚠️ **Asked, not remembered.** A named volume is created root-owned, and with
# `--user` below an unwritable one makes every build fail on a permission error
# naming a path inside the container and nothing else. ⚠️ **The probe is
# `test -w` as the invoking uid, not a marker file**: a marker records that a
# chown happened and not *for whom*, so a second developer on the same host, or
# the same one after a uid change, skipped the chown and got EACCES from every
# cargo invocation — the failure this is here to prevent.
#
# ⚠️ **One container, not one per volume** (`ADR-0029`): the common case —
# both volumes already owned by this uid, true on every run after the first —
# used to pay a full container start twice for a `test -w` each, measured at
# ~380 ms apiece against this image. Checking both mount points in one run
# halves that; the fallback below still chowns each volume that actually
# needs it, so a first-run or uid-changed host is no worse off than before.
if ! docker run --rm -u "$HOST_UID:$HOST_GID" \
      -v oqueue-cargo-registry:/a -v oqueue-target:/b "$IMAGE" \
      sh -c 'test -w /a && test -w /b' 2>/dev/null; then
  # ⚠️ **Re-probed per volume here, not chowned unconditionally.** The
  # combined check above only says *one or both* failed, and a blanket
  # `chown -R` on a volume that was already fine pays a recursive walk over
  # however many gigabytes of cached crates or build output it holds — found
  # by review, which built two throwaway volumes, root-owned one, and
  # measured the unconditional version re-chowning the other for nothing.
  for v in oqueue-cargo-registry oqueue-target; do
    if ! docker run --rm -u "$HOST_UID:$HOST_GID" -v "$v:/v" "$IMAGE" \
          test -w /v 2>/dev/null; then
      docker run --rm -u 0 -v "$v:/v" "$IMAGE" \
        chown -R "$HOST_UID:$HOST_GID" /v >/dev/null
    fi
  done
fi

# ⚠️ **A unique id per invocation, because a container's pgid is always 1.**
# `lib.sh` keys a timing row by process group and `check-budget.sh` groups on
# it. Inside a PID namespace every run is pgid 1, and `target/timings` lives on
# a volume that outlives `--rm` — so rows from an earlier contained run are
# charged to the next one. Measured: three single-hook runs, then a hook that
# reported 19586 ms over a 10000 ms budget across 20 gates with two counted
# twice, and refused the commit. `lib.sh` prefers this when it is set and falls
# back to the pgid, so a native run is unchanged.
RUN_ID="oq-$$-$(date +%s%N 2>/dev/null || date +%s)"

# ⚠️ **`PRE_COMMIT_HOME` on a host path, and this one is about not losing
# work.** `pre-commit run --hook-stage pre-commit` stashes unstaged changes to
# a patch under its cache and reapplies them in a `finally`. Left on the
# container's writable layer that patch dies with `--rm` — so a container the
# kernel OOM-kills, which is the event this script exists to cause, takes the
# developer's uncommitted edits with it and leaves nothing to recover from.
#
# ⚠️ **And the user's git configuration**, because `check-reviewed` recomputes
# a hash `review.sh` computed on the host. `core.abbrev`, `diff.noprefix`,
# `diff.context` and `diff.algorithm` all change `git diff --cached` output;
# measured, four different hashes for one staged tree. Without this a developer
# with any of them set records a verdict under one hash and the contained gate
# looks for another, and every commit is refused for a reason the error cannot
# name.
# ⚠️ **The host's *resolved* global config, forwarded as values — not its
# files, mounted.** `check-reviewed` recomputes a hash `review.sh` computed on
# the host, and `core.abbrev`, `diff.noprefix`, `diff.context` and
# `diff.algorithm` all change `git diff --cached`; measured, four hashes for
# one staged tree. Three rounds of review were spent trying to mirror the
# *locations* git reads and each version missed one: `~/.gitconfig` only, then
# an `if/elif` that chose one file where git merges two, then both files but
# not `include.path` or `includeIf`, which resolve to a third file nothing
# mounted. ⚠️ **`git config --global --list --includes` ends that class**: it
# is what git itself resolves to, includes followed, and
# `GIT_CONFIG_COUNT`/`GIT_CONFIG_KEY_n`/`GIT_CONFIG_VALUE_n` is git's own
# documented way to carry it. No mount, no precedence to re-implement.
#
# ⚠️ **`--show-scope`, not `--global`**, and the difference is a case review
# caught: with both `~/.gitconfig` and `~/.config/git/config` present,
# `--global --list` reports only the first while git *reads* both — so
# `diff.noprefix` set in the XDG file was still missing inside. `--show-scope`
# reports what git actually resolved, and `local`/`worktree`/`command` are
# dropped because `.git/config` is on the bind mount and read inside exactly as
# it is outside.
GITCONFIG_ARGS=()
_gc_n=0
while IFS='=' read -r _gc_k _gc_v; do
  [ -n "$_gc_k" ] || continue
  GITCONFIG_ARGS+=(-e "GIT_CONFIG_KEY_${_gc_n}=$_gc_k" -e "GIT_CONFIG_VALUE_${_gc_n}=$_gc_v")
  _gc_n=$(( _gc_n + 1 ))
done <<EOF
$(git config --list --show-scope --includes 2>/dev/null \
    | grep -vE '^(local|worktree|command)	' | cut -f2- || true)
EOF
# ⚠️ **`safe.directory` rides in the same list** rather than through
# `GIT_CONFIG_PARAMETERS`: `GIT_CONFIG_COUNT` and that variable are both read,
# but keeping one mechanism means one place to look when the container's git
# disagrees with the host's.
GITCONFIG_ARGS+=(-e "GIT_CONFIG_KEY_${_gc_n}=safe.directory" -e "GIT_CONFIG_VALUE_${_gc_n}=/work")
_gc_n=$(( _gc_n + 1 ))
GITCONFIG_ARGS+=(-e "GIT_CONFIG_COUNT=$_gc_n")

# `-it` only when there is a terminal: an agent or a CI job invokes this with
# no TTY, and `docker run -it` fails outright there rather than degrading.
TTY_FLAGS=()
[ -t 0 ] && [ -t 1 ] && TTY_FLAGS=(-it)

exec docker run --rm ${TTY_FLAGS[@]+"${TTY_FLAGS[@]}"} \
  --user "$HOST_UID:$HOST_GID" \
  --memory="$MEM" --memory-swap="$MEM" \
  --cpus="$CPUS" --pids-limit="$PIDS" \
  --tmpfs "/tmp:rw,exec,size=$TMPFS,mode=1777" \
  -v "$REPO:/work" \
  -v oqueue-cargo-registry:/usr/local/cargo/registry \
  -v oqueue-target:/work/target \
  -v "$REPO/target/review:/work/target/review" \
  -v "$REPO/target/seeds:/work/target/seeds" \
  -v "$REPO/target/pre-commit-home:$REPO/target/pre-commit-home" \
  ${GITCONFIG_ARGS[@]+"${GITCONFIG_ARGS[@]}"} \
  -e HOME=/tmp/home \
  -e PRE_COMMIT_HOME="$REPO/target/pre-commit-home" \
  -e OQUEUE_RUN_ID="$RUN_ID" \
  ${OQUEUE_SEED:+-e OQUEUE_SEED="$OQUEUE_SEED"} \
  ${SWEEP_SEEDS:+-e SWEEP_SEEDS="$SWEEP_SEEDS"} \
  ${SKIP:+-e SKIP="$SKIP"} \
  -e CARGO_BUILD_JOBS="$CARGO_JOBS" \
  -w /work \
  "$IMAGE" \
  "${CMD[@]}"
