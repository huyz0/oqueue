# 0011. `put` gains an `Option<Precondition>` parameter

Status: accepted
Date: 2026-08-16
Requirements: FR-31

## Context

ADR-0005 deferred conditional writes out of `ObjectStore` explicitly: *"the
right shape depends on what S3's `If-None-Match` and GCS's generation
preconditions turn out to have in common, which is `M1`'s to discover."*
`M1.5` answered that with `Precondition { IfAbsent, IfMatches(PreconditionToken)
}`. `M1.6`'s task — conditional writes on the fake — needs somewhere on the
trait to carry that value, and that is a `contracts.md` rule-12 method-set
change, so it gets its own ADR rather than riding in on `M1.6`'s.

## Decision

`put` gains a third parameter, `precondition: Option<Precondition>`. `None`
is today's behaviour: an unconditional overwrite, racing writers are
last-writer-wins. `Some(p)` makes the write happen only if `p` holds against
the key's current state, checked and applied atomically under whatever
serialization point the implementor already has — the fake's single `Mutex`
acquisition, and (for `M1.15`/`M1.17`) whatever `object_store`'s `PutMode`
gives natively, since `ADR-0008` chose it specifically for this.

`Error::PreconditionFailed { key }` is the failure this can now produce —
dissolved in from the retired `M1.4`, landing here as the first thing to
actually construct it.

## Alternatives considered

**A separate `put_if` method, leaving `put` unconditional.** Rejected: it
would give every future caller two methods to choose between where one
parameter already says everything a call site needs, and `object_store`'s own
`PutMode` (the crate `ADR-0008` chose) unifies the same three states — a
plain overwrite, create-if-absent, update-if-matches — behind one call rather
than two. Splitting the trait when the library underneath does not split its
own is a needless divergence.

**A three-variant `PutMode`-shaped enum on the trait** (`Overwrite`,
`IfAbsent`, `IfMatches(token)`), replacing `Option<Precondition>`. Rejected as
redundant: `None`/`Some(Precondition::IfAbsent)`/`Some(Precondition::IfMatches(_))`
already expresses exactly those three states, and inventing a fourth type to
say the same thing `Option<Precondition>` already says is exactly the kind of
duplicate vocabulary `rust-style.md` warns against.

## Consequences

**Easy.** Every existing call in this crate's own tests needed `, None`
appended, which is mechanical; nothing outside `oqueue-core` implements or
calls `ObjectStore` yet, so — as with `ADR-0010` — this is the cheapest this
contract will ever be to change again.

**Hard.** `M1.15`'s S3 backend and `M1.17`'s GCS backend both need to map
`Option<Precondition>` onto `object_store::PutMode` faithfully, including the
one-directional trap `ADR-0008` already recorded: only the finishing call of
a multipart upload can carry a condition, never an individual part — which is
exactly what `M1.16`'s row already names as its own concern.
