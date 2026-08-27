#!/usr/bin/env bash
# The M3 completion condition, as `M3.md` states it: concurrent producers
# yield exactly `0..k` per partition with no gaps (FR-11); a kill between
# PUT and ack loses no acknowledged record (FR-10); a fetch at the high
# watermark issues zero GETs (FR-12); a cold fetch issues a bounded number
# of GETs and zero LIST (FR-13, NFR-30); a flush spanning N topics issues
# exactly one PUT (FR-32).
#
# ## Three of those five claims are not what they sound like
#
# `M3.25`'s review found two legs structurally unfalsifiable and a third
# overclaiming, and this gate is what was built instead. A green tick on a
# test that cannot fail is worse than no tick, so each of the three says
# what it actually asserts, in the line it prints:
#
#   - **Zero LIST** cannot be counted. `ObjectStore` is `get`/`put`/`delete`
#     and `Operation` has no `List` variant, so no code path can issue one
#     and no counter can count one — a runtime assertion here would be a
#     test with no failing input. `ADR-0009` §2's own argument is stronger
#     and *is* falsifiable: the property holds because the seam cannot
#     express the operation. Section 0c asserts that structurally — the
#     trait's method set and the enum's variant set — so adding `list()`
#     turns this red on the commit that adds it.
#
#   - **One PUT for N topics** is counted at the trait seam, above where
#     multipart billing happens. `put_strategy_for` turns one
#     `ObjectStore::put` into `CreateMultipartUpload` + N `UploadPart` +
#     `Complete` past the threshold, and every flush test runs against the
#     fake, which has no multipart path. So the claim is *one trait call*,
#     not one HTTP request, and the printed line says so rather than
#     letting the stronger reading stand. The per-request cost model is
#     M14's (NFR-31).
#
#   - **FR-10** says durable *in object storage*, and `M3.19` scoped the
#     ack to that. The test asserts the bytes reached the bucket and the
#     client was refused — so "no acknowledged record is lost" holds
#     because nothing was acknowledged. `ADR-0005` guarantee 1, that an
#     `Ok` survives process death, is not tested anywhere and `M15.md`
#     task 15 owns it. The line says which half ran.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — the four gates before it do the
# same. NFR-56 gives the whole pre-commit suite 10 s; this runs property
# tests over a multi-thread runtime and the workspace suite besides.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

STORE_SEAM="crates/oqueue-core/src/store.rs"
OP_SEAM="crates/oqueue-core/src/op_counts.rs"

# ── 0a. The seams the zero-LIST claim rests on exist ────────────────────────
# First and tool-free, m1-complete.sh's ordering: section 0c reads these two
# files, and a gate that would skip its only falsifiable leg because a path
# moved should say so here rather than there.
for f in "$STORE_SEAM" "$OP_SEAM"; do
  if [[ ! -f "$f" ]]; then
    fail "$f not found -- the zero-LIST claim is asserted from this file's shape"
    finish
  fi
done
ok "the object-store seam and its operation counter are where section 0c reads them"

# ── 0b. Every M3 commit read as a whole ─────────────────────────────────────
# Above every skip and every tool requirement (M1.43's finding): this check
# needs no cargo, so nothing below may gate it. The exit code is reported,
# not collapsed (m1-complete.sh's second finding).
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M3 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M3 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M3's coverage is unknown, not failing"
else
  fail "M3's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M3 to build the packet"
fi

# ── 0c. FR-13 / NFR-30: zero LIST, structurally ─────────────────────────────
#
# ⚠️ **Above the cargo guard, and that is where the negative suite put it.**
# It reads two files and runs nothing, so a machine without a toolchain must
# still be told the truth about it — the same reason section 0b sits where
# it does (`M1.43`'s finding). Written below the cargo legs first, where a
# `skip` for a missing workspace swallowed it and the gate exited 0 with the
# only falsifiable leg unrun -- which `tests/gates/negative.sh` caught.
#
# ⚠️ **Not a test, and that is the point.** `ADR-0009` §2 puts listing on a
# `MaintenanceStore` seam that does not exist, so the read path cannot LIST
# because the trait it holds cannot say it. A runtime assertion would have
# no failing input; this one does — add `list` to either file and it fails.
#
# ⚠️ **Three things, not one**, because the first version of this check
# asserted one and round one's reviewer defeated the other two.
#
# 1. The trait's *declaration line*, so a supertrait cannot smuggle the
#    method in. `pub trait ObjectStore: Listing` gives every holder of a
#    `dyn ObjectStore` a `.list()` while the method list below stays three
#    long, and this gate's printed sentence is "no holder of it can LIST".
# 2. Its method set, `unsafe fn` and `async fn` included — the modifier
#    list is what the first version got wrong, and matching `fn` after any
#    of them is what closes it.
# 3. The counter's variant set, by *name*, so `List(ObjectKey)` and
#    `List { prefix }` — the shapes a `MaintenanceStore` counter would
#    actually take — are seen. A regex anchored on the trailing comma sees
#    neither, which is the exact variant this half exists to catch.
#
# ⚠️ Each pipeline ends `|| true`. `lib.sh` sets `pipefail`, and a `grep`
# that matches nothing exits 1 — without this the gate dies mid-section
# with no FAIL line, no note, and no summary, on the refactor most likely
# to break the scan.

