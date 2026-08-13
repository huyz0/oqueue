---
title: "Encryption: Per-Topic BYOK and FIPS Builds"
slug: encryption-byok-and-fips
status: working-position
last_updated: 2026-08-13
tags: [encryption, byok, kms, aws-kms, gcp-kms, envelope-encryption, fips, aws-lc-rs, rustls, compliance, multi-tenancy, dek, kek]
related: [15-scale-architecture-position, 12-object-discovery-and-api-cost, 20-build-and-release-portability, 04-object-storage-s3-gcs]
summary: >
  Two requirements added 2026-08-13: per-topic BYOK portable across AWS KMS and
  GCP Cloud KMS, and FIPS 140-3 as a separate build. Both are optional. Core
  findings: per-topic keys and multi-topic object batching collide, and the
  resolution is per-region encryption inside one object; SSE-KMS cannot satisfy
  the requirement at all, so broker-side envelope encryption is forced; AWS KMS
  caps customer-managed keys at 100,000 per region, so "per topic" must mean
  per-topic configuration rather than per-topic KMS key; and the portable KMS
  abstraction is wrap/unwrap, not generate-data-key, because GCP has no
  GenerateDataKey equivalent.
---

# Encryption: Per-Topic BYOK and FIPS Builds

**⚠️ A design position**, in the sense of [15](15-scale-architecture-position.md). Requirements stated 2026-08-13; this document derives what they imply.

Marked **[Documented]**, **[Assessment]**, or **[Design]**.

---

## 0. The requirements

1. **BYOK** — customers supply their own key material, configured **per topic**, portable across **AWS KMS** and **GCP Cloud KMS**. Optional; the default is a provider-managed key.
2. **FIPS 140-3** — available as a **separate build**. Optional; not the default artifact.

Both are opt-in. Neither may impose cost on deployments that do not use them — which turns out to be the constraint that shapes most of the design.

---

## 1. Why SSE-KMS cannot satisfy this

**[Assessment]** The obvious implementation is to let the object store encrypt: S3 SSE-KMS or GCS CMEK, pointing at the customer's key. **It does not work here, for a structural reason.**

Server-side encryption is scoped to a *bucket or object*. But this architecture's entire cost model depends on **bundling records from many topics into one object** ([06](06-distributed-systems-design-challenges.md), [12](12-object-discovery-and-api-cost.md)) — object-storage PUT pricing and latency both favour fewer, larger writes. One object therefore contains many tenants' data.

An object can carry exactly one SSE-KMS key. So SSE-KMS offers a choice between:

- **one key for the whole object** — every tenant in that object shares a key, which is not BYOK in any meaningful sense; or
- **one object per topic** — which destroys the batching that makes the design economical.

**Neither is acceptable, so encryption must happen broker-side, before the bytes reach the object store.** That is a forced conclusion rather than a preference, and it has the useful side effect of making the scheme identical on S3, GCS, and any other backend — which is what "portable" requires anyway.

SSE-KMS remains worth enabling underneath as defence in depth. It just cannot be the mechanism.

---

## 2. The collision, and the resolution

**[Assessment]** Per-topic keys and multi-topic batching appear to be in direct conflict: records encrypted under different keys cannot share an encryption context.

**The resolution is that they do not need to share one.** An object is already a *container of independently addressable regions* — the read path never reads a whole object, it issues a **ranged GET** resolved through the offset→object index ([12](12-object-discovery-and-api-cost.md) §3). Each partition's data is already a contiguous region within the object.

So: **encrypt each region independently, under the key belonging to its topic.** One object, one PUT, many encryption contexts.

```
  object (one PUT, many tenants)
  ┌──────────────────────────────────────────────────────┐
  │ region A: topic-1 p0   AEAD(DEK_1, nonce_a)          │
  │ region B: topic-2 p3   AEAD(DEK_2, nonce_b)          │
  │ region C: topic-1 p7   AEAD(DEK_1, nonce_c)          │
  │ …                                                     │
  │ footer: per-region { key_id, wrapped_dek, nonce }    │
  └──────────────────────────────────────────────────────┘
```

**[Design] What this costs, stated honestly:**

