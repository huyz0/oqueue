#!/usr/bin/env bash
# Mutation testing, narrowed to what the staged diff touched. `M0.17`.
#
#   scripts/mutants.sh                 every crate, mutants in the staged diff
#   scripts/mutants.sh oqueue-core     one crate, mutants in the staged diff
#   scripts/mutants.sh --full          the whole workspace, unnarrowed (nightly)
#
# ## Why narrowed
#
# `testing.md` rule 16: cost proportional to the **change**, not to the
# codebase. `cargo-mutants` takes `--in-diff`, so the mutants generated are only
# those in lines the diff touched — which is what makes this runnable per commit
# at all. The unnarrowed run over `oqueue-core` alone is 119 mutants and ~23 s
# today, and grows with every crate that acquires code.
#
# ## ⚠️ Why this is a tool and `check-mutants.sh` is the gate
#
# This script *runs* mutants and prints what survived. It does not decide
# whether that is acceptable — `check-mutants.sh` does, against
# `baselines/mutants.txt`. Splitting them is what lets a developer run this
# freely while the gate stays a single, argued decision point.
#
# ## Cost, measured — ⚠️ by mode, cache state *and* where in the dep graph the
# ## changed code sits, because the third one turned out to dominate
#
#   narrowed, diff with no mutants                    0.1 s  (a skip, not a run)
#   narrowed, 1 mutant                                1.2 s
#   narrowed, 13 mutants, warm, `oqueue-core`         4.8 s
#   narrowed, 13 mutants, cold, `oqueue-core`         9.7 s
#   unnarrowed, `-p oqueue-core`, warm               23 s   ⚠️ one leaf crate
#   unnarrowed, the whole workspace              ~6 hours   ⚠️ `M4.32`, 2294 mutants
#   narrowed, 30 mutants, warm, `oqueue-broker`     102 s   ⚠️ `M4.19`, `dev`
#   narrowed, 30 mutants, warm, `oqueue-broker`      33 s   ⚠️ `M4.19`, `mutants`
#   narrowed, 30 mutants, warm, `oqueue-broker`      40 s   ⚠️ `M4.21`, `mutants`, rust-lld
#   narrowed, 30 mutants, warm, `oqueue-broker`      40 s   ⚠️ `M4.21`, `mutants`, mold
#   narrowed, 30 mutants, warm, `oqueue-broker`      68 s   ⚠️ `M4.21`, `mutants`, GNU ld
#
# ⚠️ **The `M4.21` rows name a linker because two of them are configurations
# nothing runs.** `rust-lld` is the row that describes this gate: it is the
# toolchain default on x86-64 Linux from Rust 1.90, this repo pins 1.97.1, and
# no cargo config here passes `-fuse-ld=` — so a run with no linker flag links
# with `ld.lld`, verified from the driver rather than assumed. The other two
# arms had to be forced. ⚠️ **`M4.19`'s 33 s and `M4.21`'s 40 s are the same
# configuration on two days** — ordinary run-to-run spread on this gate, which
# also moved 62 s against 68 s between a cold and a warm run inside a single
# container. Read the table's `oqueue-broker` cost as *tens of seconds, moving
# by about a fifth between runs*, and never support a ratio with one unpaired
# pair of readings.
#
# ⚠️ **The first row is not a cost, it is a skip**, and quoting it as "narrowed
# runs take 0.08 s" is how this script's placement was first argued. Review
# caught that.
#
# ⚠️ **The `23 s` row is one leaf crate, and quoting it as the cost of `--full`
# is how the *full* run's placement was argued** — per-push, for the whole of
# M4, against a workspace pass `M4.32` measured at 2294 mutants over roughly
# six hours. `M4.51` moved it to `.github/workflows/mutants.yml`'s nightly
# schedule. ⚠️ **The six-hour figure is `M4.32`'s, read from its run's log and
# from `baselines/mutants.txt:26`, not re-measured here** — `mutants.out` is
# overwritten by every run and that log is gone. The 2294 is exact; treat the
# wall clock as "hours, not minutes", which is the only part the placement
# turns on.
#
# ⚠️ **The four `oqueue-broker` rows are `M4.19`'s and `M4.21`'s correction,
# and the four rows above them were stale by ~20x for the code this gate
# actually runs on now.** Every earlier row was measured against
# `oqueue-core` — a leaf, sans-I/O crate. The same
# gate over `oqueue-broker`, which sits on top of tokio, rustls and
# `kafka-protocol`, costs **~4 s per mutant**, so cost tracks *what a mutant
# forces a rebuild of*, not the mutant count the earlier rows imply. Phase
# split, measured with `cargo mutants` timings on: unmutated baseline **8 s
# build + 1 s test**, then 30 mutants in ~2 min. ⚠️ **So it is the per-mutant
# rebuild loop, not the baseline**, that this gate is made of.
#
# ⚠️ **`-j` does not fix it — measured and reverted, `M4.19`.** Running the
# identical diff warm at one job and at four: 103 s against 106 s. No gain,
# because each individual mutant rebuild already saturates every core cargo
# was given, so N concurrent jobs each get 1/N of them. ⚠️ Memory was never
# the reason it fails to help (3.53 GiB at `-j 1` against 6.86 GiB at `-j 4`,
# under `docker-test.sh`'s 12 GiB cap) — so anyone reasoning from headroom
# will reach for `-j` again, and this paragraph is why not to.
#
# ⚠️ **What worked instead was making each rebuild cheaper, not running more
# of them at once** — `[profile.mutants]`, below. It is so far the only thing
# that has.
#
# ⚠️ **Installing a faster linker does not help — measured and rejected,
# `M4.21`.** The paragraph above invites the next idea: this gate relinks once
# per mutant, so link time should dominate and mold should halve it. ⚠️ **The
# premise is wrong, and it is wrong because the fast linker is already here.**
# Rust ships `rust-lld` as the x86-64 Linux default from 1.90 and this repo
# pins 1.97.1, so every run of this gate already links with `ld.lld`. Measured
# three ways in one container, warm, identical 30-mutant diff, `mutants`
# profile, each arm's linker read back from the driver: **rust-lld 40 s, mold
# 40 s, GNU ld 68 s**, all three `19 caught, 11 unviable`. mold ties the
# default. What it beats is a linker nothing here uses.
#
# ⚠️ **`docs/researches/18` §3.6 already said so** — "rust-lld is the default
# linker on x86-64 Linux since Rust 1.90 … mold was 0.7% *slower*; don't
# cargo-cult it" — and `M4.21` spent three A/B runs rediscovering it. The
# `research` skill exists to be used before a build-tooling change, not after.
#
# ⚠️ **A 1.97x speedup was claimed for mold first, and the claim survived two
# A/B runs**, which is the more useful half of this comment. In order:
#
#   1. against a target directory on the container's *overlayfs* — mold
#      **slower**, 116 s against 126 s. I/O dominated and masked the link
#      difference. ⚠️ Benchmark this gate on the filesystem it actually runs
#      on, or measure the filesystem instead of the change.
#   2. against the volume — 120 s against 61 s, "1.97x", which is the reading
#      that installed mold in the image. Its baseline arm was labelled "GNU
#      ld", and **the gate's baseline is not GNU ld** — so at best it measured
#      a linker nothing uses against mold, and at worst its arms never
#      differed, because neither was read back from the driver.
#   3. the three-arm run the rows above come from: one container, one
#      volume-backed target directory, every arm spelling out the whole flag
#      set through `CARGO_ENCODED_RUSTFLAGS` so none inherits a config file,
#      each run cold then warm, and each arm's linker **verified from the
#      driver** — `rustc -C link-arg=-Wl,--version` printing `ld.lld`,
#      `GNU ld`, `mold 2.40.4` — before any timing was believed.
#
# ⚠️ **Two mechanical traps are why step 3 is written out.** First: an arm has
# to be *shown* to differ. Second: selecting a linker by editing a config file
# inside the container cannot work from `docker-test.sh` — `$CARGO_HOME` is
# root-owned and the container runs as the invoking uid, so `mv` returns
# `Permission denied` and the arm silently keeps the image's linker, measured
# here on a run that then reported two "different" arms one second apart.
# `CARGO_ENCODED_RUSTFLAGS` avoids both, at the cost of having to restate every
# flag the config would have supplied — `--cfg tokio_unstable` included, since
# the variable *replaces* `[build] rustflags` rather than merging with it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

