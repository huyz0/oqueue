# 0002. Async runtime, and how a core seam is expressed

Status: accepted
Date: 2026-08-15
Requirements: NFR-51

## Context

`M0.9` through `M0.11` add three trait seams to `oqueue-core` — `Clock`,
`ObjectStore`, `KeyProvider` — and two of them are asynchronous. Two questions
have to be answered before the first is written, because both are expensive to
change once three traits and their fakes exist.

**Which runtime**, and **what shape an async method on a core trait takes**. The
second decides whether the broker's I/O shell can hold an `Arc<dyn ObjectStore>`
at all, and the composition root exists precisely to choose a backend at startup
(FR-50).

The constraint over both is **NFR-51**: `oqueue-core` names no concrete I/O
type. A trait that mentions a runtime in its signature is not a seam.

## Decision

**Tokio**, on doc 05 §3's evidence — an order of magnitude more downloaded than
any alternative, and the substrate hyper, tonic and axum are built on. Nothing
in this project's shape argues against the default.

⚠️ **`oqueue-core` names it nowhere**, and no crate takes the dependency until it
does I/O — which is no task in M0. See *Consequences*.

**Seam shape: one `dyn`-compatible trait per async seam, returning a boxed
future written by hand.**

```rust
pub trait ObjectStore: Send + Sync + fmt::Debug {
    fn get<'a>(&'a self, key: &'a ObjectKey)
        -> Pin<Box<dyn Future<Output = Result<Payload>> + Send + 'a>>;
}
```

⚠️ `Payload` is a placeholder, and deliberately not `bytes::Bytes`. What the
seam carries is `M0.10`'s decision; what this ADR fixes is that it must be a
type **`oqueue-core` owns** (`contracts.md` rule 8), because core takes no
dependency and `oqueue-buf` is downstream of it. ⚠️ Nothing enforces this:
`check-layering.sh` intersects each manifest's dependencies with *workspace*
crate names, so an external crate added to `oqueue-core` passes every gate.

This is `contracts.md` rule 7's stated default — "an object-safe,
`dyn`-compatible trait is the default for a seam meant to be swapped at
runtime" — and nothing here earns the exception. `Clock` is not async and needs
no boxing at all. `KeyProvider` takes the same shape as `ObjectStore`: its
wrap/unwrap calls reach a KMS, so they are I/O-bound too.

⚠️ **The `+ Send` on the boxed future is written explicitly**, because a
`dyn Future` is opaque — nothing infers it, and without it no spawned task can
hold the future.

⚠️ **Both input lifetimes are tied to one `'a`.** Independent lifetimes work
too, and a purely delegating wrapper needs no extra bound — but relating them
later is a change to the *trait's own* signature, which for an `oqueue-core`
seam is a contract change under `contracts.md` rule 12. Tying them now costs a
caller nothing and keeps every wrapper form reachable without one.

## Alternatives considered

**Native `async fn` in trait.** Rejected: **measured on 1.97.1, it is not
`dyn`-compatible** — `&dyn Store` over a trait with `async fn get` is
`error[E0038]`, "consider moving `get` to another trait". Since `bin/oqueue`
picks a backend at startup, a seam with no trait-object form is one the
composition root cannot use.

**`async fn` in trait *plus* a second, bridged `dyn`-compatible trait.**
Rejected, and this was the first draft. It compiles — measured, both the blanket
`impl<T: Store> DynStore for T` and the `impl Store for Arc<dyn DynStore>` that
makes a generic consumer usable from the composition root. Three reasons it
loses. Its only advantage is avoiding one allocation per call, and the argument
that the allocation matters does not survive contact with the requirement: it
would have cited NFR-2's 10 ms cached read, but NFR-2 is verified by *zero
object-storage round trips*, so an `ObjectStore` call is not in that budget —
and where the call does happen, one `Box::pin` is noise against 60–250 ms of
network. It doubles every async seam, and ⚠️ `check-core-contract.sh` compares
method sets per trait name and does not know two traits are paired, so a drifted
bridge is caught by nothing. And it takes `contracts.md` rule 7's exception
without a reason that holds.

**`#[async_trait]`.** Rejected. It boxes exactly as the hand-written form does,
and adds a proc-macro dependency to `oqueue-core` — the crate every other crate
rebuilds behind. The macro's value is saving the signature above from being
typed out; that is not worth a dependency at the centre of the graph.

**A different runtime (`smol`, `async-std`, `monoio`).** Rejected without
detailed comparison, which is the honest record: the object-store SDKs, `hyper`,
and `madsim`'s simulation targets are all Tokio-shaped, so choosing otherwise is
a decision to leave the ecosystem, and nothing here needs that.

## Consequences

**Easy.** The composition root holds `Arc<dyn ObjectStore>` and chooses S3, GCS,
or the in-memory backend at startup. Every seam is one trait, so a fake is one
fake and `check-core-contract.sh`'s implementor check covers it. `oqueue-core`
still compiles with no dependency at all.

**Hard.** One heap allocation per seam call, unconditionally, including from
code that statically knows its backend. If a future benchmark shows it matters
on a specific path — which would require that path to be seam-crossing *and* not
I/O-bound — the escape is to add a generic method beside the boxed one,
⚠️ **which requires `where Self: Sized`**: measured, without it the trait stops
being dyn-compatible (`E0038`, "references an impl Trait type in its return
type") and every `Arc<dyn ObjectStore>` breaks at once. With the bound it
compiles, and the method is simply unavailable through a trait object — which
is the correct trade, since a caller reaching for it knows its backend.

⚠️ **No M0 task adds an async dependency.** `Cargo.toml`'s comment and
ADR-0001's said the runtime "arrives with M0.4"; this ADR is a decision, not a
dependency, and both are corrected in this commit. The consequence lands on
`M0.16`, which measures NFR-56's pre-commit constant on a workspace with no
async dependency compiled — a floor, and it must say so rather than presenting
the number as steady state.

**Deterministic simulation.** doc 10 #32 called DST placement open, warning that
`madsim` "touches every crate that does async". ⚠️ doc 10's own resolved log
supersedes that: `madsim` swaps the runtime by `cfg` rather than changing crate
structure, so its answer "affects roughly one paragraph of `testing.md` — not
`architecture.md` or the crate split". This ADR is not blocked on the DST spike,
nor it on this. What follows: a simulated backend implements these seams, so DST
cost is bounded by the number of seams, not the number of crates.

**Foreclosed.** Nothing structural. Swapping Tokio later is a large mechanical
change, and the seams are what insulate the business logic from it — the
property NFR-51 exists to buy.