- **No single streaming encrypt of the object.** Each region is sealed separately, so the writer holds per-region AEAD state. Bounded by the number of distinct topics in a flush, not by object size.
- **Nonce discipline per region.** A nonce reused under one DEK is catastrophic for GCM. Nonces must be constructed, never randomly drawn — see §5.
- **The footer and the coordinator index must carry key metadata** per region: key identifier, wrapped DEK, nonce. Small, but it widens the index entry.
- **Compaction must re-seal.** Merging regions across objects means decrypt-and-re-encrypt under the same topic key, so compaction workers need unwrap capability. This is the largest operational consequence and it is discussed in §6.
- **No cross-topic compression.** Already true — compression is per-batch — so nothing is lost here.

---

## 3. ⚠️ "Per topic" cannot mean "one KMS key per topic"

**[Documented]** AWS KMS allows **100,000 customer-managed keys per region** ([resource quotas](https://docs.aws.amazon.com/kms/latest/developerguide/resource-limits.html)). AWS-managed and AWS-owned keys do not count against it.

**[Assessment]** The scale target in [15](15-scale-architecture-position.md) is **1M–100M topics**. One KMS key per topic exceeds the quota by **one to three orders of magnitude**, and no quota increase closes that gap.

**So "per topic" must mean per-topic *configuration*, not per-topic *KMS key*:**

- A topic **names** the KEK it is encrypted under.
- **Many topics share one KEK** — in practice one KEK per customer, or per customer per environment.
- The number of distinct KEKs is bounded by *customers who opt into BYOK*, not by topic count.
- Topics that do not opt in use a provider-managed default key.
- Each topic still gets its **own DEK**, so cryptographic isolation between topics is preserved even when they share a KEK.

This is the standard envelope hierarchy and it is what makes per-topic isolation affordable: **DEKs are cheap and local, KEKs are scarce and remote.**

---

## 4. The portable KMS abstraction

**[Documented] AWS and GCP differ in a way that decides the trait's shape.**

| | AWS KMS | GCP Cloud KMS |
|---|---|---|
| Generate + wrap a DEK server-side | `GenerateDataKey` | **no equivalent** |
| Wrap a locally-generated DEK | `Encrypt` | `Encrypt` |
| Unwrap | `Decrypt` | `Decrypt` |

GCP's documented envelope pattern is explicit: **generate the DEK locally**, encrypt data with it, then wrap the DEK with the KEK. There is no `GenerateDataKey` and none is planned.

**[Design]** Therefore the seam is **wrap/unwrap**, with DEK generation local and provider-independent — the same choice Tink makes, where a KMS client supplies an AEAD used only to wrap:

```rust
/// In `oqueue-core`. Deliberately does not expose key generation:
/// DEKs are generated locally so the abstraction is the intersection
/// of what AWS and GCP both offer, not the union.
#[async_trait]
pub trait KeyProvider: Send + Sync + fmt::Debug {
    /// Wrap a locally-generated DEK under the named KEK.
    async fn wrap(&self, kek: &KeyId, dek: &Dek) -> Result<WrappedDek, KeyError>;

    /// Unwrap. The KEK is identified by the wrapped blob plus `KeyId`.
    async fn unwrap(&self, kek: &KeyId, wrapped: &WrappedDek) -> Result<Dek, KeyError>;
}
```

**[Assessment]** Two consequences worth stating:

- **Using the intersection rather than the union costs one AWS round trip's worth of convenience and buys genuine portability.** `GenerateDataKey` would have to be emulated on GCP anyway, so building on it would produce an abstraction that leaks.
- **Local DEK generation must use the FIPS DRBG in FIPS builds** (§7). This is the one place the two requirements interact directly.

Implementations: AWS KMS, GCP Cloud KMS, a static key for tests, and an in-memory fake. HashiCorp Vault and Azure Key Vault fit the same trait if wanted later.

---

## 5. KMS call amplification — the thing that actually breaks

**[Documented]** AWS KMS cryptographic operations share a request quota of **5,500–10,000 requests/second per account per region**, depending on region ([request quotas](https://docs.aws.amazon.com/kms/latest/developerguide/throttling.html)). All crypto operations draw on the same budget.

**[Assessment]** A broker flushing objects several times per second across many topics would exhaust that instantly if each flush wrapped a fresh DEK. **Uncached, this design does not work at all** — and unlike most scale problems it fails at modest load, not at the target.

**[Documented]** The proven mitigation is exactly S3's: **Bucket Keys** generate a short-lived intermediate key reused across objects within a time window, cutting KMS calls **by up to 99%**.

**[Design] The same shape, broker-side:**

- **One DEK per topic**, held in memory, **rotated on whichever comes first** — a time bound or a bytes-encrypted bound. Both are constants, not settings.
- The **wrapped DEK travels with the data** in the object footer, so a reader needs the KEK but never the writer.
- **Unwrapped DEKs are cached on read** with a TTL, keyed by wrapped-blob identity.
- **KMS is on the rotation path, never the per-batch path.** If a produce request can trigger a synchronous KMS call, the design is wrong.

⚠️ **A cached DEK is plaintext key material in process memory.** It must be zeroized on drop, excluded from every `Debug` implementation, and never reach a log, span, metric label, or error variant. This is the same discipline any secret gets, and it is worth a gate rather than a convention.

⚠️ **KMS availability becomes a dependency of the read path** for cold data whose DEK has aged out of cache. A KMS outage degrades reads for BYOK topics. That is inherent to BYOK — the customer holds the ability to revoke access, which is the point — but it must be stated, monitored, and reflected in the SLO rather than discovered.

---

## 6. Where this sits in the architecture

**[Design]**

- **`oqueue-core`** gains the `KeyProvider` trait seam, alongside `Clock` and `ObjectStore`. `#![forbid(unsafe_code)]` as before.
- **`oqueue-crypto`** (new crate) holds the AEAD, envelope logic, DEK cache, nonce construction, and zeroization. `#![forbid(unsafe_code)]` — the AEAD comes from a vetted implementation, not from us.
- **`oqueue-store`** stays unaware. It moves bytes; it does not know they are encrypted. This keeps the object-storage conformance suite ([19](19-workspace-engineering.md) §4.2) independent of encryption.
- Encryption sits **between the codec and the store**, and the sans-I/O rule holds: `oqueue-crypto` performs no I/O, since `KeyProvider` is injected.

**Nonce construction, not generation.** With per-region sealing there are many nonces under one DEK, and random 96-bit nonces have a birthday bound that is uncomfortable at this volume. Construct them deterministically — writer epoch ‖ object sequence ‖ region index — so reuse is structurally impossible rather than statistically unlikely. **This is the single highest-severity thing in the document to get wrong**, and it is exactly the kind of invariant that belongs in a type rather than a comment.

**Compaction** ([05](05-rust-ecosystem.md), M5) must unwrap, re-seal, and preserve topic→key association. Two consequences: compaction workers need KMS access for every topic they touch, and a revoked KEK blocks compaction of that topic's data — which needs a defined behaviour rather than a stuck queue.

---

## 7. FIPS as a separate build

**[Documented]** The Rust path is `aws-lc-rs` with its FIPS feature, which binds `aws-lc-fips-sys` to **AWS-LC-FIPS 4.x**, covered by **FIPS 140-3 certificate #4816**. `rustls` exposes a `fips` feature that selects this provider.

**[Documented] Build requirements: CMake, Go, and a C/C++ compiler**, plus `bindgen` for any target without pre-generated bindings.

**[Assessment] That requirement alone justifies the separate build**, independent of the compliance argument. [20](20-build-and-release-portability.md) §1 sets the rule that `cargo build` must need only cargo and a C compiler; a Go toolchain in the default build path violates it for every user who does not need FIPS. Keeping FIPS as a distinct artifact confines that cost to the people who asked for it.

**What FIPS constrains:**

| | Default build | FIPS build |
|---|---|---|
| Crypto provider | `aws-lc-rs` (or `ring`) | `aws-lc-rs` **FIPS**, cert #4816 |
| AEAD | AES-256-GCM or ChaCha20-Poly1305 | **AES-256-GCM only** — ChaCha20-Poly1305 is not FIPS-approved |
| DEK generation | any CSPRNG | the **approved DRBG** from the validated module |
| TLS cipher suites | full rustls set | FIPS-approved subset |
| Host build tools | cargo + C compiler | **+ CMake, + Go** |

**[Design] Requirements this places on the build and the runtime:**

- A **`fips` feature** that flips the provider, and a CI job that builds it. It is an artifact in [20](20-build-and-release-portability.md) §7's matrix, not a variant of an existing one.
- ⚠️ **The AEAD choice must be a runtime-visible property recorded with the data**, not a compile-time assumption. A FIPS build must be able to *read* data written by a non-FIPS build, which means the region header names its algorithm. Getting this wrong makes the two builds mutually unreadable and is discovered late.
- **A runtime assertion that the module is actually in FIPS mode**, exposed and checked by a gate. "We built with the feature on" is a claim; `fips_mode_enabled()` returning true is a fact — the same distinction as [21](21-ai-development-loop.md) §5.
- ⚠️ **FIPS complicates cross-compilation**, which interacts with [20](20-build-and-release-portability.md) §4's preference for native builds. Native runners for both Linux architectures remain the answer.

---

## 8. What this costs deployments that do not use it

**[Assessment]** The requirement that neither feature burdens the default:

- **No encryption configured** → no KMS dependency, no DEK cache, no per-region sealing. The region header still names an algorithm, and it says "none". Cost: a few bytes per region.
- **Non-FIPS build** → no Go, no CMake, no aws-lc-fips-sys, ordinary build times.
- **The `KeyProvider` seam exists regardless**, with a no-op implementation. A trait with a null implementation costs nothing at runtime and keeps the encrypted and unencrypted paths from diverging — which is how the unencrypted path stays tested.

---

## 9. Open questions

- **Key rotation semantics.** When a customer rotates a KEK, existing wrapped DEKs remain valid under the old KEK version. Both clouds keep old versions available for unwrap, but the policy — re-wrap eagerly, re-wrap during compaction, or never — is undecided and affects how long old key versions must be retained.
- **Revocation behaviour.** If a customer revokes a KEK, that topic's data becomes unreadable *by design*. What does the broker do — fail fetches with a specific error, stall compaction, or tombstone? Undefined, and it needs to be defined before anyone relies on revocation.
- **DEK rotation constants.** §5 requires a time bound and a bytes bound; neither has a value. Derive from the KMS request budget and the flush rate once those are known.
- **Does BYOK compose with compaction across tenants?** Compaction currently plans across objects; with per-topic keys it may need to plan within a key domain. Not worked through.
- **Is a FIPS-mode conformance suite needed?** The two builds must agree byte-for-byte on anything they both produce. A differential test across builds would catch divergence, but running two toolchains in CI is a real cost.
- **Per-topic key configuration and the metadata plane.** A topic's KEK reference is metadata, and [15](15-scale-architecture-position.md) requires metadata cost proportional to *active* partitions. A KEK reference per topic is small, but it is another per-topic field at 100M topics.

---

## Sources

[AWS KMS resource quotas](https://docs.aws.amazon.com/kms/latest/developerguide/resource-limits.html) (100,000 customer-managed keys per region) · [AWS KMS request quotas and throttling](https://docs.aws.amazon.com/kms/latest/developerguide/throttling.html) (5,500–10,000 crypto ops/sec) · [S3 Bucket Keys](https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucket-key.html) (up to 99% reduction in KMS calls) · [GCP Cloud KMS envelope encryption](https://docs.cloud.google.com/kms/docs/envelope-encryption) (generate DEKs locally; no `GenerateDataKey` equivalent) · [GCP client-side encryption with Tink](https://docs.cloud.google.com/kms/docs/client-side-encryption) (the portable KMS-client abstraction) · [aws-lc-rs](https://github.com/aws/aws-lc-rs) and its [requirements](https://aws.github.io/aws-lc-rs/requirements/index.html) (CMake, Go, C compiler) · [rustls FIPS manual](https://docs.rs/rustls/latest/rustls/manual/_06_fips/index.html) · [AWS-LC FIPS 140-3 certification](https://aws.amazon.com/blogs/security/aws-lc-is-now-fips-140-3-certified) (certificate #4816)
