# 0004. The `Clock` seam

Status: accepted
Date: 2026-08-16
Requirements: NFR-51

## Context

Several behaviours this broker needs are time-dependent: retention, session
timeouts, DEK cache expiry, the batching window that decides when a produce
buffer is flushed. Each of them is testable only if time is something a test can
control, and untestable if any of them calls `SystemTime::now` directly.

`contracts.md` rule 15 makes a wholly new `pub trait` in `oqueue-core` a
decision, and `check-core-contract.sh` enforces it: a new trait's old signature
set is `None`, which counts as a changed method set, so the commit is refused
without an ADR staged beside it.

## Decision

One method:

```rust
pub trait Clock: Send + Sync + fmt::Debug {
    fn now(&self) -> Timestamp;
}
```

`Timestamp` is milliseconds since the Unix epoch, non-negative, backed by `i64`
because that is the protocol's type for a record timestamp.

⚠️ **Not async**, unlike `ObjectStore` and `KeyProvider`. Reading a clock does no
I/O, so ADR-0002's boxed-future shape would cost an allocation per call for
nothing.

### What an implementor must guarantee

1. **`now` is infallible.** No `Result`. A clock that can fail forces every
   caller to decide what to do about it, and there is no useful answer.
2. **`now` is cheap** — a syscall at worst. It will be called on the produce
   path.
3. ⚠️ **`now` is non-decreasing**: two calls in program order never return a
   smaller second value.
4. ⚠️ **It is not strictly increasing.** Two calls may return the same
   `Timestamp`, and a caller needing distinct values must not get them here —
   that is what `Offset` is for. Stating this is the point of the ADR: a caller
   that assumes uniqueness gets a bug that appears only under a fast clock or a
   coarse one.

5. ⚠️ **Non-decreasing is a property of the clock, not of a thread.** Two calls
   from *different* threads are still ordered: if A returns `t` and B later
   observes any value, that value is `>= t`. This is the guarantee that costs
   something to implement, and review caught `FakeClock` violating it — a
   load-check-store `advance` on a shared `Arc` lost updates and let an
   observer see time run backwards. It is now one atomic read-modify-write, and
   a four-thread test asserts both halves.
6. ⚠️ **A pre-epoch host clock is the implementor's problem, not the caller's.**
   `Timestamp` cannot represent a time before 1970 and `now` is infallible, so
   an implementation over `SystemTime` — where `duration_since(UNIX_EPOCH)`
   returns an error, and `unwrap` is denied outside tests — must clamp to
   [`Timestamp::EPOCH`] rather than panic. Stated here because `M1` would
   otherwise have to invent an answer under deadline.

⚠️ Guarantee 3 is a real constraint on a real implementation, not a formality:
`SystemTime::now` **can** go backwards across an NTP step. An implementor
backed by it must clamp, and that is a thing to remember when `M1` writes one.

## Alternatives considered

**`std::time::Instant`, or `SystemTime`, as the returned type.** Rejected.
`Instant` is opaque and can only be produced by `Instant::now()`, so a fake
cannot construct one at a chosen point — the type defeats the seam. `SystemTime`
is a concrete `std` clock type crossing a seam whose whole purpose is that no
concrete clock appears in `oqueue-core`, and `contracts.md` rule 8 says a
trait's types are types this workspace owns.

**A monotonic method alongside the wall-clock one** — `fn elapsed_since(..)` or
a second `Instant`-like type for timeouts. Deferred rather than rejected: it is
a real need, and adding it now means guessing its shape before any caller
exists. `contracts.md` rule 12 makes adding a method a contract change with its
own commit, which is the right cost to pay when `M1` knows what it wants.

**Putting the fake in `oqueue-testkit`.** Rejected, and `milestones/M0.md`'s
completion condition proposed it. `contracts.md` rule 9 and `testing.md` rule 4
put a fake beside its trait so a downstream crate can test against the seam
without depending on the testkit; rule 11 calls a fake found in the testkit a
layering violation. The plan's own Goal section agrees with the standards.

**No seam — call `SystemTime::now` and test with generous tolerances.**
Rejected. It is what `check-sans-io.sh` exists to refuse, and the tolerances are
where flaky tests come from: a threshold wide enough never to fail on a loaded
CI box is wide enough to hide the bug it was written for.

## Consequences

**Easy.** Retention, timeouts and cache expiry become ordinary unit tests with
no sleeping and no tolerance. `FakeClock::advance` is the only way time moves,
so a test states its schedule explicitly.

**Hard.** Every type that needs the time now takes a `&impl Clock` or holds one,
which is visible in a lot of signatures. That is the cost of the seam and it is
paid deliberately.

⚠️ **`FakeClock` cannot be driven backwards** — `advance` refuses a negative
delta, refuses to wrap, and is a single atomic read-modify-write so concurrent
advances neither lose updates nor let an observer see time regress. A fake that could would let a test construct a state
no real implementor may produce, which is the failure a fake exists to prevent
rather than to enable. It is `testing.md`'s "a fake's job is fidelity to the
documented contract" applied to the one guarantee this contract has.

**What has no gate.** That an implementor actually *is* non-decreasing.
`check-sans-io.sh` stops other crates reading a real clock; nothing checks that
`M1`'s real `Clock` clamps an NTP step. That is review's, and it is written into
the trait's own documentation so a reviewer has something to check against.
