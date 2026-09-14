#!/usr/bin/env bash
# A surviving mutant is killed or argued. `M0.17`, `testing.md` rule 17.
#
#   scripts/check-mutants.sh
#
# ## What this is for
#
# ⚠️ **Mutation testing is the primary quality gate** — `testing.md` rule 15.
# Coverage says a line ran; a surviving mutant says nothing *constrained* it,
# which is the characteristic failure of generated tests and the one thing
# `check-coverage.sh` cannot see. `check-coverage.sh`'s own header says so.
#
# ## The baseline
#
# `baselines/mutants.txt`, one argued survivor per line with a reason, in the
# same shape and for the same reason as `baselines/review.txt` and
# `baselines/unsafe.txt`: ⚠️ **a list nobody grows quietly.** A survivor is
# killed by writing a test, or argued here — and an argued one is a claim a
# reviewer can check, not a suppression.
#
# ⚠️ **A growing baseline is itself the signal**, the same way `review.md`
# rule 17 treats a growing argued list. Reviewed at milestone boundaries.
#
# ## Where this runs
#
# **Narrowed in pre-commit, full in the nightly tier.** `testing.md` rule 16
# asks for "diff-narrowed on every commit; the full sharded run is nightly" —
# ⚠️ this gets the first half exactly and the second **partly**: nightly since
# `M4.51` (`.github/workflows/mutants.yml`), still unsharded.
#
# ⚠️ **It was per-push until `M4.51`, argued from a number that described
# something else.** This paragraph said "At 23 s that is affordable; the day it
# is not, a nightly schedule is the change rule 16 actually describes" — and
# 23 s is `M0.17`'s *unnarrowed `oqueue-core`* figure, one leaf crate, not the
# workspace. `M4.32` ran the workspace for the first time: **2294 mutants**,
# and `baselines/mutants.txt:26` and that commit's own body both describe the
# log it produced as a **six-hour** run. That was the day this paragraph named,
# and it went on saying the opposite for the rest of M4.
#
# ⚠️ **Per-push was worse than merely slow.** A GitHub-hosted job is cancelled
# at 360 minutes and `gates.yml`'s job also walks every commit in the push
# through `pre-commit run --all-files` first, so the step could not finish —
# and a cancelled job is not distinguishable from a red one. The negative
# suite and the seed-corpus replay were serialised behind it despite
# `if: always()`.
#
# ⚠️ **The nightly job declares `timeout-minutes: 330`, and that bound is
# expected to fire until sharding lands.** It does not make a six-hour run
# fit; it makes the failure *say* the run did not fit, which the platform's
# own cancellation does not. `cargo mutants --shard k/n` is what makes it fit
# and `M4.60` is the row — sharding interacts with the converse loop below,
# because a shard tests 1/k of the mutants and every baseline entry arguing a
# survivor in another shard looks dead to it.
#
# ⚠️ **The placement was argued twice from bad numbers before it was argued from
# good ones.** First against 23 s, which is the *unnarrowed* run; then against
# 0.08 s, which is the early exit taken when a diff generates no mutants at all
# — a skip, not a run. The number that decides it is **4.8 s warm for a
# 13-mutant diff**, leaving ~5 s of NFR-56's 10 s budget against a suite that
# currently uses 2.2 s. Cold, the same diff is 9.7 s and does not fit; that run
# is a compiling run, which `check-budget.sh` reports rather than fails.
#
# ⚠️ The cost is proportional to the change, so a very large diff can still be
# slow. `check-budget.sh` is what notices: a run where this gate alone exceeds
# 5 s is reported as compiling rather than as erosion, and a suite that is
# permanently 'compiling' is the signal that this belongs back in CI only.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

BASELINE="baselines/mutants.txt"



if ! has_rust; then
  skip "mutation testing (no Cargo.toml yet)"
  finish
fi
require_tool cargo "install Rust via https://rustup.rs" || finish
if ! cargo mutants --version >/dev/null 2>&1; then
  skip "mutation testing (cargo-mutants not installed)"
  note "install: cargo install cargo-mutants"
  note "⚠️ a missing tool is a skip, never a pass — this gate did not run"
  finish
