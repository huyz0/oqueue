#!/usr/bin/env bash
# The recorded backend matrix is well-formed, and agrees with what actually
# ran. `M1.21`, `FR-31`.
#
#   scripts/check-conformance-matrix.sh
#
# ## Why this is its own script rather than a section of m1-complete.sh
#
# It started as one. ⚠️ **Review measured two ways a section there could
# regress in silence**: changing `$2 == "verified"` to a string no row matches
# made one direction vacuous, and deleting the other direction's loop outright
# also left the gate reporting `ok` and exiting 0. Neither had a case in
# `tests/gates/negative.sh`, because writing one meant scaffolding a cargo
# workspace **and** a Docker daemon just to reach the text-comparison part.
#
# Split out, the comparison needs neither: a matrix file and a roster file are
# the whole input, so `negative.sh` can plant each defect in a scratch
# directory in milliseconds. That is the difference between a gate that was
# verified once by hand and one that is verified on every run — `testing.md`
# rule 20a, which is what the split is for.
#
# ## The two halves
#
# **Format.** Every non-comment row is `<backend>  <status>  <why>`, status one
# of `verified` / `not-yet-run`, reason non-empty. ⚠️ **A row with no reason is
# refused**, the same rule `check-mutants.sh` and `check-unsafe.sh` enforce on
# their own baselines and for the same reason: an entry with no reason records
# a gap without recording anything about it. A status *typo* is refused for a
# sharper reason — an unrecognized status makes the row vanish from both
# comparisons below and from the counts, so the only visible signal is a
# plausible-looking `ok` line with a smaller number in it.
#
# **Agreement, in both directions.** Every `verified` row must appear in the
# roster a test run records; every roster entry must appear here as `verified`.
# The second direction is not symmetry for its own sake: `M1.21` found sixteen
# invented names in the roster, written by the recording mechanism's own unit
# test, and nothing noticed because nothing read the file back.
#
# ## Where each half runs, and why they are not the same place
#
# **Format: pre-commit**, reading the **index**, because it is pure text and
# costs milliseconds, so a malformed row cannot reach a commit. ⚠️ The index
# and not the working tree — `check-mutants.sh`, `check-unsafe.sh` and
# `check-reviewed.sh` all read their baselines the same way and for the same
# reason: a malformed row staged while a clean copy sits in the working tree
# otherwise passes here and fails in CI, which `lib.sh`'s own header records
# as a defect this repository has already fixed once.
#
# **Agreement: `--against-roster`, and only from `scripts/gates/m1-complete.sh`.**
# ⚠️ **It was briefly in pre-commit too, and that was a real break**, not a
# style question: the hook that runs `cargo test --workspace` writes a roster
# containing exactly `fake`, so the very next hook compared a matrix naming a
# verified `s3` against a roster that legitimately lacked it and failed every
# commit on a freshly-cleaned tree — and would have turned CI red on the next
# push. A partial roster is indistinguishable from the defect this check
# exists for, so the check belongs where a *complete* roster is guaranteed:
# after the milestone gate has actually run the suite against every backend.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ⚠️ **Literals, not environment overrides.** They were overridable, and
# review measured what that bought: `OQUEUE_CONFORMANCE_ROSTER=/nonexistent`
# sent the agreement half down its "no roster, say so and stop" path, so this
# script *and* `m1-complete.sh` above it both printed `ok` having compared the
# matrix to nothing. Nothing in the repository set either variable. A path a
# gate reads is a threshold in the sense that matters — non-negotiable 2's
# reasoning, applied to what is checked rather than to what it is checked
# against.
MATRIX="baselines/conformance-matrix.txt"
ROSTER="target/conformance/backends.txt"

