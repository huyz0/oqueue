# `oqueue` (the binary)

## What is it?

The broker's composition root: the process that starts, chooses which concrete
implementation sits behind each `oqueue-core` seam, and hands the result to the
I/O shell.

## Why does it exist?

Because a library crate here does not reach for S3, a socket or a clock
directly — it takes a trait and lets something else choose (NFR-51) — and
something has to make that choice. ⚠️ **Which is a rule with named exceptions,
not a property of every crate.** `scripts/check-sans-io.sh` draws the line, and
what it actually exempts is: `oqueue-store` from the object-storage pattern,
because it is the crate that implements those backends; and `oqueue-broker`
from all three, because the I/O shell is meant to live there. Every other
crate is held to all three — ⚠️ but `bin/` is not scanned at all, so nothing
here is held to any of them; this crate's own Invariants table says so too.

⚠️ This sentence claimed *every* library crate until `M1.28`. `oqueue-store`
had falsified it since `M1.15` gave it a real S3 backend — not, as that row
predicted, the connection loop.

⚠️ **This is the only place that choice is made** (FR-50) — which is what lets
the entire system be tested with no network, no credentials and no container.

## Upstream

- `oqueue-core` — the seams and the types.
- `oqueue-crypto` — `NoOpKeyProvider`, the default when no KMS is configured.
- `oqueue-broker` — the I/O shell `serve` composes: connection task, dispatcher, stub cluster (`M2.25`).
- `tokio` — the runtime under `serve`'s listener; `net` arrived exactly when this binary bound one.
- `mimalloc` — the global allocator. ⚠️ C, compiled by `cc` at build time;
  within NFR-42 ("cargo and a C compiler") and recorded in ADR-0007.
- `tikv-jemallocator` — **optional**, behind the non-default `heap-profiling`
  feature. ⚠️ It bakes in the *build host's* page size, so a binary built on a
  4 KB-page host aborts at startup on a 64 KB-page aarch64 kernel. Build with
  `JEMALLOC_SYS_WITH_LG_PAGE=16` when those hosts are in scope. ADR-0007.

⚠️ **A composer.** `check-layering.sh`'s `COMPOSERS` set is
`{oqueue-broker, oqueue}`; every other crate may depend on `oqueue-core` and
nothing else here.

## Downstream

Nothing. It is the top of the graph.

## Invariants

| Must stay true | Held by |
|---|---|
| The only place the *composition* is chosen — which concrete implementation the running broker gets | ⚠️ **No gate.** `check-sans-io.sh` does not scan `bin/`, so this is review's. ⚠️ Narrowed by `M1.28` from "the only place a concrete implementation is named", which `oqueue-store` falsifies: it names `S3Store` and `GcsStore` because it defines them |
| No `oqueue-testkit` in `[dependencies]` | `scripts/check-layering.sh` — ⚠️ the *only* dependency rule it enforces here, since a composer is exempt from the star-topology one |
| Depends only on `oqueue-core` and the crates it composes | ⚠️ **No gate.** Composer exemption means `check-layering.sh` accepts any workspace dependency; review's |
| No `unsafe` | `#![forbid(unsafe_code)]`, `scripts/check-unsafe.sh` |
| Every dependency in `Cargo.toml` is named in `## Upstream` below | `scripts/check-readmes.sh` — ⚠️ **only since `M0.21`**; this crate was outside its glob while `M0.13` added the allocator |
| The global allocator is set here and in no library crate | ⚠️ **No gate.** ADR-0007; one `grep` from being checkable, review's until it is |

## Notes for whoever touches this

- **With no arguments it prints a version and exits;** `oqueue serve
  <host:port> [advertise]` binds a listener and serves the M2 wire protocol
  over a stub partition (`M2.25`) — real durability is `M3`'s. `advertise`
  overrides the identity `Metadata` hands out (doc 02 §7.2: identity is a
  decision); the harness's capture proxy relies on it.
- ⚠️ **`cargo build` links this on x86_64 only.** aarch64 stays at `cargo check`
  until a cross-linker exists, which is `M13`'s work — and `M0`'s completion
  condition says so rather than claiming a link it never performed.
- ⚠️ **The default is `NoOpKeyProvider`, which refuses.** ADR-0006: a deployment
  with no KMS configured should fail the first encryption call, not silently
  write a plaintext data encryption key beside the data it protects.
