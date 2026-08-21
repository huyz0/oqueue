---
title: "Security"
description: >
  Read when touching the wire protocol, anything parsing untrusted input, secrets, key material, tenant isolation, or unsafe.
tags: [quality, untrusted-input, secrets, crypto, isolation, unsafe]
applies_to: ["*oqueue-core/*", "*oqueue-codec/*", "*oqueue-crypto/*", "*oqueue-broker/*", "*oqueue-store/*", "*oqueue-buf/*", "*oqueue-checksum/*", "*oqueue-coordinator/*", "*bin/oqueue/*", "*.pem", "*.key"]
---

# Security

Rules for a multi-tenant broker that terminates untrusted connections and holds
customer key material. Rationale lives in the corpus; this file is the rules.

Each rule names its gate, or is marked as having none.

## The threat model in one paragraph

Anyone who can reach the port is untrusted. They send length-prefixed binary
frames that we parse before we know who they are. Once authenticated they are
still untrusted with respect to every *other* tenant. We hold data encrypted
under keys their owners can revoke, and we run in a process that also holds
other tenants' data and other tenants' keys.

## Untrusted input

The wire protocol decoder is the highest-risk code in the project. It parses
attacker-controlled bytes from an unauthenticated peer.

1. **Anything sized by a client-supplied number has a maximum.** A client
   claiming a 2 GB frame gets an error, not an allocation. → fuzz target,
   `check-unsafe.sh` for the buffer paths
2. **Validate before allocating**, never allocate then validate.
3. **No `panic!` reachable from client bytes.** A malformed frame must not take
   down a node serving other tenants. → clippy `unwrap_used`/`expect_used`
   denied outside tests; fuzz targets run to a corpus
4. **Integer arithmetic on parsed lengths is checked.** `overflow-checks = true`
   in every profile including release, and `unchecked_add`/`sub`/`mul` are
   banned outright. ⚠️ The precedent is RUSTSEC-2026-0007: an unchecked addition
   produced an out-of-bounds slice from entirely safe calling code, and debug
   builds panicked while release wrapped, so tests could not see it. →
   `scripts/check-layering.sh` asserts `overflow-checks = true` under the root
   `[profile.release]` (`M0.21`); ⚠️ **the `unchecked_*` grep gate is not
   written** — no backlog row names it, so it is unscheduled rather than done
5. **Every decoder has a fuzz target**, seeded from a corpus and run in the
   nightly tier. → `scripts/fuzz.sh`

## Secrets and key material

6. **No secret reaches a log, span, metric label, error variant, or admin
   response.** Secrets live in a type that cannot be printed; the only route to
   a value is an explicit `expose()`, and no result of that may reach a
   formatting macro. → `check-secrets.sh` holds the static half; the end-to-end
   suite searches every node's log for known values
7. **Nothing holding a secret derives `Debug`.** → the same gate
   ⚠️ **One carve-out, and it is a type rather than an exception:** a struct may
   derive `Debug` when the secret is inside `oqueue_core::Redacted<T>`, whose
   own `Debug` and `Display` take no `T: Debug`/`T: Display` bound and so cannot
   reach the value at any format specifier. A gate implementing this rule
   should treat a `Redacted<_>` field as satisfying it, not as a violation.
   ⚠️ `Redacted` is a *formatting* guarantee only — it does not zeroize, so
   rule 8 still applies to what goes inside it.
8. **Key material is zeroized on drop.** A cached DEK is plaintext key material
   in process memory. → `zeroize` on every key type, checked by review
9. **Key material never lands on disk**, including in a core dump, a heap
   profile, or a crash artifact.

## Multi-tenant isolation

10. **Every operation is scoped to the authenticated principal.** A tenant must
    not observe, address, or exhaust another tenant's resources. → test
    asserting cross-principal access is refused on every API (FR-40)
11. **`Metadata` responses are principal-scoped**, and their cost is
    O(topics this principal can see). This is a security property *and* a scale
    property — see doc 15 §4. → FR-4, NFR-12
12. **No object contains regions from two key domains.** → FR-42, and a test
    asserting it
13. **A tenant cannot exhaust a shared resource** — connection counts, memory,
    in-flight requests are bounded per principal, not only globally.

## Cryptography

14. **Never implement a primitive.** AEAD, hashing, and key derivation come from
    a vetted implementation. `oqueue-crypto` composes; it does not invent.
15. **AEAD nonces are constructed, never drawn at random.** Writer epoch ‖
    object sequence ‖ region index. A nonce reused under one key is catastrophic
    for GCM, and at this volume random 96-bit nonces have an uncomfortable
    birthday bound. ⚠️ **This is the single highest-severity invariant in the
    codebase.** Encode it in a type, not a comment. → review, and a property
    test asserting no nonce repeats across a large synthetic run
16. **The algorithm is named in the data**, not assumed by the reader. A FIPS
    build must read what a non-FIPS build wrote. → FR-43
17. **KMS is never on the per-batch path.** → NFR-33

## `unsafe`

18. **`unsafe` exists in three crates only** — `oqueue-buf`, `oqueue-codec`,
    `oqueue-checksum` — and never in the async or concurrency layer, where Miri
    is blindest. Everything else is `#![forbid(unsafe_code)]`. A fourth crate
    requires a recorded decision. → `check-unsafe.sh`
19. **Every unsafe block satisfies all six conditions** in doc 18 §5.7:
    a benchmark, the safe alternatives tried first, a `SAFETY:` comment naming
    the precondition, a `debug_assert!` of it, the obligation encoded in a type
    where possible, and a differential property test kept forever.
20. ⚠️ **Soundness bugs cluster in safe APIs that fail to validate input**, not
    in unsafe blocks. Review the boundary, not only the `unsafe` keyword.

## Dependencies

21. **`cargo-deny` runs in CI**, and an advisory fails the build. → supply-chain job
22. **A new dependency in a crate that parses untrusted input is a decision**,
    and gets an ADR naming what it does and why a smaller option was rejected.
23. **Prefer fewer, larger, well-maintained dependencies** over many small ones
    on the untrusted path.

## What has no gate

**Whether an error message leaks information usefully to an attacker** while
still helping an operator. That trade cannot be mechanized; it belongs to review
against the error-handling standard.

**Whether the threat model above is still the right one.** It is a claim about
the world, and it should be revisited at milestone boundaries rather than
assumed permanent.

## See also

- Encryption design and BYOK: [docs/researches/22](../../researches/22-encryption-byok-and-fips.md)
- `unsafe` policy in full: [docs/researches/18](../../researches/18-rust-performance-methodology.md) §5
- Public reporting policy: [SECURITY.md](../../../SECURITY.md)
