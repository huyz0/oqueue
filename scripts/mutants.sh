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
# ## Cost, measured — ⚠️ by mode *and* cache state, because both matter
#
#   narrowed, diff with no mutants       0.1 s   (an early exit, not a run)
#   narrowed, 1 mutant                   1.2 s
#   narrowed, 13 mutants, warm           4.8 s
#   narrowed, 13 mutants, cold           9.7 s
#   unnarrowed, `-p oqueue-core`, warm  23 s
#
# ⚠️ **The first row is not a cost, it is a skip**, and quoting it as "narrowed
# runs take 0.08 s" is how this script's placement was first argued. Review
# caught that. The row that decides placement is the 4.8 s one: a modest new
# file, warm, leaves ~5 s of NFR-56's 10 s budget for everything else — which
# the suite currently uses 2.2 s of. Cold, it does not fit, and
# `check-budget.sh` reports that as a compiling run rather than as erosion.
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
# is *also* environment-settable, and `m0-complete.sh` pins the two constants
# `requirements.md` names. When a
# test appears whose own timeout is load-bearing, rule 18 is the reason to
# revisit this; until then the default is the sized cap the rule asks for.
args=(--colors=never --no-times)
[[ -n "$CRATE" ]] && args+=(-p "$CRATE")

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
