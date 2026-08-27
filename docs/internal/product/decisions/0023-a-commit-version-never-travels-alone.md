# 0023. A commit version never travels without its epoch

Status: accepted; 2026-08-27 (`M3.30`): the epoch comparison is
**directional**, which this ADR's body states as an inequality. A watermark
from an *older* epoch is incomparable and the cache is fine; a watermark from a
*newer* one is evidence that this cache's own coordinator has been superseded,
and the remediation is the opposite — ⚠️ **discard the cache**, the same
answer `CacheFromAnotherEpoch` gets and for the same reason, rather than tell
the client its watermark cannot be compared. ⚠️ **This note said "go back to
the coordinator" until `M3.37`**, which is this enum's vocabulary for the
*round trip* a merely-behind cache gets — the opposite half of the rule the
code calls its organizing one. `RefreshReason` carries two
reasons for the two directions since that task.
Date: 2026-08-25
Requirements: FR-11, FR-12, FR-13, NFR-21

## Context

`ADR-0020` point 1 states a requirement and deliberately does not choose a
mechanism:

> ⚠️ **A `CommitVersion` must therefore never travel without its shard
> identity**, and a watermark held across a *rebalance* — the shard moving to
> another coordinator, which doc 15 §3 makes an ordinary operation — needs the
> receiving coordinator's version line to continue the old one rather than
> restart, or `AtLeast(v)` either stalls forever (new line below `v`) or
> returns stale data believing it is fresh (new line already past `v` for
> unrelated commits). This ADR states the requirement and does not choose the
> mechanism; the candidates are a shard-scoped epoch beside the version, or
> persisting the high-water version with the shard so it resumes rather than
> restarts. **`M3.10` owns picking one**, and cannot claim read-your-writes
> without it.

So there are two candidates, and the failure this must prevent is precise:
**a version compared against a line that is not the one that issued it.** Both
of its outcomes are bad and only one of them is loud. A new line *below* `v`
stalls a reader until its deadline — visible, annoying, safe. A new line
already *past* `v` for unrelated commits answers the reader immediately with
data that does not contain its write — invisible, and hazard H2 exactly.

Two further constraints narrow the choice:

- `CoordinatorEpoch` **already exists** in the vocabulary (`M3.3`, `M3.md`
  task 3) and is already required to be in every response, precisely so a
  reader can revalidate after a failover rewinds the log. A rebalance and a
  failover are the same event from a reader's side: the shard is now answered
  by a different incarnation.
- `MetadataShardId` does **not** exist and is `M7`'s. Today there is one
  implicit shard, so any mechanism keyed on a shard identifier would be
  unexercised and unenforceable until M7 — the shape `AGENTS.md` calls a
  preference rather than a rule.

## Decision

**A watermark is the pair `(CoordinatorEpoch, CommitVersion)`, never a bare
version, and a comparison across two epochs is not a comparison — it is a
forced round trip.** `SessionWatermark` in `oqueue-core` is that pair, and
`ReadMode::AtLeast` carries one.

Concretely:

1. A `CommitAck` already carries both. A session records the pair, and offers
   the pair back on its next read.
2. Cache admission compares the version **only** when the epochs are equal. A
   watermark from an older epoch is not "behind" and not "ahead"; it is
   **incomparable**, and the only sound answer is to go to the coordinator.
3. A coordinator receiving a rebalanced shard takes a **higher** epoch. It does
   not have to resume the previous version line, and deliberately is not
   required to: whatever line it starts, every watermark from before the move
   is incomparable and forces exactly one round trip per session, after which
   the session holds a watermark in the new epoch. The cost is bounded by the
   number of live sessions and paid once.

⚠️ **Point 3 is the part that makes this cheaper than it looks**, and it is why
this ADR does not also require the version line to be persisted with the shard.
The version line only has to be monotonic *within* an epoch, which one
allocator already guarantees.

### ⚠️ What this ADR does *not* discharge

`ADR-0020` point 1 asks for two things and this settles one of them. The other
is its first sentence — *"A `CommitVersion` must never travel without its shard
identity, and is never compared across shards"* — and `SessionWatermark` does
not carry a shard identity, because `MetadataShardId` does not exist and is
`M7`'s.

⚠️ **The residue is real, and it is not merely untidy.** `CoordinatorEpoch`'s
own definition is per *log*, and a log is per shard, so two shards' first
coordinators both sit at `CoordinatorEpoch::ZERO`. A session holding a
watermark from shard A and reading against shard B's cache would find the
epochs *equal* and fall straight through to a version comparison between two
independent counters — the exact failure this ADR's Context calls meaningless,
arriving through the mechanism built to prevent it.

Nothing can enforce it today: there is one implicit shard, so a test for it
would assert against a distinction the type system cannot express. It is
recorded in `roadmap.md`'s deferred-into-a-later-milestone table and in
`M7.md`'s plan, which is where an obligation with no row yet belongs, and `M7`
inherits it as a **safety** requirement rather than a tidying pass: the moment a
second shard exists, a watermark without one is admissible against the wrong
line.

## Alternatives considered

**Persist the high-water version with the shard, so the receiving coordinator
resumes the line rather than restarting it.** Rejected, though it is the other
candidate `ADR-0020` names and it is not unreasonable. Three reasons. It makes
correctness depend on a durable write completing during a rebalance — the one
moment the previous owner may be unreachable, which is exactly when the write
cannot be relied on, and a lost write silently produces the "new line already
past `v`" case that answers a reader with data missing its write. It needs
`MetadataShardId`, which is `M7`'s, so nothing could exercise it until then.
And it buys only the elimination of one round trip per live session per
rebalance, against a hazard whose failure mode is silent.

⚠️ **The two are not mutually exclusive** and `M7` may want both: resuming the
line makes the round trips rarer, while the epoch pair is what makes them
*correct*. This ADR chooses the one that is a safety property; the other is an
optimization and belongs where the shard identifier does.

**Compare versions across epochs and take the higher.** Rejected outright, and
recorded because it is the shape a well-meaning simplification would take. Two
epochs' version lines are independent counters; "higher" between them means
nothing, and the reading that feels safe — treating a *lower* new line as stale
and refreshing — is the safe half of a rule whose other half serves a reader
data that does not contain its own write.

**Make `CommitVersion` globally unique** (a UUID, or a hash), so a version from
another line simply never matches. Rejected on `ADR-0020` point 1, which is
explicit that staleness compares as `u64 <= u64` and nothing else: ordering
within a line is what the whole push/park mechanism rests on, and a value that
cannot be ordered cannot answer "is the index far enough along yet".

## Consequences

**Makes easy:** a rebalance and a failover need no distinguishing — both bump
the epoch, both make every outstanding watermark incomparable, and both are
answered by the same one round trip. `M6`'s failover inherits this rather than
designing its own.

**Makes hard:** every caller that holds a watermark has to hold both halves,
and an API that returns a bare `CommitVersion` for a caller to remember is a
trap. `CommitAck::version` is such an API; it is safe today because there is
one epoch and `M3.14` is the first caller to store a watermark across requests,
which is where the pair has to be carried rather than the number.

**Forecloses:** nothing that `M7` needs. Persisting the version line with the
shard remains available and remains an optimization on top of this, not an
alternative to it.