trait_decl="$(grep -n '^pub trait ObjectStore' "$STORE_SEAM" | head -1 | cut -d: -f2- || true)"
if [[ "$trait_decl" == "pub trait ObjectStore: Send + Sync + core::fmt::Debug {" ]]; then
  ok "zero LIST: ObjectStore's supertraits are Send + Sync + Debug, none of which can list"
else
  fail "zero LIST: ObjectStore's declaration is '$trait_decl', not the pinned one"
  note "a supertrait is a method every holder of this trait gets -- see ADR-0009 §2"
  note "changing the bounds is a decision to record, not a line to widen"
fi

store_methods="$(
  awk '/^pub trait ObjectStore/,/^}/' "$STORE_SEAM" |
    grep -oE '^ *(async |unsafe |const )*fn [a-z_]+' |
    awk '{print $NF}' | sort | tr '\n' ' ' || true
)"
if [[ "$store_methods" == "delete get put " ]]; then
  ok "zero LIST: ObjectStore is exactly get/put/delete, so no holder of it can LIST"
else
  fail "zero LIST: ObjectStore's method set is '$store_methods', not 'delete get put '"
  note "NFR-30 rests on the seam being unable to express a LIST -- see ADR-0009 §2"
  note "a new method here needs that decision revisited, not this line widened"
fi

# By name, and the name only: a variant is `Get,` or `List(K),` or
# `List { prefix }`, and all three must be visible here.
op_variants="$(
  awk '/^pub enum Operation/,/^}/' "$OP_SEAM" |
    grep -oE '^ *[A-Z][A-Za-z0-9_]*' |
    tr -d ' ' | sort | tr '\n' ' ' || true
)"
if [[ "$op_variants" == "Delete Get Put " ]]; then
  ok "zero LIST: Operation is exactly Get/Put/Delete, so no counter can record one"
else
  fail "zero LIST: Operation's variants are '$op_variants', not 'Delete Get Put '"
  note "the enum and the trait must agree -- see op_counts.rs's own doc"
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# ⚠️ `m2-complete.sh`'s helper, and the reason is the same: a cargo test
# filter that matches nothing exits 0 ("0 passed; N filtered out"), so a leg
# named by filter alone goes green the day a refactor renames the test —
# which is the exact failure these legs exist to catch. The pass count is
# parsed and held to a floor.
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
# The legs below name individual tests; this is what says the tree they run
# in is green. No --all-features: M3 adds no feature-gated path, and turning
# on the codec features here would be m2-complete.sh's leg run twice.
if cargo test --workspace --quiet >/dev/null 2>&1; then
  ok "cargo test --workspace"
else
  fail "cargo test --workspace failed"
  note "run it directly for the failure detail"
  finish
fi

# ── 2. FR-11: concurrent producers yield exactly 0..k, no gaps ──────────────
# The property test, not an example: `proptest` drives up to 16 producers of
# 1..=8 records each through one partition on a 4-worker runtime, flattens
# every `(base_offset, record_count)` it got back, and asserts the union is
# exactly `0..total`. Set-shaped, so it refuses a gap, an overlap and a
# duplicate with the same assertion.
run_counted "FR-11: concurrent producers over one partition yield exactly 0..k" 1 \
  -p oqueue-coordinator --test it \
  assignment::concurrent_producers_over_one_partition_yield_exactly_zero_to_k || finish

# And the allocator's half of it: a journal that refuses must consume
# neither a version nor an offset, or the retry lands at 1 and FR-11's gap
# arrives through the error path rather than the concurrent one.
run_counted "FR-11: a refused journal consumes neither a version nor an offset" 1 \
  -p oqueue-coordinator --test it \
  sequencing::a_refused_journal_consumes_neither_a_version_nor_an_offset || finish

# ── 3. FR-10: a kill between PUT and ack loses no acknowledged record ───────
# ⚠️ **Scoped, and the line says so.** `M3.19` scoped the ack to durable in
# object storage: the test arms `crash_after_put_before_ack`, then asserts
# the bytes ARE in the bucket, the client got `NotEnoughReplicas`, and the
# high watermark did not move. So nothing was acknowledged, and the claim
# holds for that reason rather than by surviving a crash.
run_counted "FR-10 (injectable half): a lost ack after a durable write is still refused" 1 \
  -p oqueue-broker --lib \
  produce::answer::tests::a_produce_whose_ack_was_lost_after_a_durable_write_is_still_refused || finish

