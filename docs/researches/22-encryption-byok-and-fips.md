---
title: "Encryption: Per-Topic BYOK and FIPS Builds"
slug: encryption-byok-and-fips
status: working-position
last_updated: 2026-08-13
tags: [encryption, byok, kms, aws-kms, gcp-kms, envelope-encryption, fips, aws-lc-rs, rustls, compliance, multi-tenancy, dek, kek]
related: [15-scale-architecture-position, 12-object-discovery-and-api-cost, 20-build-and-release-portability, 04-object-storage-s3-gcs]
summary: >
  Two optional requirements: per-topic BYOK portable across AWS KMS and GCP
  Cloud KMS, and FIPS 140-3 as a separate build. BYOK applies to roughly 10,000
  topics out of 1M-100M, which is the number that decides the design: it is a
  rare, opt-in path, so BYOK data is segregated into its own objects by key
  domain rather than complicating the object format for everyone. SSE-KMS still
  cannot satisfy it, so broker-side envelope encryption is forced. The portable
  KMS abstraction is wrap/unwrap, not generate-data-key, because GCP has no
  GenerateDataKey equivalent.
---

# Encryption: Per-Topic BYOK and FIPS Builds

**⚠️ A design position**, in the sense of [15](15-scale-architecture-position.md). Requirements stated 2026-08-13; this document derives what they imply.

Marked **[Documented]**, **[Assessment]**, or **[Design]**.

---

## 0. The requirements

1. **BYOK** — customers supply their own key material, configured **per topic**, portable across **AWS KMS** and **GCP Cloud KMS**. Optional; the default is a provider-managed key. **Expected volume: ~10,000 topics**, against a total target of 1M–100M.
2. **FIPS 140-3** — available as a **separate build**. Optional; not the default artifact.

Both are opt-in. Neither may impose cost on deployments that do not use them.

**[Assessment] The 10,000 figure is the single most important number in this document.** BYOK covers on the order of **0.01%–1% of topics**. That makes it a *rare, opt-in, premium* path rather than a property of the system, and nearly every design decision below follows from it:

- **The object format does not change for everyone.** BYOK data is segregated rather than co-mingled (§2).
- **KMS quotas stop being a scaling problem** and become a rounding error (§3, §5).
- **BYOK topics may pay worse batching efficiency**, because they are few and it is the customer's choice (§2).

⚠️ Designing BYOK as though every topic used it would impose per-region sealing, wider index entries, and a DEK cache sized for millions on **100% of traffic to serve under 1%.** That is the mistake this section exists to prevent.

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

## 2. The collision, and the resolution: segregate, don't complicate

**[Assessment]** Per-topic keys and multi-topic batching are in direct conflict: records encrypted under different keys cannot share an encryption context, and an object bundles many topics precisely because that is what makes the cost model work.

There are two ways out, and **the 10,000-topic figure picks the second.**

**Option A — per-region sealing.** Keep co-mingling every topic and seal each region inside the object under its own topic's DEK. Workable, because the read path already issues ranged GETs into the object rather than reading it whole. But it changes the object format, the footer, and the index entry **for all traffic**, and it forbids a single streaming encrypt of an object forever after.

**Option B — segregate by key domain.** An object is **either** a default-key object (the overwhelmingly common case, unchanged from today) **or** a BYOK object containing only regions whose topics share one KEK. Two object kinds, one simple and one not.

**[Design] Option B, because BYOK is under 1% of topics.** Option A pays format complexity on 100% of objects to serve under 1% of them. Segregation confines the cost to the traffic that asked for it and leaves the hot path exactly as it is.

```
  default object  (the >99% path — unchanged)
  ┌──────────────────────────────────────────┐
  │ topic-1 p0 │ topic-2 p3 │ topic-7 p1 │ … │   one key or none
  └──────────────────────────────────────────┘

  BYOK object     (one customer's key domain)
  ┌──────────────────────────────────────────┐
  │ region: topic-A p0   AEAD(DEK_A, nonce)  │
  │ region: topic-B p2   AEAD(DEK_B, nonce)  │   topics A,B share KEK_cust
  │ footer: per-region { key_id, wrapped_dek, nonce, alg }
  └──────────────────────────────────────────┘
```