AGAINST_ROSTER=0
if (( $# > 1 )); then
  fail "this gate takes at most one argument, got $#"
  finish
fi
if (( $# == 1 )); then
  if [[ "$1" == "--against-roster" ]]; then
    AGAINST_ROSTER=1
  else
    # ⚠️ Refused, not ignored. Ignoring it left the agreement half skipped
    # while both this script and `m1-complete.sh` still printed `ok` — the
    # same vacuity the removed environment overrides caused, reachable through
    # the argument instead. A one-character typo in the caller's flag is all
    # it takes, and the only visible difference is one missing `ok` line.
    fail "unknown argument: $1"
    note "the only argument this gate takes is --against-roster"
    finish
  fi
fi

if [[ ! -f "$MATRIX" ]]; then
  fail "$MATRIX not found -- the recorded backend matrix is what this gate checks"
  finish
fi

# ⚠️ From the index, not the working tree — see this file's header. But only
# where there *is* an index: `review.sh` materialises the staged tree under
# `target/tmp` and plants a deliberately invalid `.git` in it, so `git show`
# cannot work there and the file on disk already **is** the staged content, by
# construction. Failing in that harness would have put a permanent, false
# `**FAILED**` line into every review packet from here on — which is what the
# first version of this did, and the packet that carried the finding was
# itself the evidence. The three baseline gates this header claims kinship
# with degrade rather than fail for the same reason.
MATRIX_SRC="$MATRIX"
STAGED_MATRIX=""
if git rev-parse --git-dir >/dev/null 2>&1; then
  if MATRIX_TEXT="$(git show ":$MATRIX" 2>/dev/null)" && [[ -n "$MATRIX_TEXT" ]]; then
    mkdir -p "$REPO_ROOT/target/tmp"
    STAGED_MATRIX="$REPO_ROOT/target/tmp/conformance-matrix-staged.$$"
    trap 'rm -f "$STAGED_MATRIX"' EXIT
    printf '%s\n' "$MATRIX_TEXT" > "$STAGED_MATRIX"
    MATRIX_SRC="$STAGED_MATRIX"
  else
    fail "$MATRIX is not in the index -- stage it, so what is checked is what commits"
    note "an unstaged matrix passes here and fails in CI, against different bytes"
    finish
  fi
fi

# --- Format -----------------------------------------------------------------
malformed=0
lineno=0
while IFS= read -r line || [[ -n "$line" ]]; do
  lineno=$((lineno + 1))
  [[ "$line" =~ ^[[:space:]]*# ]] && continue
  [[ -z "${line//[[:space:]]/}" ]] && continue

  # `<name>  <status>  <reason>`: at least two spaces between fields, so a
  # reason containing single spaces stays one field.
  if [[ ! "$line" =~ ^([^[:space:]]+)[[:space:]][[:space:]]+([^[:space:]]+)[[:space:]][[:space:]]+([^[:space:]].*)$ ]]; then
    fail "$MATRIX:$lineno is not <backend>  <status>  <why>: $line"
    note "fields are separated by two or more spaces, and the reason is required"
    malformed=$((malformed + 1))
    continue
  fi
  status="${BASH_REMATCH[2]}"
  if [[ "$status" != "verified" && "$status" != "not-yet-run" ]]; then
    fail "$MATRIX:$lineno has status '$status', which is neither verified nor not-yet-run"
    note "⚠️ an unrecognized status makes the row invisible to every check below"
    malformed=$((malformed + 1))
  fi
done < "$MATRIX_SRC"

# ⚠️ Two rows for one backend is refused rather than resolved. Direction 2
# below stops at the first match, so a `verified` row shadowing a
# `not-yet-run` one for the same backend passes in that order and fails in
# the other -- an order-dependent verdict is worse than either answer.
duplicates="$(awk '
  /^[[:space:]]*#/ { next }
  NF == 0          { next }
  { seen[$1]++ }
  END { for (name in seen) if (seen[name] > 1) print name }
' "$MATRIX_SRC")"
if [[ -n "$duplicates" ]]; then
  while read -r dup; do
    [[ -z "$dup" ]] && continue
    fail "$MATRIX names '$dup' more than once"
    malformed=$((malformed + 1))
  done <<< "$duplicates"
fi

if (( malformed > 0 )); then
  finish
fi

rows() {
  awk '
    /^[[:space:]]*#/ { next }
    NF == 0          { next }
    { print $1, $2 }
  ' "$MATRIX_SRC"
}

n_verified="$(rows | awk '$2 == "verified"' | wc -l | tr -d ' ')"
n_pending="$(rows | awk '$2 == "not-yet-run"' | wc -l | tr -d ' ')"

if (( n_verified == 0 && n_pending == 0 )); then
  fail "$MATRIX names no backends at all"
  note "⚠️ an empty matrix would make every check below vacuously true"
  finish
fi

ok "backend matrix is well-formed (${n_verified} verified, ${n_pending} not-yet-run)"

# --- Agreement with what actually ran ---------------------------------------
if (( AGAINST_ROSTER == 0 )); then
  finish
fi

if [[ ! -f "$ROSTER" ]]; then
  skip "matrix/roster agreement (no roster at $ROSTER)"
  note "only a conformance test run writes one: cargo test -p oqueue-store --test it"
  note "⚠️ this half proved nothing -- scripts/gates/m1-complete.sh is what runs the suite first"
  finish
fi

problems=0

while read -r backend status; do
  [[ "$status" == "verified" ]] || continue
  if ! grep -qxF "$backend" "$ROSTER"; then
    fail "matrix says '$backend' is verified, but the roster records no run for it"
    problems=$((problems + 1))
  fi
done < <(rows)

# ⚠️ `|| [[ -n "$recorded" ]]`, matching the format loop above. Without it a
# roster whose last line has no trailing newline loses that line entirely, and
# the entry never examined is exactly the kind this direction exists to catch:
# a roster of `fake\nconcurrent-0` with no final newline passed, silently.
while read -r recorded || [[ -n "$recorded" ]]; do
  [[ -z "$recorded" ]] && continue
  # ⚠️ A here-string, not `rows | awk … exit`: `portability.md` rule 21 —
  # a consumer that exits at the first match closes the pipe, the producer
  # takes SIGPIPE, and `pipefail` turns that into 141 inside a command
  # substitution, which `set -e` then kills the gate on with no message.
  status="$(awk -v want="$recorded" '$1 == want { print $2; found = 1; exit } END { if (!found) print "absent" }' <<< "$(rows)")"
  if [[ "$status" == "absent" ]]; then
    fail "the roster records a run for '$recorded', which $MATRIX does not name"
    note "either add it to the matrix with a reason, or find out what wrote it"
    problems=$((problems + 1))
  elif [[ "$status" != "verified" ]]; then
    fail "the roster records a run for '$recorded', which the matrix calls '$status'"
    problems=$((problems + 1))
  fi
done < "$ROSTER"

if (( problems > 0 )); then
  finish
fi

ok "backend matrix agrees with the recorded roster, in both directions"
finish
