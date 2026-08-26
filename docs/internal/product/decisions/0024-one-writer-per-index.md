# 0024. One writer per materialized index, and the seam that enforces it

Status: accepted; 2026-08-26 (`M3.11`): ⚠️ **every mention of "`M3.11`'s
quota" below should now be read as `M5`'s.** The body is left as accepted —
the `adr` skill records divergence on this line and never by rewriting what
was believed at decision time — and what changed is only where the quota
lands: a ceiling at M3's index keying gives back range a rebuild cannot
restore, so `roadmap.md`'s deferral table carries the enforcement to `M5`
beside the coarse per-object re-keying, and `M5.md` task 8a receives it.
Nothing about this ADR's own decision moves: the door a quota asks through is
still `Coordinator::drop_cache`, and still queued.
Date: 2026-08-25
Requirements: FR-12, FR-13, NFR-2, NFR-11

## Context

`M3.9` decided that a coordinator is the sole writer of the index it was opened
with, and the only party that may clear it. That decision was recorded **in a
rustdoc**, restated in four more places, and enforced by nothing. M3's
checkpoint review called it out: it constrains `M3.11`'s degraded mode,
`M3.14`'s fetch path, M6's failover and M7's rebalance, which is the `adr`
skill's own trigger, and four copies of an unenforced rule is a rule that
drifts.

The decision itself is not in doubt and this ADR does not reopen it. What is
at stake is why it must be *structural*.

**Two writers is not a race that loses a write; it is a race that produces
wrong offsets.** `MaterializedIndex::apply` checks version *order* — strictly
increasing, refusing anything at or below `applied_upto` — and does **not**
check contiguity, because contiguity is not knowable from a batch. So:

- If a second writer folds first, the coordinator finds the index ahead of it,
  takes its rebuild branch, and pays a **full log replay inside the ack path**,
  on the one task every producer on the shard queues behind.
- If a `clear` lands between the coordinator's own "is this current" check and
  its fold, the fold is *accepted* — an empty index has no `applied_upto` to
  refuse against — and `IndexState` bases every partition it has forgotten at
  `Offset::ZERO`, while the log, the allocator and the ack already given to the
  producer place those records far higher. That is an index that is **wrong**
  rather than empty, and nothing detects it.

⚠️ The second is the one that matters, and it is reachable from the seam's own
documented behaviour: `MaterializedIndex` invites any holder to drop the cache,
because for its *writer* that genuinely is always safe.

The obstacle is that `Coordinator::open` took `Arc<dyn MaterializedIndex>`. A
caller must therefore construct the `Arc`, and an `Arc` it constructed is one it
can keep — carrying `apply` and `clear`. `M3.14`'s broker has to hold *something*
to serve fetches from, so "just don't keep it" was a rule the API asked to be
broken.

## Decision

**A coordinator takes ownership of its index, and hands back a read-only view.**

```rust
pub async fn open(
    log: Arc<dyn MetadataLog>,
    index: Box<dyn MaterializedIndex>,      // moved in, not shared in
    epoch: CoordinatorEpoch,
) -> Result<(Coordinator, CoordinatorLoop, IndexReader), CoordinatorError>
```

1. The index arrives as a `Box`, so the caller keeps nothing. `open` converts
   it to an `Arc` internally; the loop holds one clone and the returned
   `IndexReader` holds the other, and neither is reachable as
   `Arc<dyn MaterializedIndex>` from outside. ⚠️ **"Cannot" is too strong and
   the honest claim is "not by accident"**: a caller may implement
   `MaterializedIndex` on a newtype delegating to an `Arc` it retains. What
   changes is that the writable handle stops being what the API hands you and
   becomes a wrapper somebody had to mean to write.