fi

out="$REPO_ROOT/target/tmp/mutants-run.$$"
mkdir -p "$REPO_ROOT/target/tmp"
# ⚠️ Runs the tool through `mutants.sh`, so the two cannot disagree about how a
# run is configured. `build.md` rule 22's reason: a check implemented twice
# drifts, and the version that matters is whichever one was not run.
# ⚠️ `|| run_rc=$?`, not a bare call. `lib.sh` sets `-e`, and a surviving mutant
# makes `cargo mutants` exit 2 — so a bare call killed this script *before* it
# could read the exit code, let alone compare survivors against the baseline.
# The gate exited 2 with no output and the negative suite counted that as a
# pass. Exactly the shape `tests/gates/negative.sh`'s own header documents for
# its `invoke_*` functions.
# ⚠️ `OQUEUE_SUPPRESS_TIMING` on the **child call only**, not exported. Both
# scripts source `lib.sh` and both call `finish()`, so one hook would otherwise
# append two timing rows and `check-budget.sh` would count a phantom gate.
# Exporting it instead suppressed *this* gate's row too — 15 rows for a 16-hook
# suite — which is the same error one level up.
run_rc=0
OQUEUE_SUPPRESS_TIMING=1 bash "$REPO_ROOT/scripts/mutants.sh" "$@" > "$out" 2>&1 || run_rc=$?

# `cargo mutants` prints `MISSED <file>:<line>:<col>: <description>` per
# survivor. Anything else it prints is not this gate's business.
mapfile -t survivors < <(grep -E '^MISSED ' "$out" | sed 's/^MISSED  *//' || true)

