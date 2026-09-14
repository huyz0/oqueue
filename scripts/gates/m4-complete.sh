#!/usr/bin/env bash
# The M4 completion condition, as `M4.md` states it: multiple consumers join
# and leave a group and complete rebalances against **librdkafka and the Java
# client** (FR-20); the offset-survival leg *reports* rather than asserts
# (FR-21, see below); every fencing error is reachable in a test rather than
# merely defined; and cross-principal access is refused on the APIs this
# milestone adds (FR-40).
#
# ⚠️ **And one leg beyond `M4.md`'s own sentence: the TLS + SASL/PLAIN round
# trip** (`M4.37`). `M4.18` is the commit that first makes `M9`'s security
# mechanism reachable in the shipped binary at all — `M9` built `tls::acceptor`,
# `tls_terminated`, `with_credentials`, `with_topic_grants` and `with_quota`
# and wired none of them — so a run on a host without `openssl` asserted none
# of it while this gate said the condition held. `m11-complete.sh` gates on
# `idempotent-conformance` for the identical reason.
#
# ## Each leg is falsifiable or says it is not
#
# `M3.16`/`M10.15`/`M11.12`'s precedent, and the reason this file is longer
# than its leg count: a leg that cannot fail is worse than no leg, so each
# says what it actually asserts rather than what the milestone's prose
# suggests.
#
#   - **FR-20 is asserted against real clients, not against the unit suite.**
#     The unit suite is green on brokers that no real consumer can use:
#     `M4.29` found every group's first round waiting out `rebalance_timeout_ms`
#     (five minutes with either client's defaults) and `M4.17` found two more
#     defects — a newcomer becoming leader, and a follower answered from the
#     previous generation's assignment — none of which 280 passing tests saw.
#     So the FR-20 leg is `kafka-client-harness.sh`'s two group legs, and a
#     missing client is a **skip**, not a pass.
#   - **FR-21's restart leg is reported, never asserted, and this gate
#     asserts the reporting.** `serve` wires `FakeGroupMetadataLog`, whose
#     entries are an in-memory `Vec`, so no committed offset survives a
#     process restart — measured by `M4.17` at offset 5 before and `-1001`
#     after. That is a recorded deferral (`ADR-0035`; `roadmap.md`'s deferred
#     table, received by `M6.md` task 7c), so asserting survival here would
#     be asserting a requirement the tree does not meet. ⚠️ **The harness
#     fails if the offsets ever DO survive**, which is what stops this
#     deferral being discharged silently: the day `M6` lands, that leg goes
#     red and says to promote it.
#   - **The fencing leg names the tests that drive `fencing::Refusal`, and
#     checks every variant is among them.** ⚠️ Two earlier versions were
#     wrong in ways worth recording, because both looked right: the first
#     compared variant count against match-arm count, which asserts only what
#     `rustc` already enforces for an exhaustive match; the second named
#     `refusals::every_refusal_answers_the_code_its_cause_deserves`, which
#     drives `join_group::refusal_code` over `JoinOutcome` and never touches
#     `fencing::Refusal` at all — so the leg reported "every fencing error is
#     reachable in a test" while naming a test that reaches none of them.
#     Both found by review. It now greps each variant out of the enum and
#     requires `fencing/tests.rs` to name it.
#   - ⚠️ **`check-fencing-seam.sh` *is* what makes `error_code` the only
#     constructor**, and this paragraph said the opposite until `M4.49`. It
#     claimed that gate "lists five codes", that `sync_group.rs` constructs
#     `REBALANCE_IN_PROGRESS` directly, and that the seam is "a convention
#     this gate neither enforces nor pretends to". `M4.36` made all three
#     false in one commit: the list is derived from the `Refusal` enum by
#     `scripts/lib/fencing_seam.py` rather than frozen in the script, that
#     construction was routed through the seam, and the hook is named
#     `no handler constructs any Refusal code outside crate::fencing`.
#     ⚠️ **Worse than stale for as long as it stood** — `M4.36`'s own row
#     records that the violation it fixed was found by reading this file
#     against that gate, so the paragraph was handing the next reader the
#     belief that produced the bug. Today the seam answers six codes across
#     six variants, and `M4.48` added a second leg refusing
#     `UNKNOWN_SERVER_ERROR` anywhere in the five group handlers' module
#     trees. ⚠️ **What that gate does *not* reach, so this paragraph does not
#     become the next over-trust**: the derivation matches the literal
#     `error_codes::NAME` spelling under `crates/oqueue-broker/src/`, so any
#     handler that keeps that spelling off every line passes — a grouped
#     import (`error_codes::{self, REBALANCE_IN_PROGRESS}`), a module alias
#     (`use oqueue_codec::error_codes as ec;`), or the bare numeric literal.
#     ⚠️ The *ordinary* import is caught, because
#     `use oqueue_codec::error_codes::REBALANCE_IN_PROGRESS;` carries the
#     spelling itself; and `fencing_seam.py`'s `VARIANT_RE` requires
#     the variant name be followed by `[({,]` or end of line, so a variant
#     given an explicit discriminant (`FencedInstance = 9,`) never enters the
#     derived list at all. `M4.56` is the open row for the parser's gaps.
#   - **FR-40 is re-asserted for M4's own APIs, and that is not M9's job.**
#     `m9-complete.sh` asserts FR-40 over every API implemented *as of M9*,
#     which predates all seven of these. `OffsetCommit` and `OffsetFetch`
#     name a topic and check `TopicGrants`; both are held here.
#   - ⚠️ **The other five are UNMET, and this gate reports that rather than
#     waiving it.** `FindCoordinator`, `JoinGroup`, `SyncGroup`, `Heartbeat`
#     and `LeaveGroup` scope on `GroupId`, which has no principal component —
#     principal A can join principal B's group, be elected leader, read every
#     member's subscription metadata, and hold partitions B's consumers are
#     waiting for. `M4.md`'s completion condition asks for all seven. An
#     earlier version of this gate waived the five in a comment with no
#     runtime line, which is a completion gate calling a milestone finished
#     against a requirement it does not meet; review caught it. The deferral
#     is recorded (`roadmap.md`, received by `M12.md` task 3a) and the leg
#     **fails if any of the five gains a principal check**, which is the
#     signal to promote it — the same both-directions discipline FR-21's leg
#     uses below.
#   - **FR-22 (KIP-848) is deliberately not asserted.** `ADR-0033` deferred
#     it in `M4.0`; `M4.22` annotated `roadmap.md`'s coverage row and `M4.24`
#     moved the register itself to `deferred`. Nothing is outstanding.
#
# ## Where this runs, and why not in pre-commit
#
# Standalone, at the milestone boundary — the eight gates before it do the
# same. `NFR-56` gives the whole pre-commit suite 10 s; this runs the
# workspace suite and drives two real Kafka clients over TCP besides.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

