# `oqueue-coordinator` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-coordinator` for fmt, clippy and tests.

## Easy to get wrong here

1. **Monotonicity is the invariant everything rests on.** `oqueue_core::Offset::add` refuses to wrap for this reason.
2. ⚠️ **Metadata cost must be proportional to partitions active on this node**, never to cluster-wide partition count — doc 15 and doc 16. That rule is what rules out the obvious designs at the 1M-100M topic target.

## The ordering this crate exists to hold

`M3.7` filled in the skeleton `M0.8` created. Read
[`ADR-0020`](../../docs/internal/product/decisions/0020-offset-sequencing-and-coordinator-shape.md)
before changing anything in `coordinator.rs` or `allocator.rs`: **assign →
journal → ack** is the correctness argument, not an implementation order.

3. ⚠️ **The allocator takes a position only after the append resolves `Ok`.**
   `Allocator::stage` is fallible and mutates nothing; `Allocator::apply` is
   total. Reversing that leaves a gap in a line FR-11 requires to be gap-free
   the first time a journal refuses.
4. ⚠️ **The sequencer is a channel, not a lock.** `async-concurrency.md` rule 6
   forbids holding a lock across the append's `.await`, and releasing it in
   between would let two commits reach the log out of version order. Rule 8
   names this shape directly.
5. ⚠️ **`-1`, never `0`, when there is no offset** — `UNASSIGNED_OFFSET`. Doc 13
   §8: a plausible-looking offset on an error path turns an availability bug
   into a safety bug.
