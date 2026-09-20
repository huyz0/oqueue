#!/usr/bin/env bash
# M12's completion condition. Every leg is deliberately falsifiable: the
# required evidence is named, and a missing harness or runbook is a failure,
# never a skip that can turn an unimplemented admin surface green.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"
cd "$REPO_ROOT" || exit 1

mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M12 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M12 commit is covered by a milestone review"
else
  fail "leg 0: M12's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M12"
fi

require_script() {
  local label="$1" path="$2"
  if [[ -x "$path" ]]; then
    ok "$label: $path"
  else
    fail "$label: required executable evidence is missing: $path"
  fi
}

require_file() {
  local label="$1" path="$2"
  if [[ -f "$path" ]]; then
    ok "$label: $path"
  else
    fail "$label: required evidence is missing: $path"
  fi
}

# Real Kafka clients are the requirement's verification method. The opening
# commit intentionally fails here until M12.15 supplies the harness.
require_script "FR-53 AdminClient conformance" "$REPO_ROOT/scripts/admin-client-harness.sh"
if [[ -x "$REPO_ROOT/scripts/admin-client-harness.sh" ]]; then
  bash "$REPO_ROOT/scripts/admin-client-harness.sh" || fail "FR-53 AdminClient conformance failed"
fi

# FR-50 is about actual responsibilities, not a role string in startup output.
require_script "FR-50 role smoke" "$REPO_ROOT/scripts/role-smoke.sh"
if [[ -x "$REPO_ROOT/scripts/role-smoke.sh" ]]; then
  bash "$REPO_ROOT/scripts/role-smoke.sh" || fail "FR-50 role smoke failed"
fi

# FR-40's M4 deferral is promoted here: all five group APIs must have a
# positive cross-principal refusal test, and the test must actually run.
if command -v cargo >/dev/null 2>&1; then
  group_tests=(
    "find_coordinator::tests::a_cross_principal_group_is_refused"
    "join_group::tests::a_cross_principal_group_is_refused"
    "sync_group::tests::a_cross_principal_group_is_refused"
    "heartbeat::tests::a_cross_principal_group_is_refused"
    "leave_group::tests::a_cross_principal_group_is_refused"
  )
  for test in "${group_tests[@]}"; do
    if ! git grep -q "fn ${test##*::}" -- crates/oqueue-broker/src; then
      fail "FR-40 group authorization: missing test $test"
      continue
    fi
    out="$(cargo test -p oqueue-broker --lib "$test" --quiet 2>&1)" || {
      fail "FR-40 group authorization: $test failed"
      note "$out"
      continue
    }
    if grep -qE '[1-9][0-9]* passed' <<<"$out"; then
      ok "FR-40 group authorization: $test"
    else
      fail "FR-40 group authorization: $test ran no passing test"
    fi
  done
else
  fail "FR-40 group authorization: cargo is unavailable, so the leg cannot be evaluated"
fi

# FR-52 is a review against a written scenario list, not a metric-name claim.
require_file "FR-52 diagnostic scenario review" "$REPO_ROOT/docs/internal/operations/m12-diagnostics.md"
require_script "FR-52 diagnostic checker" "$REPO_ROOT/scripts/check-m12-diagnostics.sh"
if [[ -x "$REPO_ROOT/scripts/check-m12-diagnostics.sh" ]]; then
  bash "$REPO_ROOT/scripts/check-m12-diagnostics.sh" || fail "FR-52 diagnostic review failed"
fi

finish
