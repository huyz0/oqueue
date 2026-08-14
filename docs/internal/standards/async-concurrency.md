---
title: "Async and concurrency"
description: >
  Read when writing anything that spawns a task, holds a lock, awaits, or shares state across connections — the coordinator, the broker's I/O shell, and anywhere producers or consumers run concurrently.
tags: [code, async, concurrency, tokio, locking, cancellation]
applies_to: ["*oqueue-broker/*", "*oqueue-coordinator/*", "*oqueue-store/*", "*oqueue-compact/*", "*oqueue-crypto/*", "*/bin/oqueue/*"]
---

# Async and concurrency

Rules for `tokio`, tasks, locks, and cancellation. This is deliberately the
one place `unsafe` is never allowed — see rule 1 — because it is where Miri
is blindest and where a data race is hardest to reproduce. Evidence in
[docs/researches/19](../../researches/19-workspace-engineering.md) §7.2, §8.

## `unsafe` is banned here, without exception

1. **No `unsafe` in the async or concurrency layer, ever.** `AGENTS.md`
   non-negotiable 7 names three crates where `unsafe` is allowed —
   `oqueue-buf`, `oqueue-codec`, `oqueue-checksum` — and every one of them
   is a leaf primitive with no task spawning and no shared mutable state.
   The concurrency layer is exactly where a lock-free trick looks like a
   performance win and is the hardest place in the codebase to prove
   correct without a human reading it. → `check-unsafe.sh` (M-1.11)
2. **A lock-free data structure is not proof against rule 1 — it moves the
   need for `unsafe` into `oqueue-buf` or `oqueue-core`, sized small enough
   for `loom` to explore exhaustively**, and the concurrency layer above it
   uses the safe result. See rule 12 and
   [testing.md](testing.md) rule 22.

## Blocking and the runtime

3. **No blocking call on an async task.** A synchronous filesystem call, a
   CPU-bound loop over more than a few thousand elements, or a call into a
   C library without an async wrapper starves the runtime's worker thread
   for every other task scheduled on it — including, on the same node,
   other tenants' connections. Use `tokio::task::spawn_blocking`, or move
   the work to a dedicated thread pool if it recurs on a hot path.
4. **Every I/O-bound `.await` that crosses a process or network boundary has
   a timeout.** An `ObjectStore` call, a downstream KMS call, a peer
   connection read — none of them may await indefinitely on a resource this
   process does not control. Compose the timeout with the retry policy
   named in [error-handling.md](error-handling.md) rule 7; a timeout is not
   itself a retry decision.
5. **`spawn_blocking`'s thread pool is not free capacity.** It has a bound;
   a burst of blocking work from one tenant queuing behind another's is a
   cross-tenant resource-exhaustion path, and [security.md](security.md)
   rule 13's "a tenant cannot exhaust a shared resource" applies to it
   exactly as it applies to connection counts.

## Locking

6. **Never hold a lock across an `.await`.** Every other task waiting on
   that lock is blocked for the duration of whatever the held task is
   awaiting, which turns a fast critical section into an unbounded one.
   Where the two seem unavoidable, restructure: clone or take ownership of
   what's needed under the lock, release it, then await.
7. **Prefer message-passing over shared mutable state** where the two are
   equally natural — an `mpsc` channel into a task that owns the state, over
   a `Mutex<T>` several tasks reach into. A single owner makes the
   invariants a single reader can hold in their head; a shared lock makes
   them a property of every call site that touches it, forever.
8. **When a `Mutex` is the right tool, it protects data, not control flow.**
   A lock taken to serialize "do step A then step B" across tasks is a
   sequencing problem wearing a locking primitive; use a channel or an
   explicit state machine instead.
9. **No lock is acquired in an order that isn't the same everywhere it is
   acquired with another lock.** Two locks taken in opposite orders on two
   code paths is a deadlock waiting on the right interleaving — document the
   fixed order beside the types, not in a comment nobody will find when
   adding the third lock.

## Cancellation

10. **Every future is cancellation-safe at every `.await` point it exposes
    across a public API**, or it says so loudly in its doc comment.
    `tokio::select!` and a client disconnecting mid-request both drop
    futures without running them to completion; a future that mutates state
    before its final await and only commits after it is a partial-write bug
    the moment something cancels it.
11. **A cancelled produce or fetch must not leave the system unable to say
    whether the write happened.** This is the same shape as the "written
    but not yet acknowledged" ambiguity the wire protocol already has to
    handle for a dropped connection — cancellation from `select!` or a
    timeout is not a new class of ambiguity, it is the same one arriving
    from inside the process instead of from the network. Don't special-case
    it; route it through the same idempotence and reconciliation path.
12. **`loom` covers atomic ordering; it does not cover cancellation.**
    [Doc 19](../../researches/19-workspace-engineering.md) §7.2 is explicit that ARM CI is
    not the atomics gate and `loom` is — the same document has no
    equivalent tool for cancellation safety, which is why rule 10's doc
    comment and rule 11's routing are review-enforced, not gate-enforced.

## Task ownership and shutdown

13. **A spawned task has an owner that can observe its completion and its
    panic.** `tokio::spawn` detaches by default; a task whose `JoinHandle`
    is dropped and whose panic is therefore silently swallowed is a failure
    mode nobody will see until the work it was supposed to do never
    happened. Use a `JoinSet` (or an owned `JoinHandle` that is awaited or
    explicitly aborted) for anything spawned inside a request path.
14. **Every long-lived task listens for a shutdown signal and exits on it**,
    rather than being killed by the process exiting out from under it. A
    task mid-write to object storage when the process dies is a normal
    failure this system already has to tolerate (see recovery); a task
    that never gets the chance to finish an in-flight unit of work it
    *could* have finished cleanly is an avoidable one.
15. **A panic in one connection's task must not take down another
    connection's.** Per-connection work runs in its own task specifically
    so `catch_unwind` at that one boundary — see
    [error-handling.md](error-handling.md) rule 2 — contains the blast
    radius to the tenant who triggered it.

## Environment and process-global state

16. **No `std::env::set_var` outside test setup**, and even there, only
    where `cargo-nextest`'s process-per-test isolation makes it safe.
    Environment variables are process-global; edition 2024 made setting one
    `unsafe` for exactly this reason. See [testing.md](testing.md) rule 12.

## What has no gate

**Whether a future is actually cancellation-safe.** Rule 10 asks for a
doc-comment claim; nothing currently verifies the claim against the code
except a reviewer tracing every `.await` point by hand.

**Whether a lock ordering is consistent project-wide** as the codebase
grows past what one reviewer can hold in their head. `loom` bounds this for
structures small enough to model exhaustively; there is no equivalent tool
yet named for whole-system lock ordering.

## See also

- `loom`, TSan, and the concurrency test tiers: [testing.md](testing.md) rules 22, 25
- Why `unsafe` is banned here specifically: [docs/researches/18](../../researches/18-rust-performance-methodology.md) §5.7,
  [docs/researches/19](../../researches/19-workspace-engineering.md) §7.2
- Deterministic simulation for cluster-level concurrency bugs (node failure,
  reordering, partitions): [docs/researches/19](../../researches/19-workspace-engineering.md) §8.3,
  [testing.md](testing.md) rule 26
