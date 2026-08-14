---
title: "Error handling"
description: >
  Read when defining an error type, deciding whether something is a `Result` or a panic, propagating a failure across a crate boundary, or writing an error message a client or operator will see.
tags: [code, errors, panics, results, retries]
applies_to: ["*.rs"]
---

# Error handling

How a failure is represented, classified, and carried across a boundary. This
file does not restate the bans already gated elsewhere — `no unwrap/expect
outside tests` is [code-structure.md](code-structure.md) rule 26, `no panic
reachable from client bytes` is [security.md](security.md) rule 3, `no
unchecked arithmetic` is [security.md](security.md) rule 4. What follows is
everything between "not a panic" and "a good error."

## `Result`, not a panic

1. **A panic means the programmer got an invariant wrong; a `Result` means
   the world did.** Anything caused by a peer, an object store, a config
   file, or another tenant is a `Result`, full stop, no matter how unlikely.
   The only thing allowed to panic in production code is an invariant this
   crate itself is responsible for holding, and encoding it in a type beats
   asserting it at runtime every time that's possible.
2. **A caught panic is not a substitute for a `Result`.** `catch_unwind` at
   the edge of the process (per-connection task boundary, so one client's
   bug does not take down another's connection) is acceptable as a last
   line of defense; using it as a control-flow mechanism anywhere else means
   the function should have returned `Result` in the first place.

## Types, and where they live

3. **Each crate defines its own error enum with `thiserror`.** The enum is
   part of that crate's public contract — see
   [contracts.md](contracts.md) rule 17 — and a caller matches on variants,
   not on a formatted string.
4. **Only `bin/oqueue` uses `anyhow`.** [Doc 19](../../researches/19-workspace-engineering.md) §3.3:
   `anyhow::Error` erases the type the caller needs to match on, so it is
   correct exactly once — at the top, where nothing downstream needs to
   distinguish a cause anymore.
5. **A crate's error enum classifies, it doesn't just wrap.** `#[from]` on a
   dependency's error type is a starting point, not the whole enum — collapse
   what the caller cannot act on differently, and give a distinct variant to
   everything it can. A caller that has to `match` on a wrapped third-party
   type to decide whether to retry has an error type that failed at its one
   job.
6. **A variant that can never happen from this function's inputs is not
   added "for completeness."** An error type is a claim about what can go
   wrong; a variant nothing can produce is a claim that is false the day it
   is written.

## The retryable/permanent split

7. **Every error a client or an internal caller might see answers one
   question mechanically: is retrying this ever worth it?** `ObjectStore`'s
   planned taxonomy — `NotFound`, `PreconditionFailed`, `SlowDown`,
   `Throttled`, `Transient`, `Permanent` (see
   [milestones/M1.md](../product/milestones/M1.md)) — is the general shape
   every seam's errors should follow, not a one-off for that trait: retry
   forever with backoff (`SlowDown`, `Throttled`), retry a bounded number of
   times (`Transient`), or never retry (`Permanent`, `PreconditionFailed`).
8. ⚠️ **Classification belongs to the seam that produces the error, never to
   the caller guessing from a message string.** A caller that pattern-matches
   `error.to_string().contains("throttl")` to decide whether to back off is
   the failure mode this rule exists to prevent — the classification is an
   enum variant or it does not exist.
9. **A precondition failure (CAS lost) is never retried automatically at the
   point it is detected.** Retrying a lost compare-and-swap silently
   converts it into last-writer-wins, which is exactly the invariant the
   conditional write existed to protect. See
   [milestones/M1.md](../product/milestones/M1.md) task 11. Whether to retry
   at a *higher* level, with a fresh read, is a caller decision made with
   full knowledge of what was lost — never an automatic retry of the same
   write.

## Context and propagation

10. **An error gains context as it crosses a boundary it did not originate
    at**, via `#[from]` plus a wrapping variant or an explicit `.map_err`,
    not by re-throwing the dependency's type unchanged. A `NotFound` from
    `oqueue-store` crossing into `oqueue-coordinator` should read as "the
    manifest object for partition P was not found," not as the bare
    store-level variant with no indication of what was being looked up.
11. **Never log and return the same error.** Logging at the point of
    failure and returning it up the stack means whatever calls this
    function will very likely log it again, and the operator reading the
    log sees the same failure reported twice at two call depths with no way
    to tell whether it is one incident or two. Log at the boundary that
    decides *not* to propagate further — the point where the error stops
    being returned and starts being handled.
12. **An error that reaches a client is not the internal `Debug`
    representation.** A wire-level error response is a stable, documented
    shape (a Kafka error code, or this project's own admin-API error body);
    the internal enum's `Display`/`Debug` output is for logs and is allowed
    to change without that being a compatibility break.

## What an error message says

13. **An error message states what failed and, where known, why** — not
    just "operation failed." "PUT to `s3://bucket/key` failed: precondition
    `If-None-Match: *` did not hold" is checkable by whoever reads it; "put
    failed" is not.
14. ⚠️ **Never let an error message leak more than the reader needs.** A
    message useful to an operator debugging a stalled partition can be
    exactly the message that hands an attacker a tenant ID or a key
    identifier — this is the trade [security.md](security.md)'s "what has
    no gate" section names as unmechanizable. Default to less: name the
    resource by an opaque ID the operator can look up, not by anything that
    doubles as a capability.
15. **No secret, key material, or unredacted customer payload ever appears
    in an error's `Display` output**, including inside a formatted byte
    range or a debug dump of a failed frame. → [security.md](security.md)
    rules 6–7.

## What has no gate

**Whether an error variant is the right granularity.** A script can check
that variants exist and that `unwrap` doesn't; it cannot tell you that three
variants should have been one, or that one variant is quietly doing the work
of three different failure modes. That is review's job.

**Whether a message leaks too much or too little.** Named above as the same
unmechanizable trade `security.md` already states.

## See also

- Trait signatures and what may return a `Result`: [contracts.md](contracts.md)
- The panic and arithmetic bans: [code-structure.md](code-structure.md) rule 26,
  [security.md](security.md) rules 3–4
- `ObjectStore`'s error taxonomy as the worked example: [milestones/M1.md](../product/milestones/M1.md)
- Crate-boundary error typing: [docs/researches/19](../../researches/19-workspace-engineering.md) §3.3
