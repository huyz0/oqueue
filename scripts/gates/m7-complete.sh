#!/usr/bin/env bash
# The M7 completion condition, as `M7.md` states it: a synthetic catalog at
# 1M and at 10M topics shows per-node memory flat against total cluster
# partition count (NFR-10, NFR-11); `Metadata` response size and CPU stay
# independent of catalog size, re-measured at that scale (NFR-12, inherited
# from M9); and produce cost holds at two topic counts an order of magnitude
# apart (NFR-4). Plus leg 0 (the milestone read as a whole) and leg 0b (every
# open row handed on), as `m6-complete.sh` carries them.
#
# ⚠️ **Written first, and red** (`milestone/SKILL.md`, `M5.84`): while a leg
# fails it is the statement of what is left, and the loop that drives M7 stops
# when this exits 0 — never when the backlog runs out.
#
# ⚠️ **What each leg can and cannot assert** (`ADR-0049`):
#   - **"memory flat"** is measured, not extrapolated: live bytes a node holds
#     after serving the same active topics, counted by an allocator, over a
#     synthetic catalog of 1M and of 10M topics that derives each entry on
#     lookup rather than storing it. It is memory *in the node*; the catalog's
#     own storage is object storage's, which is the point of NFR-11.
#   - **"CPU"** and **NFR-4's latency** are asserted through work counts —
#     catalog lookups and allocations per request — not wall-clock time:
#     `testing.md` rule 11 forbids timing assertions in the suite, and
#     `check-hot-path-bench.sh` puts every hot-path benchmark in M14, where the
#     harness is chosen. NFR-4's "benchmark" is therefore M14's to re-measure;
#     what this gate asserts is that nothing on the produce path does work that
#     grows with the topic count, which is what a benchmark would detect.
#
# ⚠️ **Every requirement leg is a named test run with a pass-count floor.** A
# filter that matches nothing exits 0, so the count is the check; and a leg
# whose test does not exist fails rather than skips, because this is an exit
# condition and "could not evaluate" may not read as "complete".

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
cd "$REPO_ROOT" || exit 1

# ── leg 0: M7's commits have been read as a whole ──────────────────────────
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M7 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M7 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "leg 0: scripts/check-milestone-review.sh could not be run (exit 127)"
else
  fail "leg 0: M7's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M7 to build the packet"
fi

# ── leg 0b: every row M7 leaves open is handed on ──────────────────────────
ho_rc=0
bash "$REPO_ROOT/scripts/check-milestone-handoff.sh" --milestone M7 || ho_rc=$?
if (( ho_rc == 0 )); then
  ok "leg 0b: every row M7 leaves open is handed on by a roadmap.md deferral row"
elif (( ho_rc == 3 )); then
  skip "leg 0b: M7's handoff (asked after M7's roadmap cell flipped to \`complete\`)"
elif (( ho_rc == 127 )); then
  fail "leg 0b: scripts/check-milestone-handoff.sh could not be run (exit 127)"
else
  fail "leg 0b: M7's handoff check refused (exit $ho_rc) -- its output above says why"
fi

have_cargo=1
command -v cargo >/dev/null 2>&1 || have_cargo=0

names_a_test() {
  local found
  found="$(git grep -ln -- "fn $1\b" 'crates/*/tests/*' 'crates/*/src/*' 2>/dev/null)" || return 1
  [[ -n "$found" ]] || return 1
  printf '%s' "$found"
}

run_counted() {
  local label="$1" min="$2"; shift 2
  local out passed
  if ! out="$(cargo test "$@" --quiet 2>&1)"; then
    fail "$label: the run itself failed"
    note "run: cargo test $*"
    printf '%s\n' "$out" | tail -20
    return 1
  fi
  passed="$(printf '%s\n' "$out" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')"
  if (( passed < min )); then
    fail "$label matched only $passed test(s), floor $min"
    note "a renamed, moved or #[ignore]d test leaves a filter green"
    return 1
  fi
  ok "$label ($passed test(s))"
}

# One requirement leg: the label, the test that asserts it, and what is missing
# when it does not exist yet.
leg() {
  local label="$1" test="$2" missing="$3" where
  if ! where="$(names_a_test "$test")"; then
    fail "$label: nothing asserts it -- no test named $test"
    note "$missing"
    return
  fi
  note "$label: asserted by $where"
  if (( ! have_cargo )); then
    fail "$label: a test of this name exists and no toolchain here can run it"
    return
  fi
  run_counted "$label" 1 --workspace "$test" || note "$label is not met"
}

leg "NFR-10/NFR-11 (node memory flat between a 1M and a 10M topic catalog)" \
  node_memory_is_flat_as_the_catalog_grows_tenfold \
  "needs a catalog the node looks up rather than holds, and a counting allocator"
leg "NFR-12 at scale (Metadata size and lookups flat at 1M and 10M topics)" \
  metadata_cost_is_flat_between_one_and_ten_million_topics \
  "needs M9's cost test re-run over the synthetic catalog"
leg "NFR-4 (produce cost flat across a tenfold topic count)" \
  produce_cost_is_flat_across_a_tenfold_topic_count \
  "needs a produce path that does no work proportional to the topic count"

finish