MODE="narrowed"
CRATE=""
case "${1:-}" in
  --full) MODE="full" ;;
  "") ;;
  -*) fail "unknown argument: $1"; note "usage: scripts/mutants.sh [<crate>|--full]"; finish ;;
  *) CRATE="$1" ;;
esac

if ! has_rust; then
  skip "mutation testing (no Cargo.toml yet)"
  finish
fi
require_tool cargo "install Rust via https://rustup.rs" || finish

if ! cargo mutants --version >/dev/null 2>&1; then
  skip "mutation testing (cargo-mutants not installed)"
  note "install: cargo install cargo-mutants"
  note "⚠️ a missing tool is a skip, never a pass — this did not run"
  finish
fi

# ⚠️ **No `--timeout`, and that is a decision rather than an omission.**
# `testing.md` rule 18 says a tight per-test cap reports a **kill for a mutant
# nothing detected** — silent, and in the worst direction — and asks that the
# cap be sized against the suite under the parallelism a mutation run actually
# uses. `cargo-mutants`' default is derived that way already: it times the
# unmutated baseline under the same parallelism and allows a multiple of it, so
# it moves with the suite instead of being a constant that goes stale — ⚠️
# measured on 27.1.0, `Auto-set test timeout to 20s`, which is the tool's floor
# rather than a multiple, because this suite's baseline is under a second; the
# multiple takes over as the suite grows, which is the direction rule 18 cares
# about. ⚠️ A
# literal here would be one more number nobody has measured — and one neither
# gate would catch going stale: `check-drift.sh` only reports a threshold that
# is *also* environment-settable, and ~~`m0-complete.sh` pins the two constants
# `requirements.md` names~~ — ⚠️ **that leg was already false when written**:
# `M0.23` had put `COMPILING_GATE_MS` in the map with a comment saying it is
# "not a requirement's number and is here anyway". `M1.35` widened the map
# further, to seven entries, five of them with no requirements row, so a gate-internal
# constant *can* be pinned there and this argument's second leg no longer
# holds. What remains true is that nothing pins a literal with no name at all,
# which is what a timeout written inline here would be. When a
# test appears whose own timeout is load-bearing, rule 18 is the reason to
# revisit this; until then the default is the sized cap the rule asks for.
args=(--colors=never --no-times)
[[ -n "$CRATE" ]] && args+=(-p "$CRATE")

