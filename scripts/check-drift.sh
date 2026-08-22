#!/usr/bin/env bash
# A threshold that was a fixed constant does not become settable. `M-1.7`.
#
#   scripts/check-drift.sh
#
# Non-negotiable 2: "thresholds are constants no environment can move." This
# is the half of that rule about *how* a threshold could move — the other
# half, a value silently **weakened**, is `check-tests-kept.sh`'s sibling
# concern for tests. ⚠️ Not "lowered": for **three** of the seven thresholds
# `m0-complete.sh` pins — `FILE_LINE_LIMIT`, `BUDGET_MS`, `EXEMPT_RUNS_FLOOR`
# — raising is the weakening move, and `TIMINGS_KEEP_DAYS` silences its
# warning in *both* directions. ⚠️ `EXEMPT_RATE_THRESHOLD` is **not** in that
# list: it is the rate's denominator, so raising it tightens. Read
# `m0-complete.sh`'s per-entry table rather than generalising from a suffix —
# two of its seven rows were wrong across two drafts. ⚠️ Non-negotiable 2's
# canonical wording now names the weakening *direction* rather than "lower"
# (`M2.7`, closing `M1.49`), so this header and `AGENTS.md` finally say the
# same thing — rust-style.md rule 7's raising-a-clippy-ceiling example was
# already the new wording's shape. ⚠️ ~~see "What this does not catch" below~~ — **no such section
# exists in this file** (`M1.35`). The half-gate it pointed at now has a real
# answer: `m0-complete.sh`'s `NFR_CONSTANTS` pins each threshold's *value*, so
# moving one in either direction fails a gate. ⚠️ **Two limitations, and the second matters more.**
# The map is hand-maintained (see `THRESHOLD_RE` below). And `m0-complete.sh` is
# a *milestone-boundary* gate — invoked by neither `.pre-commit-config.yaml` nor
# `gates.yml` — so moving a value lands green on the commit path and is caught
# whenever someone next runs that gate. This script is on the commit path; the
# one it points at is not.
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

