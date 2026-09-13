#!/usr/bin/env bash
# No handler constructs one of `Refusal`'s fencing codes outside
# `crate::fencing::Refusal::error_code` — structural, not a runtime
# assertion. `check-topic-list-scope.sh`'s own style, `M9.11`'s precedent
# applied to fencing instead of the global-topic-list claim.
#
# ⚠️ **The code list is derived from the seam, not written here, and
# `M4.36` is why.** `M4.11` hardcoded its own five; the enum grew
# `RebalanceInProgress` and the array did not, so `sync_group.rs` built that
# code directly and this gate reported "none of M4.11's five fencing codes
# are constructed outside the seam" — true, and not the property anyone
# wanted. A list of names frozen at the moment a gate is written is a gate
# that checks the past. M4's closing review found it by reading this file
# against `m4-complete.sh`, which derives the same list from the same enum
# and therefore knew there were six.
#
#   scripts/check-fencing-seam.sh
#
# `crates/oqueue-broker/src/fencing.rs` is the one seam every `M4.7`-`M4.10`
# handler's own fencing decision is supposed to route through (`M4.11`'s own
# backlog row, verbatim: "no handler constructs one of these five codes
# outside the shared function" — five was the count that day, and taking it
# for the property is the defect `M4.36` repaired). A handler that reaches
# for one of those constants directly — a shortcut that answers the *right*
# code for the *wrong* reason, or drifts once a further case is added —
# would satisfy every test in `fencing/tests.rs` while quietly
# reintroducing the ad hoc `if`s the task exists to remove. Nothing else would catch that: the wire
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

# ── What the seam answers, read by a parser rather than a line scanner ───
#
# ⚠️ **`scripts/lib/fencing_seam.py` exists because four review rounds broke
# four awks**, each on something ordinary — a comment naming a code, a
# comment containing an arrow, a greedy strip taking the *last* arrow on the
# line, and finally a block-bodied arm (`Self::X => { error_codes::Y }`),
# which is rustfmt-stable and which no amount of comment stripping reaches.
# Its own module doc records the series. It emits one `variant<TAB>code`
# line per variant and fails loudly on a catch-all arm or a variant that
# answers nothing, so this script reads a fact rather than a guess.
if ! seam_out="$(python3 "$REPO_ROOT/scripts/lib/fencing_seam.py" "$SEAM" 2>&1)"; then
  fail "the fencing seam could not be read"
  while IFS= read -r line; do
    note "$line"
  done <<<"$seam_out"
  note "one arm per variant, each answering an error_codes:: path, so the code"
  note "each variant produces is derivable and this gate can check it"
  finish
fi

mapfile -t VARIANTS < <(cut -f1 <<<"$seam_out")
mapfile -t CODES < <(cut -f2 <<<"$seam_out" | sort -u)

# ⚠️ **A floor, because deriving a list creates a failure mode a hardcoded
# one did not have**: delete a variant *and* its arm together — still
# exhaustive, still compiles — and both sides of any count shrink together,
# so a gate that only counts goes green on the very shape it exists to
# catch. The old fixed list caught that by naming the codes; this replaces
# it.
#
# ⚠️ **On variants, not on codes.** Flooring the deduped code list rejects a
# seam where two variants honestly answer one wire code — six variants, six
# arms, nothing deleted — and the message then names deletion at somebody
# whose change was fine. `m4-complete.sh` floors variants for this reason.
# ⚠️ Non-negotiable 2: this number only ever goes up.
MIN_VARIANTS=6
if (( ${#VARIANTS[@]} < MIN_VARIANTS )); then
  fail "Refusal has ${#VARIANTS[@]} variant(s); this seam has had at least ${MIN_VARIANTS} since M4.11"
  note "a variant and its arm deleted together shrink every count in step, so"
  note "nothing that counts notices -- raise MIN_VARIANTS when the seam gains one,"
  note "and never lower it (non-negotiable 2)"
  finish
fi
ok "the seam answers ${#CODES[@]} code(s) across ${#VARIANTS[@]} variant(s)"

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
  ok "none of the seam's ${#CODES[@]} fencing codes are constructed outside $SEAM"
fi

finish
