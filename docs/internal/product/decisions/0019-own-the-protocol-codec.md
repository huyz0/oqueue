# 0019. Own the protocol message codec; kafka-protocol becomes a test-only oracle

Status: accepted
Date: 2026-08-24
Requirements: FR-1, FR-2, FR-3
Supersedes: 0017 (which chose `kafka-protocol` as the runtime message layer)

## Context

`ADR-0017` chose `kafka-protocol` for request decoding and response encoding,
and hand-rolled only the frame codec and the RecordBatch hot path. Its
rejection of hand-rolling everything rested on one assumption, stated in its
own Alternatives: hand-rolling "reintroduces both compat-bug classes doc 02
names ... roughly doubling the milestone for negative compatibility value."
That reasoning was sound **when it had no oracle**. Two things changed it:

1. **`M2.25`-`M2.26` built the oracle `ADR-0017` lacked.** A byte-exact golden
   corpus captured from real librdkafka, produce+fetch round trips by
   librdkafka *and* the Java client, an FR-2 matrix test over every advertised
   `(api, version)`, and a fuzz harness. The compat bugs `ADR-0017` feared are
   now caught by construction, not by inspection — the negative value it
   weighed is largely bought back.

2. **`M2.26`'s fuzz target found a security defect the dependency cannot be
   made to not have.** `kafka-protocol`'s generated array decoder
   (`protocol/types.rs:988`, `:1096`) calls `Vec::with_capacity(n)` on an
   attacker-controlled element count with no bound against remaining bytes: a
   64-byte `Fetch` v16 frame demands ~30 GB, and one unauthenticated packet
   OOM-kills the broker. `with_capacity` aborts rather than returning an error,
   the crate exposes no allocation hook, and `n` is not bounded by frame size
   — so `security.md` rule 1 ("a 2 GB frame gets an error, not an allocation")
   cannot be satisfied without changing the decoder. The only deliveries of a
   two-line fix are vendoring 97k generated lines (a permanent fork, dwarfing
   the codebase) or an upstream PR not resolvable in-session. M2's independent
   milestone review ranked this **blocking**: M2 cannot be honest about rule 1
   while it stands.

This project's stated targets — turbopuffer-scale throughput, WarpStream's
architecture — put produce and fetch decode/encode on the hottest path there
is. Reference systems at that scale own that codec; a generated,
allocation-eager dependency on it is the wrong tool for both reasons the user
named: **performance** (the hot path should not route through a general
decoder) and **security** (the input layer must validate before it allocates,
which `oqueue-codec`'s `wire.rs` already does for every primitive).

## Decision

**`oqueue` owns its Kafka protocol codec.** Request decoding and response
encoding for every advertised message — `ApiVersions`, `Metadata`, `Produce`,
`Fetch` — are hand-rolled in `oqueue-codec`, against `wire.rs`'s bounded
primitives, extended with the flexible-versions machinery (compact
varint-length strings/bytes/arrays, tagged-fields sections) the modern wire
needs. The RecordBatch hot path, the frame codec, and CRC verification stay
ours as they already are.

- **Validate before allocating, everywhere.** Every count and length read from
  the wire is bounded against the bytes that remain before a collection is
  sized (`security.md` rules 1-2). This dissolves the `M2.27` DoS by
  construction — there is no `with_capacity(untrusted)` to exploit.
- **`kafka-protocol` becomes a `dev-dependency` only**, retained as a
  differential oracle: our encoder's bytes must decode under it and its
  encoder's bytes under ours, at every advertised version, alongside the
  captured-from-real-clients golden corpus. No runtime crate imports it. The
  compat-bug classes doc 02 names are held off by this differential plus the
  corpus plus the real-client harness — the oracle `ADR-0017` did not have.
- **Version-gated field presence and flexibility are data, not scattered
  conditionals.** Each message's per-version shape is expressed once, so the
  two error-prone axes doc 02 §6.4 names (version-gated presence,
  flexible-versions rules) live in one reviewed place per message rather than
  smeared across encode and decode.

## Alternatives considered

- **Vendor + patch `kafka-protocol`** — rejected: a two-line semantic fix
  delivered as a 97k-line permanent fork, re-applied on every bump, and it
  leaves the hot path routed through a general decoder. Fixes security, not
  performance, at a standing maintenance cost.
- **Upstream the allocation bound + pin** — the right long-term fix for other
  users, pursued in parallel, but not resolvable in this milestone and still
  leaves the hot-path performance argument unaddressed.
- **Schema-aware pre-parse in front of the dependency** — rejected: duplicates
  the dependency's per-message schema knowledge to guard it, the worst of both
  (a hand-rolled schema layer *and* the dependency), and fragile against bumps.
- **Keep `ADR-0017`, document the DoS, defer** — rejected by the milestone
  review's blocking rank: `security.md` is binding regardless of the storage
  stub, and the defect is permanent request-decode debt, not milestone-crossing
  scope like durable storage.

## Consequences

- `M2.27` (the DoS) is resolved by this rewrite rather than by a bound on the
  dependency; `M2.28`-`M2.34` are its decomposition. The request-path fuzz
  target that found it lands green when the `Fetch` decoder is ours.
- `ADR-0017` is superseded, not amended: its Decision (the dependency as the
  single runtime protocol surface) no longer holds. Its status line records
  this; its hand-rolled-hot-path half is subsumed here, now the whole codec.
- The golden corpus and the real-client harness stop being drift detectors on
  a dependency and become the compatibility proof of our own codec — a
  stronger claim, and the reason this reversal is safe now when it was not at
  `ADR-0017`'s writing.
- Cost is real: the message-times-version matrix is now ours to get right.
  The oracle makes that a bounded, checkable effort rather than the unbounded
  one `ADR-0017` faced — but it is the milestone's new center of gravity.
