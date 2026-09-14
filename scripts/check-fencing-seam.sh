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

# ── No group handler answers a code its client cannot retry ────────────────
#
# ⚠️ **`M4.48`, and the rule was written down four times before it was
# enforced once.** `UNKNOWN_SERVER_ERROR` is what the Java consumer raises
# out of `poll()` and never retries, so a *give-up* path answering it ends
# the consumer rather than sending it back. `join_group` changed both of its
# own arms away from it with the reason spelled out, `Known::Superseded` did
# the same, and `sync_group`'s `Known::Waiting` was missed — reachable with
# nothing wrong, because a leader can die between the barrier closing and
# its own `SyncGroup` and nothing reaps in the background.
#
# ⚠️ **Not part of the seam above**, deliberately: this code is not a
# `Refusal` variant and should not become one — it is not a fencing
# decision. What it shares with the seam is that the rule was convention
# until something read it.
#
# ⚠️ **`offset_commit.rs` is out of scope and argued**: its own module doc
# records that a durable-log failure answers this code on purpose, per
# `behavior.md`, and that is a per-partition result rather than a give-up.
# ⚠️ **The helper's own status, not a count of what it returned** —
# `M4.50`. `M4.48` floored the total at fourteen, and review pointed out
# that a file count can lawfully shrink: folding a small module back into
# its parent is legal under `code-structure.md` rule 16, and the floor then
# leaves two exits, one of which non-negotiable 2 forbids. A *handler* with
# no source file is never legitimate while the API exists, so the helper
# refuses to answer at all in that case and this reads its status. That also
# closes the stdin hazard the floor was added for: an empty list cannot
# reach the `awk` below.
handler_list="$(group_handler_files 2>&1)" || {
  fail "a group-protocol handler has no source file"
  while IFS= read -r line; do note "$line"; done <<<"$handler_list"
  note "a handler that vanished takes its whole module tree out of every leg that"
  note "walks it, and each remaining file still reads clean"
  finish
}
mapfile -t handler_files <<<"$handler_list"

# ⚠️ **The comment is deleted before the line is looked at**, which is the
# lesson `scripts/lib/fencing_seam.py` records at length: four versions of
# that parser were defeated by a comment, and a leg matching raw text is the
# first of them. A trailing `// ... UNKNOWN_SERVER_ERROR ...` is prose about
# the rule, not an answer to a client, and failing a commit for writing one
# is a gate nobody keeps.
# ⚠️ **`awk`'s status is taken**, because a process substitution's failure
# escapes both `set -e` and `pipefail` — `mapfile < <(awk ...)` sees an empty
# list and this leg prints `ok` whatever went wrong. Review of `M4.50`
# measured it: one unreadable path among the operands aborted the scan
# having read no file, and the leg reported `ok ... file(s) read` with a
# fatal code planted. `group_handler_files` now refuses a tree it cannot
# walk, which closes that at the source; this is the floor beside it, in the
# leg, for every other reason `awk` can die.
if ! fatal_scan="$(
  awk '{
    line = $0
    sub(/\/\*.*\*\//, "", line)
    sub(/\/\/.*$/, "", line)
    if (line ~ /error_codes::UNKNOWN_SERVER_ERROR/) {
      printf "%s:%d: %s\n", FILENAME, FNR, line
    }
  }' "${handler_files[@]}"
)"; then
  fail "the UNKNOWN_SERVER_ERROR scan could not read the group handlers"
  note "it proved nothing about the ${#handler_files[@]} file(s) the walk returned"
  finish
fi
fatal=()
while IFS= read -r site; do
  [[ -n "$site" ]] && fatal+=("$site")
done <<<"$fatal_scan"
if (( ${#fatal[@]} > 0 )); then
  fail "a group handler answers UNKNOWN_SERVER_ERROR, which a client cannot retry"
  for site in "${fatal[@]}"; do
    note "$site"
  done
  note "a give-up path answers a retriable code -- REBALANCE_IN_PROGRESS is what the"
  note "siblings use, and the client rejoining is the only useful next move"
else
  ok "no group handler answers UNKNOWN_SERVER_ERROR (${#handler_files[@]} file(s) read)"
fi

finish
