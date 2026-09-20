# 0050. Envelope encryption: the scheme, and what M8 can verify

Status: accepted; 2026-09-20: `M8.10` derives the writer epoch from the
coordinator's fenced epoch through `WriterEpoch`; see ADR-0054.
Date: 2026-09-20
Requirements: FR-41, FR-42, FR-43, NFR-14, NFR-33

⚠️ **Taken without the user's answer, and saying so**, as `ADR-0046` and
`ADR-0049` were. Two of its points move a requirement's evidence to a later
milestone, which is a decision rather than a detail; the user can override any
point before the code that depends on it lands.

## Context

`M8.md` settles the shape from doc 22 — segregate by key domain, a wrap/unwrap
seam that is the intersection of AWS and GCP, KMS on the rotation path only —
and leaves four questions open: key rotation semantics, revocation behaviour,
the DEK rotation constants, and nonce construction. All four are safety
decisions that the code cannot be written without.

It also states a completion condition with five clauses, two of which no
machine in this project can run: a round trip against **real** AWS KMS and GCP
Cloud KMS (there are no cloud credentials, and `M1` already deferred the same
class of evidence for real S3 and GCS to `M15`), and a **FIPS build**, whose
toolchain — `aws-lc-rs` FIPS with cmake and Go — `M13.md` task 5 already owns
as a separate build job.

## Decision

1. **The envelope.** One DEK per topic, wrapped by the topic's KEK through the
   `KeyProvider` seam. A sealed region carries `{key_id, wrapped_dek, nonce,
   alg}`; `alg` is the region-header field `M3` wrote and has carried as
   `none` ever since, and it is read at open time, never assumed.
2. **Nonce construction is a type, not a convention**: writer epoch ‖ object
   sequence ‖ region index, with no constructor able to produce a duplicate
   and no way to build one from raw bytes. A property test asserts no reuse is
   reachable under any call sequence. ⚠️ **This is the one thing in the
   milestone that is catastrophic to get wrong**: a repeated nonce under one
   AES-GCM key leaks plaintext and forges.
3. **DEK rotation: whichever comes first, 64 GiB encrypted or 7 days.** Both
   are constants, not settings. ⚠️ **Derived from the security argument, not
   the KMS quota**: with a counter nonce under one key the limit is how much
   plaintext may share a key, and 64 GiB is far below AES-GCM's own bound
   while keeping KMS calls at roughly one per topic per rotation — which is
   what NFR-33 asks for.
4. **Key rotation re-wraps lazily.** A DEK is re-wrapped under the current KEK
   version when it is next rotated or when compaction rewrites its data, never
   eagerly across the whole catalog. Old KEK versions must therefore be
   retained while any object sealed under them survives retention.
5. **Revocation is defined, and never a stuck queue.** A KEK the provider
   refuses makes reads of data sealed under it fail with a specific error a
   client can see, and makes compaction **skip** that key domain, recording
   why, rather than retrying forever or dropping the data. Produce to a topic
   whose KEK is revoked is refused, not silently written unencrypted.
6. **FR-41's real-cloud evidence is deferred to M15.** M8 builds both
   providers against one seam and proves portability against in-process
   simulations of each API — the same shape `M1` used for S3 and the same
   honesty: ⚠️ **a simulation proves the seam, not the vendor.** The round
   trip against real AWS KMS and real GCP Cloud KMS is `M15`'s, beside the
   real-S3 and real-GCS verification `M1.44` already deferred there.
7. **FR-43 moves to M13 whole.** The FIPS artifact needs the build job
   `M13.md` task 5 already plans; splitting it — a `fips` feature here, the
   artifact there — would put a runtime assertion and a differential test in
   a milestone that cannot build the thing they assert about. M8 leaves the
   algorithm agnostic at the seam, which is the property FR-43 needs and the
   reason the region header carries `alg` at all.

## Consequences

- M8's gate asserts FR-42, NFR-14, NFR-33 and FR-41-through-simulations. It
  cannot assert FR-41's vendor round trip or anything about FIPS, and says so
  in its own header rather than passing quietly.
- `roadmap.md`'s M8 requirements cell names FR-43 as **deferred, not
  delivered**, the shape `M4` already uses for FR-22.
- A KMS outage degrades reads for BYOK topics once a DEK ages out of cache.
  That is inherent to BYOK; it belongs in the SLO, and `M8.md`'s own risk list
  says so.
