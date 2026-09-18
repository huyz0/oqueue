# 0045. Object liveness is a reference count kept by the fold

Status: accepted
Date: 2026-09-19
Requirements: FR-35 (delete objects only after no reader can still reference
them), FR-33 (retention), NFR-51 (sans-I/O)

## Context

`M5.19` made trimming metadata-only: a trim drops index entries and writes and
deletes nothing. Something must then decide which *objects* behind those
entries may be deleted, and FR-35 is strict about it: only once no reader can
still reference them. After `M5.5` a bundled object carries many partitions —
`merge_round` writes one object spanning every partition in a round, and a
produce flush bundles every topic it batched (FR-32) — so an object is dead
only when **every** slice inside it is dead (`M5.20`'s row). Nine trimmed
partitions and one live one pin the object.

The question is where that count lives and how a deleter asks it.

## Decision

1. **The fold keeps a count of index entries naming each object.** Every
   entry a commit stages adds one for its object; a swap subtracts one per
   retired reference and adds one per installed; a trim subtracts one per
   entry it drops. Maintained per changed entry, never by walking the index.
   An object whose count is zero is referenced by nothing the index can
   serve.
2. ⚠️ **A publication does not subtract, and that is the safe direction on
   purpose.** A manifest absorbs history entries into one object (`ADR-0042`),
   and the data objects it absorbed are still referenced — by the manifest's
   own contents, which this fold does not hold and must not (holding them is
   the unbounded state `ADR-0042` exists to shed). So an absorbed object's
   count stays where it was, forever: it is never released by this fold. That
   leaks storage and never deletes a byte a reader could reach, which is the
   direction FR-35 permits. Releasing manifest-covered objects needs the
   manifest read back and is its own row.
3. **`MaterializedIndex` gains `references(object) -> usize`**, answered from
   memory. A deleter asks it of the objects a trim's partitions named,
   gathered before the trim, and deletes only at zero — after the lifecycle's
   delay, which is `M5.21` and `M5.22`'s.

## Alternatives considered

- **Count on demand, by walking every partition.** Rejected: it is
  O(entries) per question, and `ADR-0043` prices the index at millions of
  entries per busy partition. A lifecycle asking it of every candidate every
  round would be the index walk ADR-0036 decision 1 exists to avoid.
- **Keep, per manifest, the list of objects it absorbed, so a publication
  could subtract and a later trim of the manifest could release them.**
  Rejected for now: that list is the history the manifest replaced, held
  again, which is exactly `ADR-0042`'s unbounded state returning by another
  name. Reading the manifest back when it is dropped is the bounded way, and
  it needs object storage, so it is an executor's job and not this fold's.
- **Emit a "released" event from `apply` for the deleter to drain.**
  Deferred: a drain is a mutation on a read seam, and `ADR-0024` keeps the
  coordinator as the index's only writer. A read-only query the deleter
  checks at deletion time is also the stronger guarantee, because it is
  asked at the moment it matters rather than at the moment the count changed.

## Consequences

- An object pinned by one live slice is visible as such (`references > 0`)
  and is never deleted; repacking it so it can die is a compaction concern
  split to its own row.
- Objects absorbed into a partition manifest are never released by this
  count. Storage cost grows with every publication until the manifest-reading
  release exists; no data is at risk.
- Every `MaterializedIndex` implementor gains one method; the count is
  rebuilt from the log on replay exactly as the rest of the fold is.