# Case-insensitive. ⚠️ **Mostly substring, and the unit suffixes are anchored** —
# never with `\b`, whose meaning differs across platforms, but with an explicit
# `([^a-z0-9]|$)`, which does not. `threshold`, `_limit`, `_ceiling` and `_floor`
# stay unanchored because a false positive there is a word somebody chose; `_ms`
# unanchored matched `_msg`, which is a word everybody chooses.
#
# ⚠️ **And a false positive is *not* cheap to dismiss**, which this comment used
# to claim — ⚠️ and `M1.35` removed the last place that still quoted the retracted
# phrase as though it were current wording, further down this same file. There is no suppression mechanism in this script — no baseline, no
# per-line escape — so the only exits are renaming a legitimate variable or
# widening the regex, and widening it is editing a non-negotiable-2 gate to make
# a check pass. `_limit` is the live one: `rate_limit_header = env::var(...)` is
# refused and the remedy offered ("hard-code the value") is wrong for it. Adding
# a baseline is the honest fix and belongs to whoever hits it; ⚠️ until then the
# cost of a false positive here is a rename, and the comment says so.
#
# ⚠️ **A name-based matcher only sees what somebody named conventionally**, and
# `M0` demonstrated the failure twice in three commits. `M0.15` found
# `MIN_CRATE_COVERAGE` matched nothing — its gate's acceptance said
# "`check-drift.sh` passes" and it did, having looked at no constant at all —
# renamed it `COVERAGE_FLOOR`, and recorded that **the name is load-bearing**.
# `M0.16`, the next commit, wrote `BUDGET_MS` and `COMPILING_GATE_MS`, and this
# regex saw neither. `_ms` and `_seconds` are here because a duration is the
# other shape a threshold takes; ⚠️ **the class is still open**, and
# ~~`m0-complete.sh` is what makes it not depend on someone choosing the right
# word: it asserts that every constant a requirement names is matched by this
# regex, so a new one that is invisible here fails a gate rather than passing
# quietly.~~ `M0.23`.
#
# ⚠️ **False, measured by `M1.35`.** `m0-complete.sh` asserts that only for the
# constants *listed in its own `NFR_CONSTANTS` literal*, and that list is
# hand-maintained: delete all four of `M1.35`'s entries and the gate still
# exits 0. So a threshold whose name matches neither this regex nor that map is
# unenforced for non-negotiable 2 while every gate reports `ok`. ⚠️ Of the four
# `M1.35` found, **`LIMIT` and `TIMINGS_KEEP_DAYS` were invisible to both** —
# the latter matches this regex only since `M1.35` widened it below.
# `EXEMPT_RATE_THRESHOLD` and `EXEMPT_RUNS_FLOOR` already matched, because
# `M1.30` chose names that would. All four were absent from the map, so none
# was pinned by value.
# **Adding a threshold means making its name visible to this regex *and*
# listing it in that map — the map does not substitute for the regex, since
# `m0-complete.sh` asserts both — and nothing will tell you that you did
# neither.**
# ⚠️ `_ms` and `_seconds` are **suffix-anchored**; the rest stay substrings.
# Unanchored, `_ms` matches `_msg` and `_msvc` — ⚠️ **not `_message`, which this
# line claimed until `M1.24` checked it**: `_message` has no `_ms` in it at all
# (`_me`…), so it never matched, anchored or not. The two that do are enough to
# make the point, and an example that does not match weakens it. ⚠️ Measured
# **before the anchoring below existed** — a plain
# `let err_msg = std::env::var("OQUEUE_BANNER")...` was rejected by the gate,
# with a remedy telling the developer to hard-code a banner string. Against the
# pattern as it stands, `err_msg` matches nothing and no such rejection is
# reachable; the sentence records why the anchoring was added, not what the
# gate does now. ⚠️ **And this script has
# no suppression mechanism**, so dismissing a false positive is not actually
# available — ⚠️ this sentence quoted "cheap to dismiss on sight" as the
# header's wording, which the header itself retracted (`M1.35`): the only exits from a false positive are renaming a
# legitimate variable or widening this regex, and the second is editing a
# non-negotiable-2 gate to make a check pass. A duration constant ends in its
# unit; a message variable does not — ⚠️ **except that `msec` is also a unit
# spelling**, which the first anchoring dropped: `POLL_MSEC="${OQUEUE_POLL_MSEC:-500}"`
# matched before and matched neither branch after, so narrowing to fix a false
# positive opened a false negative on the same gate. Measured by review. The
# optional `ec`/`ecs` and the digit-tolerant tail keep both.
# ⚠️ `_days?` added by `M1.35`. `check-budget.sh`'s `TIMINGS_KEEP_DAYS=30` is a
# retention window, which is the same kind of threshold as one in seconds or
# milliseconds — both of which this regex already matched — so widening is the
# right remedy here rather than the rename `FILE_LINE_LIMIT` took. The choice
# is per-constant: rename when the name is simply wrong — `LIMIT` became
# `FILE_LINE_LIMIT` — and widen when the naming convention has a genuine gap.
THRESHOLD_RE='threshold|_limit|_budget|_ceiling|_floor|_ms(ecs?)?[0-9]*([^a-z0-9]|$)|_secs?(onds?)?[0-9]*([^a-z0-9]|$)|_days?([^a-z0-9]|$)'

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
  # ⚠️ **The second remedy, for the case the first one misdiagnoses.** The
  # matcher is by name, so a variable that merely *contains* one of the words
  # above is refused too — `rate_limit_header` is the live one — and telling
  # its author to hard-code an HTTP header name is nonsense. There is no
  # suppression mechanism here, so rename is the exit, and widening the regex
  # to pass is the thing non-negotiable 2 forbids. Found by review, which
  # noted the remedy had been corrected in a comment and not in the output.
  note "⚠️ if the name merely contains one of those words and is not a"
  note "threshold at all, rename it — this gate matches by name and has no"
  note "suppression list, and widening its regex to pass is what rule 2 forbids"
fi

finish