# ⚠️ **`M4.19`: the one change that actually moved this gate — 102 s to 33 s,
# 3.1x, on the identical diff.** `[profile.mutants]` in the root manifest is
# `dev` with debuginfo off and nothing else altered. The cost table above is
# why it helps so much: this gate is a per-mutant *rebuild* loop, so anything
# that shortens one rebuild is multiplied by the mutant count.
#
# ⚠️ **Quality-neutral, and the argument is narrow enough to check.**
# `opt-level`, `debug-assertions` and `overflow-checks` are inherited from
# `dev` unchanged — those are what can flip a mutant from caught to missed,
# because a mutant is caught by a test failing and an assertion or an overflow
# check is often the thing that fails. Debuginfo is read by a *debugger*, and
# nothing under test asserts on a backtrace's line numbers. ⚠️ **Verified
# rather than argued**: dev and this profile were run warm against the same
# 30-mutant diff and both reported `19 caught, 11 unviable` — the counts are
# the check, not the wall clock.
#
# ⚠️ **It is its own `--profile`, not an edit to `dev`**, so a developer's
# normal build keeps its debuginfo. The trade is one cold rebuild the first
# time this runs, into `$CARGO_TARGET_DIR/mutants/` rather than `debug/`.
#
# ⚠️ **And the trade is disk, not only time — review caught this being
# stated as time alone, then caught the correction overstating itself.**
# This script is the only writer of `target/mutants`, so once every run
# passes `--profile mutants` the old `target/mutants/debug` tree is never
# read or written again. ⚠️ **There are two of them, and the first version
# of this paragraph conflated them**: the container's `oqueue-target`
# volume held **48 GB**, and the host checkout a separate, older **22 GB**
# — both stale, both deleted when the profile landed. Cargo
# has no target GC (`build.md`, Hygiene) and nothing here runs
# `cargo-sweep`, so a stale tree persists for the life of the checkout —
# the `No space left on device` failure doc 19 §9 records for this exact
# tool, arriving after the builds have already been paid for. The new tree
# is far smaller than either (a few hundred MB), which is
# the debuginfo this profile drops.
args+=(--profile mutants)