Regions inside a BYOK object are still sealed **per topic**, not per object — a customer's topics stay cryptographically isolated from each other. What segregation buys is that the *default* object never learns about any of it.

**[Design] What this costs, stated honestly:**

- ⚠️ **BYOK topics get worse batching efficiency.** They can only batch with other topics in the same key domain, so a customer with one low-volume BYOK topic produces smaller objects and pays more PUTs per byte. **This is the real price of BYOK and it should be stated to customers rather than absorbed.** The mitigation is a longer flush interval for BYOK topics — trading latency for cost, which is the customer's trade to make.
- **Compaction must re-seal** within a key domain: decrypt and re-encrypt under the same topic key, so compaction workers need unwrap capability for the topics they touch (§6).
- **Nonce discipline per region.** A nonce reused under one DEK is catastrophic for GCM. Nonces are constructed, never drawn at random — see §6.
- **Two object kinds to test**, and the conformance suite must cover both. Cheaper than one universally complicated kind.

---

## 3. KMS key count: not a constraint at this volume

**[Documented]** AWS KMS allows **100,000 customer-managed keys per region** ([resource quotas](https://docs.aws.amazon.com/kms/latest/developerguide/resource-limits.html)). AWS-managed and AWS-owned keys do not count against it.

**[Assessment]** At ~10,000 BYOK topics this ceiling is **not binding** — one KMS key per BYOK topic would fit inside it with an order of magnitude to spare. Two further facts widen the margin:

- **BYOK keys live in the customer's account, not ours.** That is what makes it *their* key. So the quota applies per customer account, and a customer with a handful of BYOK topics is nowhere near any limit.
- **KMS charges roughly $1/month per customer-managed key.** With keys in the customer's account, that cost is theirs and visible to them, which is the correct place for it.

⚠️ **This is worth recording because it would bind at a different volume.** Had BYOK applied to every topic at the 1M–100M target, one key per topic would exceed the quota by one to three orders of magnitude and no increase would close it. The design would then be forced into a strict hierarchy — one KEK per customer, many topics sharing it. **At 10,000 topics we get to choose instead of being forced**, and the recommendation is the hierarchy anyway:

- A topic **names** the KEK it is encrypted under; several topics may name the same one.
- Each topic still gets its **own DEK**, so topics stay cryptographically isolated even when sharing a KEK.
- Customers who want strict one-key-per-topic can have it, because the headroom exists.

**DEKs are cheap and local; KEKs are remote and metered.** That asymmetry, not the quota, is what shapes §5.

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

## 5. KMS call amplification — bounded, but only with caching

**[Documented]** AWS KMS cryptographic operations share a request quota of **5,500–10,000 requests/second per account per region**, depending on region ([request quotas](https://docs.aws.amazon.com/kms/latest/developerguide/throttling.html)). All crypto operations draw on the same budget.

**[Assessment] At 10,000 BYOK topics the budget is comfortable, but only because DEKs are cached.** The arithmetic is worth doing rather than assuming:

| | Uncached (KMS per flush) | Cached, hourly DEK rotation |
|---|---|---|
| KMS ops/sec | flush rate × BYOK topics — **thousands** | 10,000 ÷ 3,600 ≈ **3/sec** |
| Against a 5,500/sec quota | throttles | **~0.05%** |

So caching converts this from a hard blocker into a rounding error. ⚠️ **Without it the design fails at modest load**, not at the target — a flush-rate-driven KMS call per BYOK topic reaches the quota long before the topic count does.

**[Documented]** The proven mitigation is exactly S3's: **Bucket Keys** generate a short-lived intermediate key reused across objects within a time window, cutting KMS calls **by up to 99%**.

**[Design] The same shape, broker-side:**

- **One DEK per topic**, held in memory, **rotated on whichever comes first** — a time bound or a bytes-encrypted bound. Both are constants, not settings.
- **The cache is small.** 10,000 DEKs at 32 bytes is ~320 KB of key material. There is no eviction pressure and no need for a sophisticated policy; it is sized by the BYOK topic count, not by the catalog.
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
- **The writer routes by key domain** (§2): a topic with a KEK goes to a BYOK object, everything else to the default path. That routing decision belongs with the flush planner, and it is the only place the >99% path needs to know BYOK exists.
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

- **No BYOK on a topic** → its data goes into ordinary objects on the ordinary path, with no per-region sealing, no KMS dependency, and no DEK cache entry. Because BYOK objects are segregated (§2), **the >99% path is not merely cheap, it is unchanged**. The region header still names an algorithm and says "none" — a few bytes.
- **Non-FIPS build** → no Go, no CMake, no aws-lc-fips-sys, ordinary build times.
- **The `KeyProvider` seam exists regardless**, with a no-op implementation. A trait with a null implementation costs nothing at runtime and keeps the encrypted and unencrypted paths from diverging — which is how the unencrypted path stays tested.

---

## 9. Open questions

- **Key rotation semantics.** When a customer rotates a KEK, existing wrapped DEKs remain valid under the old KEK version. Both clouds keep old versions available for unwrap, but the policy — re-wrap eagerly, re-wrap during compaction, or never — is undecided and affects how long old key versions must be retained.
- **Revocation behaviour.** If a customer revokes a KEK, that topic's data becomes unreadable *by design*. What does the broker do — fail fetches with a specific error, stall compaction, or tombstone? Undefined, and it needs to be defined before anyone relies on revocation.
- **DEK rotation constants.** §5 requires a time bound and a bytes bound; neither has a value. The KMS budget is not the binding constraint at this volume, so they should be derived from the security argument — how much data may share a key — rather than from the quota.
- **How much does segregation cost a small BYOK customer?** §2 accepts worse batching for BYOK topics. Unquantified: at what topic volume does a BYOK object become small enough that PUT costs dominate, and is a longer flush interval sufficient compensation?
- **Should one-key-per-topic be offered?** §3 shows the headroom exists at 10,000 topics. Whether to expose it, or to require the KEK-per-customer hierarchy, is a product decision with a support cost.
- **Is a FIPS-mode conformance suite needed?** The two builds must agree byte-for-byte on anything they both produce. A differential test across builds would catch divergence, but running two toolchains in CI is a real cost.
- **Per-topic key configuration and the metadata plane.** A topic's KEK reference is metadata, and [15](15-scale-architecture-position.md) requires metadata cost proportional to *active* partitions. A KEK reference per topic is small, but it is another per-topic field at 100M topics.

---

## Sources

[AWS KMS resource quotas](https://docs.aws.amazon.com/kms/latest/developerguide/resource-limits.html) (100,000 customer-managed keys per region) · [AWS KMS request quotas and throttling](https://docs.aws.amazon.com/kms/latest/developerguide/throttling.html) (5,500–10,000 crypto ops/sec) · [S3 Bucket Keys](https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucket-key.html) (up to 99% reduction in KMS calls) · [GCP Cloud KMS envelope encryption](https://docs.cloud.google.com/kms/docs/envelope-encryption) (generate DEKs locally; no `GenerateDataKey` equivalent) · [GCP client-side encryption with Tink](https://docs.cloud.google.com/kms/docs/client-side-encryption) (the portable KMS-client abstraction) · [aws-lc-rs](https://github.com/aws/aws-lc-rs) and its [requirements](https://aws.github.io/aws-lc-rs/requirements/index.html) (CMake, Go, C compiler) · [rustls FIPS manual](https://docs.rs/rustls/latest/rustls/manual/_06_fips/index.html) · [AWS-LC FIPS 140-3 certification](https://aws.amazon.com/blogs/security/aws-lc-is-now-fips-140-3-certified) (certificate #4816)
