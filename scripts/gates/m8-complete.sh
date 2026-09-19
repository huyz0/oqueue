#!/usr/bin/env bash
# The M8 completion condition, as `M8.md` states it and `ADR-0050` amends it:
# BYOK data segregated so that no object holds regions from two key domains
# (FR-42); the default path's object format and cost unchanged when BYOK
# topics exist (NFR-14); KMS calls a function of DEK rotation and not of
# produce volume (NFR-33); and one seam carrying both the AWS and the GCP
# provider (FR-41). Plus leg 0 (the milestone read as a whole) and leg 0b
# (every open row handed on), as `m7-complete.sh` carries them.
#
# ⚠️ **Two clauses of `M8.md`'s condition are not asserted here, by decision**
# (`ADR-0050` points 6 and 7), and this header is where that is said rather
# than left to a reader to notice:
#   - **FR-41's round trip is against in-process simulations of each KMS
#     API**, not against real AWS KMS or real GCP Cloud KMS. There are no
#     cloud credentials in this project, and `M1.44` already deferred the same
#     class of evidence for real S3 and GCS to `M15`, which receives this too.
#     ⚠️ A simulation proves the seam, not the vendor.
#   - **FR-43 (the FIPS build) is not M8's at all.** It needs the separate
#     build job `M13.md` task 5 plans, with its cmake and Go toolchain, so the
#     runtime assertion and the differential test land there, with the
#     artifact they assert about. M8 leaves the algorithm agnostic at the
#     seam, which is what the region header's `alg` field is for.
#
# ⚠️ **Written first, and red** (`milestone/SKILL.md`, `M5.84`): while a leg
# fails it is the statement of what is left, and the loop that drives M8 stops
# when this exits 0 — never when the backlog runs out.
#
# ⚠️ **Every requirement leg is a named test run with a pass-count floor.** A
# filter that matches nothing exits 0, so the count is the check; and a leg
# whose test does not exist fails rather than skips, because this is an exit
# condition and "could not evaluate" may not read as "complete".

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
cd "$REPO_ROOT" || exit 1

# ── leg 0: M8's commits have been read as a whole ──────────────────────────
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M8 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M8 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "leg 0: scripts/check-milestone-review.sh could not be run (exit 127)"
else
  fail "leg 0: M8's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M8 to build the packet"
fi

# ── leg 0b: every row M8 leaves open is handed on ──────────────────────────
ho_rc=0
bash "$REPO_ROOT/scripts/check-milestone-handoff.sh" --milestone M8 || ho_rc=$?
if (( ho_rc == 0 )); then
  ok "leg 0b: every row M8 leaves open is handed on by a roadmap.md deferral row"
elif (( ho_rc == 3 )); then
  skip "leg 0b: M8's handoff (asked after M8's roadmap cell flipped to \`complete\`)"
elif (( ho_rc == 127 )); then
  fail "leg 0b: scripts/check-milestone-handoff.sh could not be run (exit 127)"
else
  fail "leg 0b: M8's handoff check refused (exit $ho_rc) -- its output above says why"
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

leg "FR-42 (no object holds regions from two key domains)" \
  no_object_mixes_key_domains \
  "needs per-topic key domains and a flush planner that routes by them"
leg "NFR-14 (the default path is unchanged when BYOK topics exist)" \
  byok_topics_leave_the_default_path_untouched \
  "needs BYOK segregated into its own objects, not a format every object pays"
leg "NFR-33 (KMS calls follow DEK rotation, not produce volume)" \
  kms_calls_follow_rotation_not_produce_volume \
  "needs a DEK cache rotating on a byte and a time bound"
leg "FR-41 (one seam, both providers) — simulated, see the header" \
  both_kms_providers_round_trip_through_one_seam \
  "needs the AWS and GCP KeyProvider implementations and a simulation of each"
# ⚠️ **The nonce is the one catastrophic mistake available here** (`ADR-0050`
# point 2), so it is a leg of its own even though `M8.md`'s completion
# condition does not name it: a repeated nonce under one AES-GCM key leaks
# plaintext and forges, and no other leg would notice.
leg "ADR-0050 point 2 (no call sequence can repeat a nonce)" \
  no_call_sequence_repeats_a_nonce \
  "needs the nonce to be a type no constructor can duplicate"

finish
