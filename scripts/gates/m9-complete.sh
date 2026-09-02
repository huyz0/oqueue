#!/usr/bin/env bash
# The M9 completion condition, as `M9.md` states it: cross-principal access
# is refused on every implemented API (FR-40); a principal sees only its
# own topics in `Metadata`, including for a null topic array (FR-4);
# response size and CPU are independent of catalog size at two catalog
# sizes an order of magnitude apart (NFR-12); and a log scan over the
# end-to-end suite finds no credential, key, or token material (FR-44).
#
# ## Each leg is falsifiable or says it is not
#
# `M3.16`/`M10.15`/`M11.12`'s precedent: a leg that cannot fail is worse
# than no leg, so each one below says what it actually asserts.
#
#   - **"every implemented API"** (leg 2) is `cross_principal.rs`'s own
#     sweep — `Metadata` (both shapes), `Produce`, `Fetch`, `ListOffsets` —
#     not literally every API this broker answers. `SaslHandshake`/
#     `SaslAuthenticate` are the pre-authentication trio `M9.7`'s own
#     authorization decision point exempts by construction, and
#     `InitProducerId`/`ApiVersions` carry nothing to scope by topic. A
#     ninth API added later that scopes by topic needs its own row in
#     `cross_principal.rs`, which nothing here enforces structurally.
#   - **"a log scan finds no credential, key, or token material"** (leg 4)
#     is scoped exactly as `M9.15`'s own docstring names it: no crate in
#     this workspace initializes a `tracing` subscriber yet, so this leg
#     scans every text surface the suite can currently observe a node
#     produce (wire reply bytes, `Dispatcher`'s own `Debug`) — not a
#     literal process log, which does not exist until something emits one.
#   - **"CPU independent of catalog size"** (leg 3) is asserted through
#     `Cluster::topic_lookups`, a call-count proxy — `testing.md` rule 11
#     forbids asserting on wall-clock duration, so nothing here is a
#     benchmark.
#
# ## What this does not assert
#
# `M9.16` (rate limiting and quotas, FR-45) is real work this milestone
# shipped, but `M9.md`'s own completion condition — quoted above, verbatim
# — never named it, and neither does the roadmap's own four-leg summary. A
# leg here for FR-45 would assert something true and irrelevant to whether
# *this* condition holds; FR-45's own gate is `crates/oqueue-broker/tests/it/quota.rs`
# and `oqueue-core`'s `principal_quota` unit tests, run by the ordinary
# workspace suite below rather than singled out here.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — the seven gates before it do the
# same. `NFR-56` gives the whole pre-commit suite 10 s; this runs the
# workspace suite and `check-milestone-review.sh` besides.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

# ── 0. Every M9 commit read as a whole ──────────────────────────────────────
# Above every skip and every tool requirement (M1.43's finding,
# m3-complete.sh's precedent): this check needs no cargo, so nothing below
# may gate it.
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M9 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M9 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M9's coverage is unknown, not failing"
else
  fail "M9's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M9 to build the packet"
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# `m2-complete.sh`'s helper, `m3-complete.sh`/`m10-complete.sh`/
# `m11-complete.sh`'s precedent: a cargo test filter that matches nothing
# exits 0 ("0 passed; N filtered out"), so a leg named by filter alone goes
# green the day a refactor renames the test. The pass count is parsed and
# held to a floor.
run_counted() {
  local label="$1" min="$2"; shift 2
  local out passed
  if ! out="$(cargo test "$@" --quiet 2>&1)"; then
    fail "$label failed"
    note "run: cargo test $*"
    return 1
  fi
  passed="$(printf '%s\n' "$out" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')"
  if (( passed < min )); then
    fail "$label matched only $passed test(s), floor $min -- a renamed test leaves a filter green"
    note "run: cargo test $*"
    return 1
  fi
  ok "$label ($passed test(s))"
}

# ── 1. The workspace suite ──────────────────────────────────────────────────
if cargo test --workspace --quiet >/dev/null 2>&1; then
  ok "cargo test --workspace"
else
  fail "cargo test --workspace failed"
  note "run it directly for the failure detail"
  finish
fi

# ── 2. FR-40: cross-principal access is refused on every scoped API ────────
# `cross_principal.rs`'s own sweep -- `Metadata` (both shapes), `Produce`,
# `Fetch`, `ListOffsets` -- through the real `Dispatcher`, not a sample.
run_counted "FR-40: every scoped API refuses a cross-principal topic" 1 \
  -p oqueue-broker --test it \
  cross_principal::every_scoped_api_refuses_a_cross_principal_topic || finish

# ── 3. FR-4: a principal sees only its own topics, including the null array ─
# ⚠️ **The explicitly-named shape is `TOPIC_AUTHORIZATION_FAILED`; the
# null-topic-array shape is silent omission** -- `M9.1`'s verified Kafka
# finding, `M9.9`/`M9.10`'s split. Both are exercised by leg 2's own sweep
# above; this leg is the null-array shape's own dedicated unit test.
run_counted "FR-4: a null-topic-array Metadata answers exactly the principal's own grants" 1 \
  -p oqueue-broker --lib \
  metadata::tests::authorization::a_null_topic_array_answers_exactly_the_principals_own_grants || finish

# ── 4. NFR-12: Metadata response size and CPU independent of catalog size ──
# ⚠️ **"CPU" without a stopwatch** -- `testing.md` rule 11 forbids asserting
# on wall-clock duration. `M9.17`'s own `Cluster::topic_lookups` counter is
# the proxy this leg's test reads instead.
run_counted "NFR-12: Metadata response size and lookup cost do not grow with catalog size" 1 \
  -p oqueue-broker --test it \
  metadata_cost::metadata_response_size_and_lookup_cost_do_not_grow_with_catalog_size || finish

# ── 5. FR-44: the log scan finds no credential, key, or token material ─────
# ⚠️ Scoped exactly as `M9.15`'s own docstring names -- see this file's own
# header for why "log" here means every text surface the suite can
# currently observe, not a literal process log.
run_counted "FR-44: the SASL/PLAIN exchange never leaks the password, through the real Dispatcher" 1 \
  -p oqueue-broker --test it \
  secrets_log_scan::sasl_plain_exchange_never_leaks_the_password || finish

# ── 6. The verdict ───────────────────────────────────────────────────────────
if (( _FAILURES == 0 )); then
  ok "M9 completion condition holds -- FR-40, FR-4 (including the null-array"
  ok "shape), NFR-12, and FR-44, each scoped exactly as this file's own header names"
fi
finish
