# 0044. The index seam answers retention's two questions

Status: accepted
Date: 2026-09-19
Requirements: FR-33 (retention, including on idle partitions), FR-35 (delete
only after no reader can reference), NFR-51 (sans-I/O)

## Context

`M5.86` gave the fold a per-partition commit-time extent and `M5.19` a log
start. Both live on `IndexState`, the concrete fold, as inherent methods. The
retention round `M5.18` builds cannot reach them there: it runs inside the
coordinator, which takes the index by `Box<dyn MaterializedIndex>` — so the
caller keeps no handle, the property `ADR-0024` made the coordinator's sole
write path rest on — and holds it only behind that trait from then on; and in
`oqueue-compact`,
which compaction's sweep already reads exclusively through that trait
(`ADR-0036` decision 1). A round asks two questions of every partition a node
holds: *how old is its newest data*, and *where does its readable log begin*.
Neither is answerable through the seam today.

⚠️ **`M5.19` already paid once for the second one's absence.** Compaction
needed the log start and could not ask for it, so it recovers the start from
the `BelowLogStart` refusal a read below it returns — lazily, so an untrimmed
walk costs no extra query. That is a workaround sized to one caller that only
needs the start after it has already been refused. Retention needs it *before*
deciding anything, for every partition, with no read to be refused.

## Decision

`MaterializedIndex` gains two read-only methods, each answered from memory with
no object-storage operation:

- `time_span(topic, partition) -> Option<TimeSpan>` — the oldest and newest
  commit time folded for the partition. `None` means nothing was committed to
  it, which is a different claim from "committed at the epoch" and must stay
  one: a round that read `EPOCH` there would reap on its first sweep.
- `log_start(topic, partition) -> Offset` — the first readable offset, zero
  until a trim moves it.

Both are infallible, matching `end_offset` beside them: they read state the
fold already holds, so there is no failure the implementation does not control
(`contracts.md` rule 6 asks for `Result` only where there is).

⚠️ **Neither adds a fetch-path cost.** They are read by a retention round on
its cadence, not by `Fetch`; `find_batches` is unchanged.

## Alternatives considered

- **Recover both from refusals, as `M5.19` did for the start.** Rejected:
  there is no refusal carrying a commit time, and a round that probed every
  partition with a read in order to be told where it starts is a read per
  partition per round against an index whose whole point, for compaction's
  trigger and now retention's, is that the decision costs one lookup.
- **Hand the retention round the concrete `IndexState`.** Rejected:
  `ADR-0024` puts the coordinator's index behind the trait precisely so the
  coordinator is its sole writer, and `oqueue-index`'s `MemoryIndex` is the
  production implementation, not `IndexState` directly. A round that
  downcast would work against one implementation and silently not against
  the other.
- **A separate `RetentionView` trait.** Rejected: its implementor would be
  every `MaterializedIndex` implementor, holding the same state behind the
  same lock — two seams over one object, which is `contracts.md`'s definition
  of a fork waiting to drift.
- **Return a whole per-partition summary struct.** Deferred rather than
  rejected: two scalars do not justify a type, and a third question (bytes
  held, for size-based retention `M5.17`) is not answerable from the index at
  all today — the history tier carries no byte range — so a struct designed
  now would be designed around a field that does not exist.

## Consequences

- Every `MaterializedIndex` implementor gains two methods: the fake and
  `MemoryIndex` delegate to `IndexState`; the test wrappers in
  `oqueue-compact` and `oqueue-broker` delegate to what they wrap.
- `M5.19`'s refusal-recovery in compaction stays. It is not made redundant —
  compaction's walks start from a caller's cursor and only need the start once
  refused, and the lazy retry is what keeps an untrimmed walk at its measured
  cost — but a future caller that needs the start up front uses the seam.
- The `EPOCH` question is now a seam contract: an implementor that returned
  `Some` at the epoch for an uncommitted partition would reap it.
