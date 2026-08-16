# 0012. `ring` as the default build's TLS crypto provider

Status: accepted
Date: 2026-08-17
Requirements: NFR-42

## Context

ADR-0008 picked the `object_store` crate but explicitly deferred one thing:
*"the TLS backend is `oqueue-store`'s choice to make (`M1.15`, `M1.17`) when
it actually compiles this in."* `M1.15` is that commit. `object_store`
offers two crypto-provider features for its HTTP client stack: `ring` and
`aws-lc-rs`.

The choice is not free either way. Doc 22 §7 (FIPS as a separate build)
records that `aws-lc-rs`'s FIPS-validated path needs **CMake, Go, and a
C/C++ compiler** at build time — and `build.md` rule (NFR-42) requires the
*default* build to need only cargo and a C compiler, with the FIPS build as
the one named exception. Picking `aws-lc-rs` here, in the crate every default
build already compiles, would put a Go toolchain requirement on every user
who never asked for FIPS — exactly the cost doc 22 §7 says justifies keeping
FIPS as a separate artifact in the first place, undone by a dependency choice
nobody would have called an M8 decision.

## Decision

**`ring`** for the default build. It needs no CMake or Go — only `cc`, which
is already within NFR-42's "cargo and a C compiler" budget (the same budget
`mimalloc`/`tikv-jemallocator` already use, per `build.md`'s existing
dependency-cost rule). `object_store`'s default build path against S3/GCS
compiles and runs identically under `ring` for every deployment that never
touches BYOK or FIPS — which doc 22 §8 puts at over 99% of them.

`M8`'s FIPS artifact is where `aws-lc-rs` belongs: a `fips` Cargo feature
flips the provider for that build only, exactly as doc 22 §7 already
specifies, and this ADR does not need to build that feature now — `object_store`
itself is feature-gated per crypto provider, so the two are mutually
exclusive at compile time by construction, not by a runtime check either
build could get wrong.

**The mechanism this actually took, found while building `M1.15`.** Picking
`ring` is not a single Cargo feature flip, because two dependencies bundle
`aws-lc-rs` *unconditionally* rather than offering it as one option among
several:

- `object_store`'s own `aws` convenience feature lists `aws-lc-rs` as a
  bundled component in its `Cargo.toml`, with no way to opt out short of not
  using that feature. `aws-base` is the actual module gate
  (`#[cfg(feature = "aws-base")]`, confirmed in `object_store`'s own `lib.rs`
  doc comments) and pulls in no crypto provider on its own.
- `reqwest` 0.13's `rustls` feature does the same — it hard-bundles
  `aws-lc-rs` via `__rustls-aws-lc-rs`. The only TLS-enabling feature that
  does not is `rustls-no-provider`, which wires up TLS with **no** provider
  chosen at compile time, deferring that choice to a runtime call.
- Cargo has no syntax to reach through `object_store`'s manifest and request
  a feature (`reqwest/rustls`) of *its* dependency from outside — that syntax
  is only legal in the manifest of the crate that declares the optional
  dependency.

So `oqueue-store` declares `reqwest` (`rustls-no-provider`) and `rustls`
(`ring`, `default-features = false`) as its own feature-only dependencies,
pinned to the same versions `object_store` resolves. Cargo's whole-graph
feature unification means this steers the single, *shared* `reqwest`/`rustls`
instances `object_store` also depends on — there is only ever one `reqwest`
and one `rustls` in the build, and the feature set is the union of every
crate's request. `crates/oqueue-store/src/tls.rs::install_ring_provider`
installs `ring`'s `CryptoProvider` as the process default at runtime, once,
before any `S3Store` is constructed — confirmed by `cargo tree -p
oqueue-store -i aws-lc-sys` / `-i aws-lc-rs` finding neither package in the
graph at all.

## Alternatives considered

**`aws-lc-rs` for every build, dropping the separate FIPS artifact.**
Rejected: this is the exact cost doc 22 §7's whole argument is built to
avoid — a Go and CMake requirement on every developer and every CI run, for
a compliance property fewer than 1% of deployments need. `build.md`'s NFR-42
rule exists precisely to keep this decision from being made by accretion.

**Deferring the choice further, compiling neither and leaving `object_store`
uninstantiable.** Rejected: `M1.15` is the commit that actually builds the S3
backend, so a choice has to be made now for anything to compile at all — the
deferral ADR-0008 recorded was to *this* commit, not past it.

## Consequences

**Easy.** The default build's dependency footprint stays at cargo + a C
compiler, matching every crate already in the workspace. `M8`'s FIPS build
adds its own feature-gated `aws-lc-rs` path without touching this one.

**Hard.** `M8` needs to actually wire the `fips` feature and prove the two
providers are mutually exclusive per artifact, not merely per intention —
that verification is `M8`'s to do, not `M1`'s, since `M1` never builds a
FIPS artifact to check.
