# 0038. Producer identity survives a rewrite in the log, not in the object

Status: accepted
Date: 2026-09-18
Requirements: FR-14 (idempotent producer), FR-34 (compaction)

## Context

`M5`'s checkpoint review filed `M5.49` as a major: `merge`'s `take_regions`
writes `PushedRecords { producer: None }` for every region it rewrites, the
bundle footer carries no producer identity by design (`M11.5` keeps producers
parallel to `Region`, out of the footer), and so an object's identity
information is structurally unrecoverable once it has been merged. The finding
concluded that FR-14 — which is `done` — stops holding the moment compaction
commits.

⚠️ **Amended by `M6.3` (2026-09-19): the replay now exists.** `Coordinator::open` over a non-empty log folds it from the beginning into the allocator, producer state included, and `a_second_coordinator_rebuilds_producer_sequence_history` replaces the test that pinned the refusal described below. Obligation 2 — a snapshot that carries producer state — is `M6.4`'s. The rest of this section is the state this ADR was written in.

⚠️ **The mechanism the finding named does not exist, and that is this ADR's
first job.** It said "`M11.3`'s allocator rebuilds `producer_state` by folding
the metadata log's producer fields". Nothing rebuilds it. `Coordinator::open`
over a log that already holds entries returns
`CoordinatorError::ReplayRequired { last_version }` and refuses to start —
`a_log_carrying_producer_sequence_history_refuses_a_second_coordinator_too`
pins exactly that — so today a coordinator's `producer_state` is built only by
the commits that coordinator itself served, in memory, capped at
`MAX_TRACKED_PRODUCERS` and evicted by recency (`ADR-0031` point 6).

So the question is not whether compaction erases something a replay reads. It
is where identity has to live **when the replay path that `ReplayRequired`
names is built**, and what compaction owes it in the meantime.

Three places identity could survive a rewrite:

1. **The object's footer.** Rejected by `M11.5` already, and rejected again
   here: the footer is read on the fetch path and a producer identity per
   region is bytes every consumer pays for to serve a check only the commit
   path makes.
2. **The index.** A `CommittedSpan` carries `Option<ProducerIdentity>`, so the
   materialized index sees it — but the index is a *fold*, and what a fold
   holds is the current state. An entry retired by compaction is gone from it.
3. **The metadata log.** Append-only, read from `CommitVersion::ZERO` by
   anything that rebuilds, and already carrying the identity on every
   `BatchCommitted` span that had one.

## Decision

**Producer identity survives a rewrite in the metadata log.** Neither the
merged object nor the index is required to carry the identity of the spans it
replaces, and `take_regions`' `producer: None` is correct rather than a defect.

Two obligations follow, and they are the substance of this decision:

1. ⚠️ **Compaction's commit appends; it never rewrites or removes a
   `BatchCommitted` event.** `M5.13` swaps input refs for an output ref as a
   new event, leaving every original event in the log. This is what
   `MetadataRecord`'s own doc means by "a log of deltas can be snapshotted at
   any point": the swap is another delta, and the identities are still in the
   events below it.
2. ⚠️ **Anything that folds the log for producer state folds it from the
   beginning, or from a snapshot that carries producer state forward.** `M6`
   is where the log gets snapshotted, and a snapshot that materializes offsets
   without producer state would drop FR-14's window at exactly the point this
   ADR says it is kept. `M6`'s snapshot format must carry it.

⚠️ **FR-14's window is bounded whatever this decides**, and that is stated here
so the guarantee is not read as larger than it is: `MAX_TRACKED_PRODUCERS` is
100,000 lines per allocator with recency eviction, so a producer quiet long
enough is forgotten and its retry is admitted as new. That is `ADR-0031` point
6's deliberate bound, and compaction neither widens nor narrows it.

## Alternatives considered

- **Carry identity through the merge.** `merge` would need each input span's
  identity, which means the plan's inputs carry it, which means `ObjectRef`
  grows an `Option<ProducerIdentity>` — 16 bytes against the ~40-byte inline
  budget doc 14 §3's arithmetic rests on, paid by every entry in the index
  whether or not its producer was idempotent, to serve a check that reads the
  log anyway. Rejected on cost, and it would not help: the merged span holds
  records from several input spans with several identities, and a
  `CommittedSpan` carries one.
- **Declare compaction and idempotence exclusive for a range**, which `M5.49`'s
  row offered as the second horn. Rejected: it makes FR-14 conditional on an
  operational decision no client can see, and a client cannot know whether the
  range it is retrying into has been compacted.
- **Rebuild producer state from the objects.** Rejected with alternative 1 —
  the footer has no identity, by `M11.5`.

## Consequences

- `M5.13` carries obligation 1 as an acceptance criterion rather than as
  advice, written into its backlog row in the same commit as this ADR. ⚠️ **It
  was not, in this ADR's first draft, and review caught the asymmetry**:
  obligation 2 was recorded in `roadmap.md`'s deferral table and obligation 1
  was recorded only here, where the author of `M5.13` — who builds from the row
  — would never read it.
- `M6` inherits obligation 2. Recorded in `roadmap.md`'s deferral table, since
  `M6` has no decomposition yet.
- `M5.49`'s row is corrected: the premise it was filed on describes a rebuild
  that does not exist.
- Forecloses: nothing. If a later milestone wants identity in the object after
  all, the footer's format is versioned (`M3.13`, doc 10 #40) and this decision
  is one a new ADR supersedes.