# ⚠️ Its own target directory, for `build.md` rule 18's reason: cargo-mutants
# rebuilds constantly and would otherwise evict the normal build on every
# alternation. The per-mutant workspace copies land in `TMPDIR`, redirected
# just below — doc 18 §3.7.4 measured 1.7 GB left behind on one workspace.
# ⚠️ **`TMPDIR`, and this is the one that matters for disk.** `cargo-mutants`
# copies the whole workspace per mutant into
# `$TMPDIR/cargo-mutants-<pkg>-<rand>.tmp` — **not** into `CARGO_TARGET_DIR`,
# which moves nothing. `/tmp` is a 16 GB tmpfs on WSL2, so those copies are RAM:
# two 394 MB orphans from this tool were sitting there when review checked.
# `build.md` rule 19 says scratch lives under `target/`; doc 18 §3.7.4 and
# doc 19 §9 name `target/mutants-tmp` specifically. ⚠️ `Drop`
# does not survive SIGKILL, so leaks happen — putting them on real disk under
# `target/` is what makes them sweepable.
export TMPDIR="$REPO_ROOT/target/mutants-tmp"
mkdir -p "$TMPDIR"

# ⚠️ **Set only if cargo has not been told otherwise by a config file.**
# `${CARGO_TARGET_DIR:-...}` sees the *environment* and not
# `.cargo/config.toml`'s `[build] target-dir` — which is how
# `tests/gates/negative.sh` redirects its scratch builds to satisfy `build.md`
# rule 19. Exporting unconditionally overrode that and put 30 MB in the system
# temp directory, while the comment here claimed the opposite. Found by review.
if [[ -z "${CARGO_TARGET_DIR:-}" ]] && ! grep -qs 'target-dir' "$REPO_ROOT/.cargo/config.toml"; then
  export CARGO_TARGET_DIR="$REPO_ROOT/target/mutants"
fi

if [[ "$MODE" == "narrowed" ]]; then
  diff_file="$REPO_ROOT/target/tmp/mutants-diff.$$"
  mkdir -p "$REPO_ROOT/target/tmp"
  # ⚠️ The **staged** diff, matching what `check-reviewed.sh` hashes and what a
  # commit will contain. An unstaged edit is not what is being committed.
  # ⚠️ `|| true`, and the emptiness check below carries the verdict. `lib.sh`
  # sets `-e`, and `git diff --cached` exits 128 wherever there is no usable
  # index — `review.sh` plants a bare `.git` to build its packet, so without
  # this every review packet would carry this gate as FAILED. Found by review,
  # in the packet for this very commit.
  git diff --cached -- '*.rs' > "$diff_file" 2>/dev/null || true
  if [[ ! -s "$diff_file" ]]; then
    rm -f "$diff_file"
    skip "mutation testing (no Rust staged — nothing to narrow to)"
    note "run: scripts/mutants.sh --full   for the whole workspace"
    finish
  fi
  args+=(--in-diff "$diff_file")
fi

ok "running cargo mutants (${MODE}${CRATE:+, $CRATE})"
# ⚠️ `|| rc=$?`, not a bare call: a surviving mutant makes `cargo mutants` exit
# non-zero, and under `lib.sh`'s `-e` a bare call would skip both the cleanup
# below and the exit code this script exists to propagate.
rc=0
cargo mutants "${args[@]}" || rc=$?
[[ "$MODE" == "narrowed" ]] && rm -f "$REPO_ROOT/target/tmp/mutants-diff.$$"

# ⚠️ This script reports; it does not judge. A surviving mutant is an exit code
# from `cargo mutants` and a line in its output, and `check-mutants.sh` is what
# turns that into a verdict against the baseline.
exit "$rc"