# ── 0. Every M4 commit read as a whole ──────────────────────────────────────
# Above every skip and every tool requirement (`M1.43`'s finding,
# `m3-complete.sh`'s precedent): this needs no cargo, so nothing below may
# gate it.
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M4 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "every M4 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M4's coverage is unknown, not failing"
else
  fail "M4's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M4 to build the packet"
fi

if ! has_rust; then
  skip "everything below (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish

# run_counted <label> <min-tests> <cargo test args...>
#
# `m2-complete.sh`'s helper, unchanged: a cargo test filter that matches
# nothing exits 0 ("0 passed; N filtered out"), so a leg named by filter alone
# goes green the day a refactor renames the test. The pass count is parsed and
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

# ── 2. FR-40: cross-principal access is refused on the APIs M4 adds ─────────
# ⚠️ **Re-asserted here because `m9-complete.sh` cannot have covered it**: its
# own FR-40 leg runs over every API implemented *as of M9*, and all seven of
# these postdate it. The two that carry a topic are the two that can leak.
run_counted "FR-40: OffsetCommit refuses a cross-principal topic" 1 \
  -p oqueue-broker --lib -- \
  offset_commit::tests::a_commit_from_principal_a_cannot_land_under_principal_bs_topic || finish
run_counted "FR-40: OffsetFetch's all-topics form hides another principal's topics" 2 \
  -p oqueue-broker --lib -- \
  offset_fetch::tests::all_topics_never_returns_a_topic_committed_by_another_principal \
  offset_fetch::tests::an_unauthorized_explicit_topic_is_refused_per_partition || finish

