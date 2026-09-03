#!/usr/bin/env bash
# No handler constructs one of `M4.11`'s five fencing codes outside
# `crate::fencing::Refusal::error_code` — structural, not a runtime
# assertion. `check-topic-list-scope.sh`'s own style, `M9.11`'s precedent
# applied to fencing instead of the global-topic-list claim.
#
#   scripts/check-fencing-seam.sh
#
# `crates/oqueue-broker/src/fencing.rs` is the one seam every `M4.7`-`M4.10`
# handler's own fencing decision is supposed to route through (`M4.11`'s own
# backlog row, verbatim: "no handler constructs one of these five codes
# outside the shared function"). A handler that reaches for one of the five
# constants directly — a shortcut that answers the *right* code for the
# *wrong* reason, or drifts once a sixth case is added — would satisfy every
# test in `fencing/tests.rs` while quietly reintroducing the five ad hoc
# `if`s the task exists to remove. Nothing else would catch that: the wire
# code would still be correct, so no handler test would fail either.
#
# ⚠️ **`src/` only.** A test asserting `response.error_code ==
# error_codes::UNKNOWN_MEMBER_ID` is reading the answer, not constructing a
# refusal — `check-topic-list-scope.sh`'s own "a test setting up a fixture...
# is exempt by scope" precedent, and the reason this check greps `*.rs`
# files while excluding every `tests.rs` and `tests/` path.
#
# ⚠️ Each pipeline ends `|| true`. `lib.sh` sets `pipefail`, and a `grep`
# that matches nothing exits 1 — without this the gate dies mid-section with
# no FAIL line, `check-topic-list-scope.sh`'s own note, repeated here rather
# than relearned.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

SEAM="crates/oqueue-broker/src/fencing.rs"

if [[ ! -f "$SEAM" ]]; then
  fail "$SEAM not found -- the fencing seam this check reads is gone"
  finish
fi
ok "the fencing seam exists at $SEAM"

CODES=(
  UNKNOWN_MEMBER_ID
  ILLEGAL_GENERATION
  NOT_COORDINATOR
  COORDINATOR_NOT_AVAILABLE
  COORDINATOR_LOAD_IN_PROGRESS
)

violations=0
for code in "${CODES[@]}"; do
  mapfile -t sites < <(
    grep -rn "error_codes::${code}\b" \
      --include='*.rs' \
      crates/oqueue-broker/src/ 2>/dev/null |
      grep -v '/tests\.rs:' |
      grep -v '/tests/' |
      grep -v "^${SEAM}:" || true
  )
  if (( ${#sites[@]} > 0 )); then
    violations=$((violations + ${#sites[@]}))
    fail "error_codes::${code} constructed outside $SEAM"
    for site in "${sites[@]}"; do
      note "$site"
    done
  fi
done

if (( violations == 0 )); then
  ok "none of M4.11's five fencing codes are constructed outside $SEAM"
fi

# ── The seam itself still names all five, so a rename or a deleted arm    ──
# ── does not make this check pass by having nothing left to find         ──
missing=0
for code in "${CODES[@]}"; do
  if ! grep -q "error_codes::${code}\b" "$SEAM"; then
    missing=$((missing + 1))
    fail "$SEAM no longer constructs error_codes::${code} at all"
    note "Refusal::error_code's own match should have one arm per code"
  fi
done
if (( missing == 0 )); then
  ok "the seam itself still answers all five codes"
fi

finish
