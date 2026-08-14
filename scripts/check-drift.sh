#!/usr/bin/env bash
# A threshold that was a fixed constant does not become settable. `M-1.7`.
#
#   scripts/check-drift.sh
#
# Non-negotiable 2: "thresholds are constants no environment can move." This
# is the half of that rule about *how* a threshold could move — the other
# half, a value silently lowered, is `check-tests-kept.sh`'s sibling concern
# for tests and has no equivalent gate for thresholds yet; see "What this
# does not catch" below.
#
# ## What "settable" means here
#
# A threshold (`too-many-lines-threshold`, a coverage floor, a time budget, a
# regression percentage — anything `performance.md`, `testing.md`, or
# `build.md` names as a constant) stops being a constant the moment its value
# can be supplied from outside the commit that sets it: an environment
# variable, `option_env!`, or a shell parameter expansion with a fallback
# (`${VAR:-default}`). A constant an environment can move is a constant in
# name only — CI, a laptop, and a release build would each see a different
# number, and non-negotiable 2 stops meaning anything.
#
# ## The mechanism, and its honest limits
#
# This is a grep gate, not a parser: a line naming a threshold-shaped
# identifier (matched by *substring*, not a Rust or shell identifier
# boundary — portable across GNU and BSD grep, which disagree about `\b`) and
# an environment-read call **on the same line** fails. That catches the
# straightforward case:
#
#   let cognitive_threshold = std::env::var("COGNITIVE_THRESHOLD")...
#   BUDGET_SECONDS="${OQUEUE_BUDGET_SECONDS:-120}"
#
# It does **not** catch a threshold assigned from a variable that is *itself*
# set from the environment several lines away, or one passed through a config
# struct populated elsewhere. Closing that gap needs a real parser and is not
# this task's job; a grep gate that is honest about its blind spot is more
# useful than one that pretends to be exhaustive and is disabled the first
# time it overclaims — the lesson `docs/researches/19` §3.2 states for
# `check-core-contract.sh`.
#
# ⚠️ **This file is the one named exception**, because the two worked examples
# a few lines up put both signals on one line on purpose, to show a reader
# what the gate looks for. Scanning itself would fail forever on its own
# documentation — measured, not assumed: it does, unconditionally, the moment
# those two lines exist. It defines no threshold of its own to protect, so
# excluding it costs no real coverage. Every other file, including every
# other gate script, is still scanned.
#
# ## Why the whole tree, not just the staged diff
#
# A threshold made settable three commits ago is exactly as much a violation
# today as one made settable in this commit. Non-negotiable 2 is a property
# of the tree, not of a diff, so this scans every tracked file every run.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# Case-insensitive, substring match — no word boundary, for the portability
# reason above. False positives (a variable merely containing "floor") are
# possible and cheap to dismiss on sight; a missed positive because `\b`
# silently didn't mean the same thing on two platforms is not.
THRESHOLD_RE='threshold|_limit|_budget|_ceiling|_floor'

# Rust environment reads, plus the shell idiom for reading one with a
# fallback default. `\$\{[A-Za-z_][A-Za-z0-9_]*:[-=]` matches `${FOO:-...}`
# and `${FOO:=...}`, the two forms that read an environment variable if the
# caller's shell has one set.
ENV_RE='env::var|env::var_os|option_env!|\$\{[A-Za-z_][A-Za-z0-9_]*:[-=]'

# SELF is excluded — see the ⚠️ note above the header's worked examples.
SELF="scripts/$(basename "${BASH_SOURCE[0]}")"

mapfile -t files < <(git ls-files -- '*.rs' '*.sh' 'clippy.toml' '*/clippy.toml' 2>/dev/null \
  | grep -vxF "$SELF" || true)

if (( ${#files[@]} == 0 )); then
  skip "threshold settability (no .rs, .sh, or clippy.toml files tracked yet)"
  finish
fi

violations=0
scanned=0

for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  scanned=$((scanned + 1))

  # Two passes rather than one combined regex: grep -E has no portable
  # same-line "and" without a lookaround GNU and BSD grep both support, and
  # piping grep -n output through a second grep keeps the "lineno:content"
  # shape intact for the loop below.
  matches="$(grep -nEi "$THRESHOLD_RE" "$f" 2>/dev/null | grep -E "$ENV_RE" || true)"
  [[ -n "$matches" ]] || continue

  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    lineno="${m%%:*}"
    content="${m#*:}"
    content="$(printf '%s' "$content" | sed -E 's/^[[:space:]]+//')"
    fail "threshold made settable: $f:$lineno"
    note "$content"
    violations=$((violations + 1))
  done <<< "$matches"
done

if (( violations == 0 )); then
  ok "no threshold reads from the environment ($scanned file(s) scanned)"
else
  note "a threshold is a constant no environment can move — non-negotiable 2"
  note "hard-code the value; if it should differ by environment, that is a"
  note "config value, not a threshold, and belongs to a different standard"
fi

finish