# ── 2b. FR-40 on the five group APIs: reported, and unmet ──────────────────
# ⚠️ **Reported rather than waived.** `M4.md` asks for cross-principal refusal
# on each of the seven APIs this milestone adds. Two of them name a topic and
# check `TopicGrants`; the other five scope on `GroupId`, which carries no
# principal, so there is nothing to refuse *with*. A completion gate that
# passed over that in a comment would be calling the milestone finished
# against a requirement it does not meet.
#
# ⚠️ **It fails the day one of them gains a principal check**, which is the
# signal to promote this to an assertion — FR-21's leg below works the same
# way, and it is the only thing that stops a recorded deferral being
# discharged without anyone noticing.
# ⚠️ **Every file of the five handlers, not five named files** — `M4.50`.
# This loop opened `join_group/mod.rs` and called it the `JoinGroup`
# handler; `code-structure.md` rule 16 has split the five across fourteen
# modules, and `sync_group/barrier.rs` was created by `M4.43` *after* this
# leg was written. Measured before the fix: a `group_authorized(..)` taking
# an `AuthzContext` appended to `barrier.rs` left this leg printing "unmet
# and reported". M12 task 3a can land its check in `round/plan.rs`, where
# the roster is built, and a closed milestone's gate would go on reporting
# the deferral open forever. `group_handler_files` is the shared walk, and
# it refuses to answer at all if a handler has no source file.
handler_list="$(group_handler_files 2>&1)" || {
  fail "a group-protocol handler has no source file -- FR-40's tripwire cannot run"
  while IFS= read -r line; do note "$line"; done <<<"$handler_list"
  finish
}
# ⚠️ **A floor on what was actually opened, which `M4.23`'s own review asked
# for on this leg and nobody applied** — its words were "the missing floor
# is `(( read < 5 ))`", and the leg shipped without it through four more
# reviews. Without it the glue is unguarded and the leg reports `ok ...
# across 14 file(s)` having read none: measured with an unresolvable
# `$path`, where every file silently fails to be opened and `scoped` stays
# empty. ⚠️ **Dropping the `$REPO_ROOT/` prefix is *not* that failure** — the
# gate `cd`s to `$REPO_ROOT` at the top and the walk returns repo-relative
# paths, so the bare form still resolves and this leg still fires. Counting
# what the loop opened, and comparing it to what the walk returned, is what
# no glue mistake between the two can survive.
# ⚠️ `tr -d ' '` because BSD `wc` pads its count, and this number is printed
# in a failure line an operator reads (`portability.md` rule 2).
expected="$(wc -l <<<"$handler_list" | tr -d ' ')"
scoped=""
read_files=0
while IFS= read -r handler; do
  path="$REPO_ROOT/$handler"
  if [[ ! -f "$path" ]]; then
    # ⚠️ `continue`, not `finish`: naming every unreadable path is worth more
    # than the first one, and the floor below is what actually stops the leg
    # reaching `ok`. A `finish` here would make that floor unreachable.
    fail "FR-40 tripwire could not read $handler"
    continue
  fi
  read_files=$((read_files + 1))
  # ⚠️ **Several spellings, because one identifier is not the property.** A
  # `GroupGrants` check written in this repo's own idiom —
  # `group_authorized(&request.group_id, authz)` taking an `AuthzContext` —
  # contains no lowercase `principal` at all, so M12 task 3a could land and
  # this leg would still print "unmet and reported". It errs toward a false
  # *failure*, the safe direction for a tripwire whose job is to notice that
  # a deferral became dischargeable. Found by review.
  if grep -qE '(principal|Principal|AuthzContext|authz::|_authorized\()' "$path"; then
    scoped="$scoped ${handler#crates/oqueue-broker/src/}"
  fi
done <<<"$handler_list"
if (( read_files != expected )); then
  fail "FR-40 tripwire read ${read_files} file(s) of ${expected} -- it proved nothing"
  finish
fi
if [[ -n "$scoped" ]]; then
  fail "group API(s) now reference a principal:$scoped -- FR-40 may be satisfiable for them"
  note "promote this leg to an assertion, and close roadmap.md's GroupGrants"
  note "deferral (M12.md task 3a) if all five are covered"
else
  ok "FR-40 on the five group APIs: unmet and reported across ${read_files} file(s) (GroupGrants, M12.md task 3a)"
  note "GroupId carries no principal, so JoinGroup/SyncGroup/Heartbeat/LeaveGroup/"
  note "FindCoordinator cannot refuse a cross-principal caller; deferred, not waived"
fi