2. `IndexReader` — in `oqueue-core`, not in `oqueue-coordinator`, because a
   coordinator is not the only sole-writer of an index — exposes
   `applied_upto`, `end_offset` and `find_batches` and nothing else. It is what
   a fetch path holds: everything a read needs, with `apply` and `clear` simply
   absent. ⚠️ **`oqueue-index`'s `LogApplier` hands out the same type**, and
   that is the half this ADR would otherwise have left open: it is the sole
   writer of a *different* index, and `index()` returning an `Arc` would have
   made every one of its callers a candidate second writer.

   ⚠️ **The two halves are not equally enforced, and the asymmetry is
   deliberate.** `LogApplier::new` still takes an `Arc`, so a caller that
   constructs one *can* keep it. What differs is need: a coordinator's caller
   **had** to keep a handle, because it has to serve fetches from that index —
   the API asked to be broken, which is why it changed. An applier's caller has
   no such need now that `index()` answers reads, so retaining the `Arc` is a
   deliberate act rather than the path of least resistance. The applier also
   cannot take a `Box` without losing the one thing it must express: an index
   that **outlives** the applier folding into it, which is what a warm restart
   is and what `a_restart_replays_the_delta_rather_than_the_log` exercises.
3. Dropping the cache stays available and goes **through the writer**:
   `Coordinator::drop_cache` for the coordinator's index, `LogApplier::drop_cache`
   for the applier's. The coordinator's is *queued*, which is the part that
   matters — a clear reaching the index outside that queue can land between the
   loop's own "is this current" check and its fold, where it is accepted.
   ⚠️ **The cost lands on the next commit**, and a caller should know where:
   that commit takes the rebuild branch and pays a full log replay inside the
   ack path, `REBUILD_PAGE_ENTRIES` at a time, with every queued producer
   behind it. `M3.18` owns not paying for that on the ack path; `M6.md` task 15
   (chunked replay, bounded transaction size) is the mechanism.

⚠️ **This is not a change to `MaterializedIndex`.** The trait's method set is
untouched, so `check-core-contract.sh` does not fire and no fake or
implementation moves. The rule is enforced by who can *reach* the trait, which
is a smaller blast radius than narrowing the trait would have been — and it
leaves the trait usable in full by `oqueue-index`'s `LogApplier`, which is the
sole writer of a *different* index and needs `apply`.

## Alternatives considered

**Split `MaterializedIndex` into a read trait and a write trait.** Rejected as
the larger change for the same benefit: it touches the trait, every fake, every
implementation and the conformance suite, and it puts the distinction in
`oqueue-core` where the constraint is not — an index outside a coordinator
(`LogApplier`'s) legitimately needs both halves through one handle. The
constraint is about ownership, so it belongs at the ownership boundary.

**Keep `Arc` and document harder.** Rejected: that is the state M3's checkpoint
review found, and the count was already four copies. A rule restated is a rule
whose copies drift; `AGENTS.md` says a rule whose script is missing is a
preference, and the same reasoning applies to a rule whose type system is
silent.

**Have `open` construct the index itself**, so no handle exists to pass.
Rejected: it forecloses doc 10 #12's engine being chosen at startup, which is
the whole reason the read side is behind a trait (`ADR-0022` decision 1). A
`Box` gives the same enforcement while keeping the choice the caller's.

**A runtime guard — a flag or a generation counter the coordinator checks.**
Rejected on the same grounds `ADR-0020` reserves conditional writes for the
control plane: a check on the ack path costs something on every commit, and it
turns a bug that cannot be written into a bug that is reported at run time. A
compile error is strictly better than an alert.

## Consequences

**Makes easy:** `M3.14`'s fetch path holds exactly what it needs and cannot
reach anything else, so the rule needs no discipline from it. `M3.11`'s degraded
mode has one door, and it is the serialized one.

**Makes hard:** anything wanting a second materialization of the same log must
build its own — which is doc 12 §4.4's model, where every agent keeps its own
cache, and `LogApplier` plus `Coordinator::subscribe` is how. That is the
intended shape rather than a cost, but it is a real constraint on anyone who
expected to share.

**Forecloses:** a caller injecting an index it also mutates *into a
coordinator*, including for tests. A test wanting to observe that index
observes it through the returned `IndexReader`, and one wanting to corrupt it
can no longer do so from outside — the drop-and-rebuild path is exercised
through `Coordinator::drop_cache`, which is how production reaches it too.
⚠️ **It forecloses nothing on the applier side**, where the constructor still
takes an `Arc`; see decision point 2 for why the need differs and what remains
a matter of discipline there.