if (( run_rc != 0 && ${#survivors[@]} == 0 )); then
  # ⚠️ A non-zero exit with no `MISSED` line is a *tool* failure — a build
  # error, a timeout — not a clean run. Reporting ok here would be the vacuous
  # shape this project keeps finding.
  fail "cargo mutants exited $run_rc with no surviving mutants reported"
  tail -20 "$out" >&2
  rm -f "$out"
  finish
fi

# ⚠️ **Zero mutants tested is not a pass.** A narrowed run over a diff that
# touches only tests, comments or non-Rust files generates nothing, and
# `cargo mutants` says `No mutants to filter` and exits 0. Reporting `ok` there
# would be the vacuous-green shape this project keeps finding — the gate would
# be reporting that nothing survived a run that never happened. Found while
# testing this script: it said "0 survivor(s), all argued" for a diff of pure
# test changes.
n_tested="$(grep -oE '^[0-9]+ mutants? tested' "$out" | tail -1 | grep -oE '^[0-9]+' || true)"
if [[ -z "$n_tested" ]] || (( n_tested == 0 )); then
  rm -f "$out"
  skip "mutation testing (no mutants in this diff — nothing to constrain)"
  note "a diff touching only tests, comments or non-Rust files generates none"
  note "⚠️ this gate proved nothing about this commit; run: scripts/mutants.sh --full"
  finish
fi


# ⚠️ **Anchored to the start of the line, and it was not until `M4.62`.**
# The pattern was `^(skip|.*skip) ...`, and `.*skip` reaches a line's
# interior: only the *phrase* after `skip` was pinned, so a rustc error
# reading `error: could not skip mutation testing (no Rust staged)` matched
# and turned a tool failure into a propagated skip — the vacuous-green shape
# this gate exists to refuse, in the branch that decides whether it ran at
# all. `M4.55` corrected the comment's claim; this is the regex.
#
# ⚠️ **No colour escape is admitted, because none can occur.** `lib.sh` sets
# `_DIM`/`_OFF` from `[[ -t 1 ]]` alone and has no force-colour switch, and
# this script always redirects `mutants.sh` to `$out`, so `skip` prints the
# bare word. A first version of this pattern carried two optional
# `.\[[0-9;]*m` groups for a caller that could force colour on; review
# measured that such a caller cannot exist, and an unreachable alternative is
# how the old one got there.
#
# ⚠️ **And this branch cannot fire on a real skip at all**, which is worth
# knowing before anyone widens it again: a skipping run prints no
# `N mutants tested` line, so the `n_tested` guard above returns first with
# its own (differently worded) skip. The only way here is the *false*
# positive this commit removed — output that is not a skip but contains the
# phrase. The branch is kept rather than deleted because deleting it would
# make `n_tested`'s wording the sole report of a skipped run, and that is a
# behaviour change beyond `M4.62`. `M4.63` if it is worth one.
if grep -qE '^skip (mutation testing \((cargo-mutants not installed|no Rust staged))' "$out"; then
  # mutants.sh already reported the skip; propagate it rather than inventing a
  # verdict of our own.
  sed -n '1,4p' "$out"
  rm -f "$out"
  skip "mutation testing (see above)"
  finish
fi

# The argued list, comments and blanks stripped.
declare -a argued=()
malformed=0
if [[ -f "$BASELINE" ]]; then
  # ⚠️ Read from the **index**, not the working tree — the same rule
  # `baselines/review.txt` follows. An unstaged argument suppresses a survivor
  # while leaving nothing in the commit to show for it.
  while IFS= read -r line; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    # ⚠️ **A location alone is refused**, the same rule `check-reviewed.sh` and
    # `check-unsafe.sh` enforce on their baselines and for the same reason: an
    # id with no reason suppresses a finding while recording nothing about why.
    if [[ ! "$line" =~ ^([^[:space:]]+([[:space:]][^[:space:]]+)*)[[:space:]][[:space:]]+[^[:space:]] ]]; then
      fail "baseline entry has no reason: $line"
      note "format: <file>:<line>:<col>: <mutation>  <why this survivor is acceptable>"
      malformed=1
      continue
    fi
    # ⚠️ And the location must look like one, so a stray word cannot argue away
    # every survivor by prefix. Review demonstrated it: a baseline of the single
    # character `c` marked five unargued survivors as argued.
    if [[ ! "${line%%  *}" =~ ^[^[:space:]]+\.rs:[0-9]+:[0-9]+: ]]; then
      fail "baseline entry does not start with a <file>.rs:<line>:<col>: location: $line"
      malformed=1
      continue
    fi
    argued+=("$line")
  done < <(git show ":$BASELINE" 2>/dev/null || true)
fi

if (( malformed > 0 )); then
  rm -f "$out"
  finish
fi

unargued=0
for s in "${survivors[@]}"; do
  matched=0
  for a in "${argued[@]}"; do
    # An entry is `<location>  <reason>`; match on the location prefix.
    loc="${a%%  *}"
    [[ -n "$loc" && "$s" == "$loc"* ]] && { matched=1; break; }
  done
  if (( matched == 0 )); then
    fail "surviving mutant, not killed and not argued: $s"
    unargued=$((unargued + 1))
  fi
done

if (( unargued > 0 )); then
  note "kill it with a test, or argue it in $BASELINE as:"
  note "  <file>:<line>:<col>: <mutation>  <why this survivor is acceptable>"
  note "⚠️ the baseline is a list nobody grows quietly — testing.md rule 17"
fi

stale=0
# ── And the converse: an entry that argues nothing ──────────────────────────
#
# ⚠️ **This loop runs the other way round, and `M4.32` is why it exists.**
# Everything above walks survivors and asks whether each is argued; nothing
# walked the baseline and asked whether each entry still matches a survivor.
# So an entry whose mutant *moved* suppresses nothing and no gate can see it
# — measured inside one task, where a single entry needed re-keying four
# times, and five by the time `M4.55` read it again: the guard gained a
# conjunct, the file split at the 500-line limit, a doc comment was added
# above it, and then a type one level out grew an accessor. ⚠️ **The last
# two touched none of that code.** Each of the first four was caught only
# because the mutant kept *surviving*, which made the mismatch surface as an
# unargued survivor. An entry whose mutant became killable would have gone
# quiet instead, and the baseline would carry a suppression for something no
# longer there — `testing.md` rule 17's "a list nobody grows quietly" read
# from the shrinking side.
#
# ⚠️ **`cargo mutants --list` re-derives a key in seconds, and nothing said
# so until `M4.55`.** It enumerates every mutant's file, line and column
# without running a test, so a key can be checked the moment a file moves
# rather than waiting for the next `--full` pass — which is the six-hour job
# `.github/workflows/mutants.yml` now runs nightly. The fifth re-key was
# found that way; the four before it were each found the slow way.
#
# ⚠️ **And `--list` is the *last* step, not a preparatory one.** `M4.55` took
# its listing before making its own edits to the same file, re-keyed from it,
# and shipped a key that was stale on arrival — review caught it, and it
# would have been the sixth instance of exactly what that entry narrates.
# Any change touching a file with a baseline entry re-derives the key after
# the edit settles.
#
# ⚠️ **Both loops run and both report before either exits.** The first
# version put this after the `unargued` exit, so on a run with any unargued
# survivor it never executed — and the one run that matters, the first full
# one, had 56. The worker clearing those would have had to fix them all
# before learning that an entry was also stale. ⚠️ **And after the
# run-validity guards, which moved up beside the survivors they judge**: a
# run that generated nothing has an empty survivor list, so every entry
# "argues no surviving mutant", which is what this reported against a
# fixture whose crate produced no mutants at all.
#
# ⚠️ **Only on a full run.** A narrowed run generates mutants for the staged
# diff alone, so almost every entry legitimately matches nothing; asking this
# question there would fail every commit. `--full` is the only mode in which
# "matched nothing" means "argues nothing".
#
# ⚠️ **`unviable:` is the exemption, and it has to exist.** A mutant can stop
# being *generated* as well as stop surviving: `cargo mutants` reports one
# that no longer compiles as unviable and never lists it at all, which is
# indistinguishable here from one that was killed. An entry whose reason
# begins `unviable:` says that is what happened and why, which is a claim a
# reader can check — where silence is not.
# ⚠️ **`$1`, exactly as `mutants.sh` reads it.** That script's own `case`
# matches on `${1:-}` alone, so `check-mutants.sh oqueue-core --full` takes
# `oqueue-core` as the crate and stays *narrowed* — and a `$*` substring test
# called it full and reported every entry outside that crate stale. Mirroring
# the parse is the only way the two cannot disagree, which is `build.md` rule
# 22's reason for this script delegating to that one in the first place.
if [[ "${1:-}" == "--full" ]]; then
  for a in "${argued[@]}"; do
    loc="${a%%  *}"
    reason="${a#*  }"
    matched=0
    for s in "${survivors[@]}"; do
      [[ "$s" == "$loc"* ]] && { matched=1; break; }
    done
    if [[ "${reason#"${reason%%[![:space:]]*}"}" == unviable:* ]]; then
      # ⚠️ **An `unviable:` claim that matches a survivor is a claim the run
      # just disproved**, and accepting it silently is how this very entry
      # came to suppress nothing: `M4.32` re-keyed one from stale prose
      # instead of from the log it had produced, marked it unviable, and the
      # loop below skipped it — inside the commit that added the loop.
      if (( matched == 1 )); then
        fail "baseline entry claims 'unviable:' for a mutant that survived: $loc"
        stale=$((stale + 1))
      fi
      continue
    fi
    if (( matched == 0 )); then
      fail "baseline entry argues no surviving mutant: $loc"
      stale=$((stale + 1))
    fi
  done
  if (( stale > 0 )); then
    note "the mutant was killed, moved, or stopped being generated -- delete the"
    note "entry, re-key it, or say 'unviable: <why it no longer compiles>'"
    note "⚠️ a suppression for something that is not there is one nobody can see"
  fi
  (( stale == 0 )) && ok "every baseline entry still argues a surviving mutant"
fi

if (( unargued > 0 || stale > 0 )); then
  rm -f "$out"
  finish
fi


rm -f "$out"
ok "mutation testing (${n_tested} mutants tested, ${#survivors[@]} survivor(s), all argued)"
finish
