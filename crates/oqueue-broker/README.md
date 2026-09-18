# `oqueue-broker`

## What is it?

The I/O shell: the connection loop, request dispatch, and the batching that turns many small produces into few large PUTs.

## Why does it exist?

Because *almost* everything else in this workspace is sans-I/O — ⚠️ **"by construction" overstates it, and `M1.38` found this variant of the same universal**: `check-sans-io.sh` exempts `oqueue-store` from the object-storage pattern because it implements those backends, and never scans `bin/oqueue` at all — it walks `crates/*.rs` only. So there are two exceptions besides this crate, which is itself exempt from all three patterns; every *remaining* library crate is held to all three. And the sockets have to be somewhere. This crate is that somewhere, and it is generic over its seams so it can be tested without any of them being real.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.
- `oqueue-codec` — the protocol surface: frames, headers, batches.
- `oqueue-checksum` — the CRC-32C half of the ingest rule produce verifies batches with.
- `thiserror` — the derive every public error here uses (`error-handling.md` rule 3).
- `oqueue-coordinator` — where offsets are assigned and journalled. ⚠️ **The concrete materialized index and the concrete object store are *not* here**: both are seams in `oqueue-core`, chosen by `bin/oqueue`, so this crate has no `oqueue-index` or `oqueue-store` dependency.
- `oqueue-compact` — the expiry heap and object lifecycle `retention.rs` runs every round (`M5.91`); the task that calls them does I/O, so it lives in this shell rather than beside them.
- `tokio` — the runtime this I/O shell is written against (feature-minimal: io, sync, rt, time, macros — the last for `connection.rs`'s `select!`; `net` waits for `bin/oqueue`).
- `uuid` — the type this crate converts `oqueue-codec`'s `[u8; 16]` topic ids to and from at the seam.
- `rustls` — `tls.rs`'s TLS termination (`M9.5`, `ADR-0012`'s `ring` default build).
- `rustls-pki-types` — the certificate/key types `tls.rs` parses PEM into.
- `tokio-rustls` — wraps a stream in `rustls`'s handshake; generic over `S: AsyncRead + AsyncWrite`, the same seam `serve_connection` already is, so no `tokio` "net" feature is needed here.

⚠️ **`kafka-protocol` is a `dev-dependency` now, not runtime (`ADR-0019`).**
`ADR-0017` had it as the dispatcher's generated message layer; `M2.31`-`M2.34`
hand-rolled every message this broker answers, and `kafka-protocol` is now
only what this crate's own tests use to build request bytes and decode
responses — so it is not in `## Upstream`.

## Downstream

`bin/oqueue`, the composition root.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| Names no **seam implementation** — no concrete `ObjectStore` or `KeyProvider`. ⚠️ **`Clock` was called contested here until `M1.29` read the three documents side by side and found no disagreement to settle**: they answer different questions. `check-sans-io.sh`'s clock exemption says where the real implementation *lives* — "in `oqueue-broker` per the same reasoning as the socket exemption". `architecture.md` says where concrete types are *chosen* — `bin/oqueue`, the composition root. Those are the same two answers `ObjectStore` already has, and nobody calls that contested: `S3Store`'s code lives in `oqueue-store`, and `bin/oqueue` is where it *is* picked — ⚠️ **that wiring is written now** (`M3.14`): `bin/oqueue` depends on `oqueue-store` and selects a backend, which is the deliberate act `M1.39` said adding the dependency would be. The sentence here said "would be picked" and "does not depend on `oqueue-store` today" until this commit made both false. ADR-0004 takes no position on where the *real* implementation lives; its one mention of "`M1`'s real `Clock`" is about NTP clamping having no gate. (It does place the *fake*, beside the trait in `oqueue-core`, rejecting `oqueue-testkit` — a location claim about a different thing.) **So this row does not claim `Clock` because `check-sans-io.sh` says the real one lives here** — asserting "names no concrete `Clock`" would be a negative invariant this crate is expected to break, the same defect `M0.31` fixed for sockets. ⚠️ Not because two documents disagree about where it goes, and not merely because the implementation is unwritten: *unwritten* is the condition under which the claim would be true today, which is exactly why it would be the wrong thing to write down. | ⚠️ **No gate, and specifically not `check-sans-io.sh`** — that script *exempts* this directory. Review is the only thing holding it, and this is the crate where that matters most. ⚠️ **Sockets are excluded from this row on purpose**, and saying so is the point: "What is it?" above calls this crate the connection loop and "Why does it exist?" says the sockets have to be somewhere and this is that somewhere. The row said "no concrete socket type" from `M0.8` until `M0.31` removed it. That wording asserted the negation of the crate's own reason for existing — stated in the same file, above this table — and would have been falsified by the first commit that wrote the loop. `architecture.md` names three seams and none is a socket |
| Depends only on `oqueue-core` and its siblings as a composer | `scripts/check-layering.sh` |

## Notes for whoever touches this

- ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
- **Generic over its seams, not hardwired to them.** The concrete `ObjectStore` and `KeyProvider` are chosen by `bin/oqueue`; this crate names neither (NFR-51). ⚠️ **`Clock` is left out of this bullet on purpose** — see the Invariants row above. `check-sans-io.sh`'s exemption says the real one lives here; this file asserted the opposite until `M0.31`. ⚠️ **Where it lives is not who constructs it** — that is the same distinction the Invariants row above turns on, and nothing here decides the wiring.
- ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.
