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
# **Narrowed in pre-commit, full in CI.** `testing.md` rule 16 asks for
# "diff-narrowed on every commit; the full sharded run is nightly" — ⚠️ this
# gets the first half exactly and the second **approximately**: the full run is
# per-push rather than nightly, and unsharded, because there is no scheduled
# workflow to hang it on. At 23 s that is affordable; the day it is not, a
# nightly schedule is the change rule 16 actually describes.
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

# ⚠️ Anchored to the line `mutants.sh` itself emits, not a substring of the
# whole tool output — a rustc error quoting either phrase would otherwise turn a
# failure into exit 0.
if grep -qE '^(skip|.*skip) (mutation testing \((cargo-mutants not installed|no Rust staged))' "$out"; then
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
  rm -f "$out"
  finish
fi

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

rm -f "$out"
ok "mutation testing (${n_tested} mutants tested, ${#survivors[@]} survivor(s), all argued)"
finish