# The fault itself, pinned where it lives — otherwise the test above could
# pass against a fake that quietly stopped writing before it reported failure.
run_counted "FR-10: the injected crash writes the object and still reports failure" 1 \
  -p oqueue-core --test it fault::crash_after_put_writes_the_object_but_reports_failure || finish

# And the conformance case that says a failed PUT is not proof of absence,
# which is the property a caller depends on. It runs inside the fake's suite
# rather than as a test of its own, so the floor is on that suite.
run_counted "FR-10: the fake passes the conformance suite, ack-loss case included" 1 \
  -p oqueue-store --test it fake::fake_passes_the_full_conformance_suite || finish

note "FR-10 is asserted for the injectable half only -- ADR-0005 guarantee 1, that an Ok survives"
note "process death, is untested here and M15.md task 15 owns it"

# ── 4. FR-12: a fetch at the high watermark issues zero GETs ────────────────
# Both ends, because either alone is weaker than it reads: the wire test
# samples `Operation::Get` across a real fetch, and the handler test pins
# the same claim where the decision is made.
run_counted "FR-12: a fetch at the high watermark issues no GETs (over the wire)" 1 \
  -p oqueue-broker --test it roundtrip::a_fetch_at_the_high_watermark_issues_no_gets || finish
run_counted "FR-12: a fetch at the watermark is empty and issues no GETs (handler)" 1 \
  -p oqueue-broker --lib \
  fetch::partition::tests::a_fetch_at_the_watermark_is_empty_and_issues_no_gets || finish

# The index seam's half: no `ObjectRef` is handed out at the watermark, so
# there is nothing a GET could be issued against.
run_counted "FR-12: a reader at the high watermark is given no object to read" 1 \
  -p oqueue-coordinator --test it tail::a_reader_at_the_high_watermark_is_given_no_object_to_read || finish

# ── 6. FR-13 / NFR-30: a cold fetch is bounded ──────────────────────────────
# The LIST half is section 0c, above, because it needs no cargo. The strongest case lands 20 commits under a parked fetch and
# asserts exactly `MAX_READS_PER_REQUEST` GETs — a literal, deliberately,
# because deriving it from the constant would assert the constant against
# itself; `check-drift.sh` pins the value.
run_counted "FR-13: a request reads a bounded number of times however much is committed" 1 \
  -p oqueue-broker --lib \
  fetch::park::tests::a_request_reads_a_bounded_number_of_times_however_much_is_committed || finish

# The cold tier's own cases: a page past the tail window, a per-partition
# cap that does not buy a read per entry, and the charge being what a read
# fetched rather than what it returned.
run_counted "FR-13: the cold read tier stays bounded and is charged what it fetched" 3 \
  -p oqueue-broker --test it reads:: || finish

# ⚠️ **Both implementations, and the floor says two.** The contract suite
# runs once per index, and naming only the memory one would let the fake's
# page bound regress under a line reading "every implementation".
run_counted "NFR-30: the index's own page bound holds for every implementation" 2 \
  -p oqueue-index --test it satisfies_the_contract || finish

# ── 7. FR-32: a flush spanning N topics issues exactly one PUT ──────────────
# ⚠️ **One trait call, not one HTTP request**, and the note below says so.
# Both cases re-read what they wrote, so "one PUT that lost three topics"
# cannot pass either of them.
run_counted "FR-32: one produce spanning four topics issues exactly one PUT (over the wire)" 1 \
  -p oqueue-broker --test it roundtrip::one_produce_spanning_four_topics_issues_exactly_one_put || finish
run_counted "FR-32: bundling four topics costs one store call where four objects cost four" 1 \
  -p oqueue-core --test it \
  bundle::bundling_four_topics_costs_one_store_call_where_four_objects_cost_four || finish

note "FR-32 counts ObjectStore::put at the trait seam, above where multipart bills: past"
note "put_strategy_for's threshold one trait call is Create + N UploadPart + Complete, and no"
note "test pins the flush's PutStrategy. The per-request cost model is M14's (NFR-31)"

# ── 8. The verdict ──────────────────────────────────────────────────────────
# ⚠️ Two of the five legs are asserted narrower than `M3.md`'s sentence
# reads, and the final line refuses to launder that: it names them.
if (( _FAILURES == 0 )); then
  ok "M3 completion condition holds, with FR-10 scoped to the injectable half and FR-32 to the trait seam"
fi
finish