# ── 3. Every fencing error is reachable in a test ───────────────────────────
# ⚠️ **Each variant is looked for in the tests that drive the seam.** See this
# file's header for the two earlier versions of this leg that were wrong: one
# asserted what `rustc` already enforces, the other named a test that does not
# touch `fencing::Refusal`. A variant added without a test naming it fails
# here — which is what "reachable in a test" has to mean.
fencing="$REPO_ROOT/crates/oqueue-broker/src/fencing.rs"
fencing_tests="$REPO_ROOT/crates/oqueue-broker/src/fencing/tests.rs"
untested=""
count=0
while read -r variant; do
  [[ -z "$variant" ]] && continue
  count=$((count + 1))
  # ⚠️ `Err(Refusal::X)` rather than the bare name: a variant merely
  # *mentioned* in a comment or a helper would satisfy a looser grep without
  # any test driving `fence` to produce it. Found by review.
  grep -qE "Err\(Refusal::${variant}\)" "$fencing_tests" || untested="$untested $variant"
done < <(awk '/^pub\(crate\) enum Refusal \{/{inside=1; next}
              inside && /^\}/{inside=0}
              inside && /^[[:space:]]+[A-Z][A-Za-z0-9]*[[:space:]]*[({,]/{
                  sub(/^[[:space:]]+/,""); sub(/[[:space:]({,].*$/,""); print}' "$fencing")
if (( count < 6 )); then
  fail "found only $count fencing refusal variant(s) — the enum moved, and this leg is reading the wrong thing"
elif [[ -n "$untested" ]]; then
  fail "fencing refusal(s) no test drives:$untested"
  note "every variant must appear in crates/oqueue-broker/src/fencing/tests.rs"
else
  ok "every fencing refusal ($count) is driven by a test in the fencing seam's own suite"
fi
run_counted "the fencing seam's own suite" 6 \
  -p oqueue-broker --lib -- fencing::tests:: || finish
# The mapping from a *join* outcome to a wire code is a second, adjacent
# property — `M4.15d` spent four rounds getting it right — and its own test.
run_counted "every join refusal answers the code its cause deserves" 1 \
  -p oqueue-broker --lib -- \
  refusals::every_refusal_answers_the_code_its_cause_deserves || finish

# ── 4. FR-20: real clients complete rebalances ─────────────────────────────
# ⚠️ **The harness, not the unit suite, and a missing client is a skip.**
# `m1-complete.sh`'s discipline. This is the leg the milestone exists for, and
# `M4.17` is why it is not `cargo test`: the unit suite was green on a broker
# no real consumer could use.
#
# ⚠️ **The roster is read, not the exit code.** `kafka-client-harness.sh`
# records each client that actually VERIFIED in `target/harness/clients.txt`
# (`m2-complete.sh`/`m11-complete.sh`'s own mechanism), so a run where every
# client skipped exits 0 and must not read as FR-20 satisfied here.
harness_rc=0
bash "$REPO_ROOT/scripts/kafka-client-harness.sh" || harness_rc=$?
roster="$REPO_ROOT/target/harness/clients.txt"
if (( harness_rc != 0 )); then
  fail "the client harness failed (exit $harness_rc)"
  note "run: scripts/kafka-client-harness.sh"
elif [[ ! -f "$roster" ]]; then
  fail "the harness left no roster at target/harness/clients.txt"
else
  for leg in librdkafka-groups java-groups; do
    if grep -qx "$leg" "$roster"; then
      ok "FR-20: $leg drove join, add and remove with no partition assigned twice"
    else
      skip "FR-20 via $leg (that client did not run -- the harness said why)"
    fi
  done
  # ⚠️ **`M4.18`'s own leg, and `M4.37` is why it is read here.** The harness
  # records `tls-sasl` on the roster and, until this row, nothing read it:
  # `M4.18` is the commit that first makes FR-4, FR-40 and FR-45 reachable in
  # the shipped binary at all — `M9` built every mechanism and wired none —
  # so a host without `openssl` asserted none of it while this gate said the
  # completion condition held. `m11-complete.sh` gates on
  # `idempotent-conformance` for the identical reason, in as many words:
  # `have_librdkafka` alone would pass on the ordinary round trip while the
  # leg that milestone exists for silently never ran.
  if grep -qx 'tls-sasl' "$roster"; then
    # ⚠️ **FR-40, not FR-4 or FR-45, and review of `M4.37` had to say so.**
    # What the leg drives is one principal, one topic, one message: it
    # falsifies authentication, and the *wiring* of the grant path — the
    # broker is started with `OQUEUE_TOPIC_GRANTS` naming
    # `<principal>:<topic>` and the produce succeeds, where before `M4.18`
    # no grant was consulted at all. ⚠️ **It does not falsify topic
    # authorization**, which this comment claimed until `M4.55`: nothing in
    # `tls_sasl.py` asks for a topic the principal has no grant for, so a
    # broker that ignored grants entirely would pass this leg. The
    # cross-principal refusal is leg 2's, on `OffsetCommit` and
    # `OffsetFetch`. ⚠️ And this leg cannot
    # falsify FR-4 (Metadata scoped to the principal — no Metadata response
    # is inspected) or FR-45 (quota isolation — no second principal, and
    # nothing approaches `OQUEUE_MAX_IN_FLIGHT`). Naming those two would
    # have made this the only completion gate printing a green FR-45 line
    # for a run that drives no quota, which is precisely what
    # `m9-complete.sh` declines to do and says why.
    ok "FR-40: a real client authenticated over TLS, and was refused without a credential"
  else
    skip "the TLS + SASL/PLAIN leg did not run (no openssl, or no confluent-kafka)"
  fi
fi

# ── 5. FR-21: the offset-survival leg reports, and is not asserted ──────────
# ⚠️ **This leg asserts the *reporting*, deliberately.** See this file's
# header: offsets do not survive a process restart and cannot until `M6.md`
# task 7c lands, so a gate asserting survival would assert a requirement the
# tree does not meet. What must be true is that somebody measured it and said
# so out loud — and that the harness fails, rather than quietly passing, the
# day it starts surviving.
survival="$REPO_ROOT/target/harness/offset-survival.log"
# ⚠️ **Empty means "did not run", and that is round 1's own fix biting
# back.** The harness truncates this log every run so a stale one cannot pass
# for a fresh measurement — which leaves a host without `confluent-kafka`
# holding a file that is present and empty. A bare `-f` test then falls
# through to "reported neither outcome" and **fails** M4's gate for a missing
# client. A skip must stay a skip (`portability.md` rule 10). Found by review.
if [[ ! -s "$survival" ]]; then
  skip "FR-21 offset-survival reporting (the harness leg did not run)"
elif grep -qx "OFFSETS SURVIVED" "$survival"; then
  fail "committed offsets now survive a broker restart -- FR-21 is satisfiable"
  note "promote scripts/harness/offset_survival.py's leg to an assertion, and"
  note "close roadmap.md's durable-GroupMetadataLog deferral (M6.md task 7c)"
elif grep -qx "OFFSETS LOST" "$survival"; then
  ok "FR-21: offset survival is measured and reported as unmet (M6.md task 7c)"
  note "$(grep -E '^committed offset' "$survival" | tail -2 | tr '\n' ' ')"
else
  fail "the offset-survival leg reported neither outcome"
  note "$(tail -5 "$survival" 2>/dev/null || true)"
fi

# ── 6. The verdict ─────────────────────────────────────────────────────────
# ⚠️ **`m2-complete.sh`'s discipline, which leg 4's own comment cites and two
# earlier versions of this file did not implement**: the completion condition
# names *both* clients, so a run where either skipped has not asserted FR-20 —
# and without this the gate exited 0 on a host where neither ran, which is the
# one leg this milestone exists for. A skipped leg withholds the verdict
# rather than passing it.
#
# ⚠️ **Three conditions, not the two that paragraph names**, and `M4.55`
# corrected it: `have_tls` is required beside `have_rdkafka` and `have_java`
# below. It is not an FR-20 client — it is `M4.18`'s wiring leg, added by
# `M4.37` for the reason leg 4 gives at length — so a reader auditing this
# verdict against the sentence above finds a third conjunct the sentence does
# not explain and has to guess whether it is deliberate. It is.
have_rdkafka=0
have_java=0
have_tls=0
if [[ -f "$roster" ]]; then
  grep -qx 'librdkafka-groups' "$roster" && have_rdkafka=1
  grep -qx 'java-groups' "$roster" && have_java=1
  grep -qx 'tls-sasl' "$roster" && have_tls=1
fi
if (( _FAILURES == 0 )) && (( have_rdkafka )) && (( have_java )) && (( have_tls )); then
  ok "M4 completion condition holds (FR-40's five group APIs excepted and deferred, above)"
elif (( _FAILURES == 0 )); then
  (( have_rdkafka )) || skip "librdkafka never drove a group -- FR-20 is unproven here"
  (( have_java )) || skip "the Java client never drove a group -- FR-20 is unproven here"
  (( have_tls )) || skip "no client authenticated over TLS -- M4.18's own wiring is unproven here"
  warn "every check that could run passed, but the completion condition was NOT fully asserted"
fi
finish
