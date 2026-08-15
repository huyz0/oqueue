---
title: "Roadmap"
description: >
  Read when starting a milestone or checking whether one is done. Every milestone to v1, in execution order, each with a completion condition that is a command.
tags: [product, milestones, planning]
---

# Roadmap

Every milestone carries a completion condition that is **a command, not a
judgement**. A milestone whose "done" cannot be expressed as an exit code cannot
be driven autonomously, so it does not belong here in that form.

Each also carries a **kind**, because milestones are not all the same sort of
thing and the difference decides how one is verified. A `functional` milestone
is done when behaviour is observable through the wire protocol; a `build`
milestone when an artifact runs somewhere it did not before; a `non-functional`
one only when a number is measured, never when code is written.

| Kind | Verified by | Example |
|---|---|---|
| `crate delivery` | the crate exists, its seams have fakes, its tests pass in isolation | M0, M1 |
| `functional` | a Kafka client observes the behaviour | M2, M3, M4 |
| `feature` | a capability is configurable and provably scoped | M8, M9 |
| `build` | an artifact runs on a target it was not built on | M13 |
| `AI-native development support` | the loop can do something unattended it could not before | M-1, M10 |
| `non-functional` | a measured number meets a stated bound | M6, M7, M14, M15 |

⚠️ **A `non-functional` milestone cannot be completed by writing code.** Its
completion condition names a measurement, and until the measurement runs the
milestone is not done however finished the implementation looks. Three of these
depend on requirements that are still **UNDERIVED** — see the risk section.

## Plan versus backlog

[`milestones/`](milestones/) holds **the plans**: forward-looking, expected to
change, binding on nobody. [`backlog.md`](backlog.md) is **the decomposition**:
the current milestone only, authoritative, and what every gate reads. Opening a
milestone means re-deriving its tasks from its plan, not copying them. See
[`standards/sdd.md`](../standards/sdd.md) §Decomposition.

⚠️ **This file is neither.** It is the index, and one column of it is binding:
the `milestone` skill reads the completion condition from here and refuses to
start a milestone whose condition is not a command. Sequence, kind, and
depends-on are revisable as the plan is; **the completion-condition column is
not a plan and changing one is a decision.**

## Execution order

⚠️ **Numeric order is not execution order.** IDs M-1 through M8 predate this
plan and are kept stable because the corpus's decision log cites them — doc 10
#40 requires the region header to name its AEAD algorithm "from M1", and the
resolved-decisions entry repeats it. ⚠️ Doc 22's *region-header* requirement
names no milestone — it says only that "the region header names its algorithm",
and the timing is doc 10's. Doc 22 does cite **M5** elsewhere, for compaction
re-sealing, so renumbering would strand both the decision log and that line. M9 through M15 were added when planning end to end showed the
original ten did not reach a shippable v1. Sequence:

| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M-1](milestones/M-1.md) | AI development system | AI-native development support | — | see `backlog.md` ⚠️ | `scripts/gates/m-1-complete.sh` | complete |
| 2 | [M0](milestones/M0.md) | Workspace, contracts, and quality gates | crate delivery | M-1 | see `backlog.md` ⚠️ | `scripts/gates/m0-complete.sh` | in progress |
| 3 | [M1](milestones/M1.md) | Object store seam and conformance suite | crate delivery | M0 | 18 | `scripts/gates/m1-complete.sh` | not started |
| 4 | [M2](milestones/M2.md) | Kafka wire protocol: produce and fetch | functional | M0 | 18 | `scripts/gates/m2-complete.sh` | not started |
| 5 | [M3](milestones/M3.md) | Coordinator: offset sequencing and the index | functional | M1, M2 | 19 | `scripts/gates/m3-complete.sh` | not started |
| 6 | [M10](milestones/M10.md) | Deterministic simulation and fault injection | AI-native development support | M3 | 14 | `scripts/gates/m10-complete.sh` | not started |
| 7 | [M9](milestones/M9.md) | Authentication, authorization, tenant isolation | feature | M2, M3 | 16 | `scripts/gates/m9-complete.sh` | not started |
| 8 | [M4](milestones/M4.md) | Consumer groups | functional | M3, M9 | 17 | `scripts/gates/m4-complete.sh` | not started |
| 9 | [M11](milestones/M11.md) | Idempotent producers | functional | M3 | 12 | `scripts/gates/m11-complete.sh` | not started |
| 10 | [M5](milestones/M5.md) | Compaction and retention | functional | M3, M10 | 20 | `scripts/gates/m5-complete.sh` | not started |
| 11 | [M6](milestones/M6.md) | Recovery and failover | non-functional | M3, M10 | 16 | `scripts/gates/m6-complete.sh` | not started |
| 12 | [M7](milestones/M7.md) | Metadata sharding and scale | non-functional | M6, M9 | 17 | `scripts/gates/m7-complete.sh` | not started |
| 13 | [M8](milestones/M8.md) | Encryption: BYOK and the FIPS build | feature | M1, M5, M9 | 18 | `scripts/gates/m8-complete.sh` | not started |
| 14 | [M12](milestones/M12.md) | Admin API and operability | functional | M4, M9 | 16 | `scripts/gates/m12-complete.sh` | not started |
| 15 | [M13](milestones/M13.md) | Release engineering and the artifact matrix | build | M12 | 15 | `scripts/gates/m13-complete.sh` | not started |
| 16 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | M5, M13 | 16 | `scripts/gates/m14-complete.sh` | not started |
| 17 | [M15](milestones/M15.md) | v1 hardening: chaos, soak, and the release gate | non-functional | M14 | 15 | `scripts/gates/m15-complete.sh` | not started |

⚠️ **Only `m-1-complete.sh` is specified today**; the rest name the script each
milestone must produce, and what it must assert is written in that milestone's
plan. The `milestone` skill reads this column and refuses to start a milestone
whose condition is not a command, so the column is load-bearing rather than
decorative — an empty cell halts the loop.

⚠️ **Depends-on is not the same as the order.** M11 depends only on M3 and could
be pulled ahead of M4; it sits at slot 9 because idempotence is worth less than
consumer groups, not because it is blocked. The column says what is *possible*,
the number says what is *chosen*.

⚠️ **M-1 is well over the 20-task limit** `sdd.md` now states, and still
moving. It accumulated the planning work, the defects its own gates found,
and every `milestone-review` checkpoint so far has turned some of what it
found into new tasks — see `milestones/M-1.md`'s own "Notes for the boundary
review" section for the specifics, rather than a count duplicated here that
would need updating at the same rate that section already does. It is the
evidence for the rule, not an exception to it; every milestone after it is
planned under the cap and should be split rather than allowed to grow. This
row's Tasks cell deliberately carries no number — `docs/internal/product/backlog.md`'s
own row count is authoritative, and `scripts/milestone-review.sh coverage`
(not this table) is what a checkpoint should ask for the current commit
list. ⚠️ A fixed number here (first 44, then 48) went stale twice before this
paragraph stopped writing one down at all — the fourth recurrence of the
identical failure mode `milestones/M-1.md`'s own boundary-review section
tracks as **M-1.52**.

⚠️ **An open milestone's Tasks cell reads `see backlog.md`.** A number here is
a plan's estimate and stops being true the moment the milestone opens: M0's
count moved from 18 to 19 during the review of the very commit that decomposed
it, which is the same drift this paragraph's M-1 half records going stale twice
before the number was deleted. No gate compares this cell to `backlog.md`'s row
count, so the only cell that cannot be wrong is the one that carries no number.
A `not started` row keeps its estimate, because an estimate is all it has.

⚠️ **M-1 is `complete` with one row still `todo`, and that is not a
contradiction.** A milestone is done when its completion condition exits 0;
`m-1-complete.sh` names every non-negotiable in `AGENTS.md`, and none of them
depends on `M-1.12` — NFR-56's constant cannot be chosen without a workspace to
measure, so the row was blocked from the first day of the milestone rather than
left undone at the end of it. **M0.16 writes it.** Recording this here matters
because "the gate passed but a row is open" is exactly the shape a milestone
declared complete on a technicality would also have, and the difference is
whether the open row was structurally excluded or quietly skipped.

### Why this order

Three placements are not obvious and are the ones worth arguing about:

- **M10 (simulation) before M4, M5, and M6, not after.** Every milestone from
  M4 on asserts something about behaviour under concurrency and failure, and
  a fault-injection harness built afterwards can only retrofit tests to code
  already written. Doc 10 #32 calls this "the largest single testing investment
  the project needs"; investments are made before the return, not after.
- **M9 (authentication) before M7 (scale).** M7's load-bearing claim is
  principal-indexed metadata — a `Metadata` request costing O(topics this
  principal can see). That is unbuildable without a principal, so the
  security milestone is a hard prerequisite rather than a parallel track.
- **M8 (encryption) after the object format settles, but its header field
  lands in M1.** Per-region sealing changes the footer and the index entry, so
  the milestone is late. ⚠️ But the region header must name its AEAD algorithm
  from M1, or a FIPS and a non-FIPS build become mutually unable to read each
  other's data — a field now, a migration later. Doc 22; doc 10 #40.

## Requirement coverage

⚠️ **A requirement with no milestone is not scheduled; a milestone with no
requirement is not justified.** Both are findings for a milestone-boundary
review. ⚠️ **Only one direction is gated.** `scripts/check-requirements-trace.sh`
fails a milestone plan that names no requirement, an id it names that
`requirements.md` does not list, and — since the table below is checked
against the plans, not authored independently — this table disagreeing with
what a plan actually says. It does not fail a *requirement* that names no
milestone, so a new FR added to `requirements.md` is unscheduled silently.
Closing that needs the gate to read both files in the other direction too,
and it is not what M-1.24 asked for.

| Milestone | Serves |
|---|---|
| M-1 | NFR-50, NFR-54 |
| M0 | FR-44, FR-50, NFR-2, NFR-40, NFR-42, NFR-50, NFR-51, NFR-52, NFR-53, NFR-55, NFR-56 |
| M1 | FR-30, FR-31, NFR-30 |
| M2 | FR-1, FR-2, FR-3 |
| M3 | FR-10, FR-11, FR-12, FR-13, FR-32, NFR-2, NFR-3, NFR-21 |
| M10 | NFR-20 (the method), FR-51 (the method) |
| M9 | FR-4, FR-40, FR-44, NFR-12 |
| M4 | FR-20, FR-21, FR-22 |
| M11 | FR-14 |
| M5 | FR-33, FR-34, FR-35 |
| M6 | FR-51, NFR-20, NFR-22, NFR-44 |
| M7 | NFR-10, NFR-11, NFR-4 |
| M8 | FR-41, FR-42, FR-43, NFR-14, NFR-33 |
| M12 | FR-50, FR-52, FR-53 |
| M13 | NFR-40, NFR-41, NFR-42, NFR-43 |
| M14 | NFR-1, NFR-2, NFR-3, NFR-13, NFR-31, NFR-32 |
| M15 | NFR-20, NFR-23, FR-10, FR-51 |

**Deferred, with nothing scheduled:** FR-15 (transactions and exactly-once) is
explicitly post-v1 — doc 10 #5. It is the largest single gap between oqueue v1
and Kafka, and saying so is the point of listing it.

## Deferred into a later milestone

⚠️ Recorded here because a deferral that exists in nobody's plan is indistinguishable
from a decision nobody made. Each must appear in the receiving milestone's plan.

| Deferred | Into | Why, and what it shapes |
|---|---|---|
| The madsim / `object_store` feasibility spike | M1 | Deferred from M-1 because it needs a repo and gates to land properly. Its answer shapes **`standards/testing.md`** — and, per doc 10's resolved log, *not* `architecture.md` or the crate split: madsim swaps the runtime via `cfg` rather than changing crate structure. ⚠️ Doc 10 #32's open-list entry still says it touches "every crate that does async", which the resolved log supersedes — the same un-struck-entry inconsistency as #8. Take the resolved log |
| Verification against **real S3** | M1 | The conformance suite runs against the fake and MinIO now. ⚠️ **Conditional-write behaviour stays marked unverified until it runs against real S3** — doc 10 #33. If the fake and MinIO are both more permissive than S3, the result is an architectural error, not a test gap |
| The region header's `alg` field | M1 (from M8) | A few bytes now against a migration later; doc 10 #40 |
| The cross-tenant admin surface | M12 (from M9) | M9 forbids cross-tenant views going through `Metadata`, but `--list` over the whole catalog still needs a deliberate answer — pagination, prefix scoping, or refusal — ⚠️ **before someone finds it**. The constraint is set in M9; the answer is M12's |
| Key-domain-aware compaction planning | M8 (from M5) | Compaction must plan **within** a key domain, workers need unwrap capability for every topic they touch, and a revoked KEK blocks compaction of that topic's data. ⚠️ M5 must not close without either that path or a recorded deferral here; doc 10 #37, #39 |

## The decisions that gate this plan

⚠️ **Eleven decisions are unmade**, and each blocks the milestone that first
depends on it. The first five are architecture; the rest are constants and
strategies that are no less blocking for being smaller. They are listed here because a plan that hides them reads
as more settled than it is. Each becomes an ADR task inside its milestone, where
the alternatives are still live.

| # | Decision | Blocks | Background |
|---|---|---|---|
| 3 | `object_store` vs `opendal` vs provider SDKs | M1 | doc 05 §1 |
| 4 | Hand-roll `oqueue-codec` vs wrap `kafka-protocol` | M2 | doc 02 §6.4, doc 05 §2 |
| 1 | Offset sequencing: external store vs object-storage CAS vs local consensus | M3 | doc 06 §1 |
| 12 | Materialized-state engine (SQLite / redb / RocksDB / fjall / SlateDB) | M6 | doc 13 §6 |
| 14 | The enumeration fork: recovery scanner vs `PREPARED`→`COMMITTED` | M6 | doc 13 §7 |
| 9 | Metadata distribution: push tail deltas vs pull per fetch | M3 | doc 12 §4.4–4.5 |
| 11 | The bounded-staleness limit | M3 (set), M5 (consumed) | doc 12 §4.6 — it is the floor for M5's deletion delay |
| 5 | Idempotency strategy | M11 | doc 06 §6 — ⚠️ also decides how much of FR-15 stays reachable post-v1 |
| 15 | Target RTOs | M6 | doc 13 §2 — hot-standby and cold rebuild are separate numbers |
| 18 | Compaction cadence at high partition counts | M5 | doc 14 §7 — ~$720/day at 60 s vs ~$24/day at 30 min, at 100k partitions |
| 22 | Retention on idle partitions | M5 | doc 10 #22 — both candidates named, neither worked through |

⚠️ **#8 (index granularity) is *not* on this list, and the corpus is
inconsistent about it.** Doc 15 §7 and doc 10's resolved-decisions log both
record it decided — *coarse coordinator index plus an in-object footer index,
forced by the state arithmetic in doc 14 §3* — but doc 10's open list still
carries #8 un-struck, unlike #16, #19, #20 and #29 which were struck when
resolved. Treat it as **decided**, build M3 to doc 15 §7, and fix doc 10's list.
Re-opening it would need an argument against the state arithmetic, not a fresh
survey of doc 12 §6.3.

⚠️ **#1 and #14 are coupled**, and #7 (the low-latency tier) couples to both:
adopting ack-before-sequencing *requires* the recovery scanner. Deciding them
independently is how a project acquires an architecture nobody chose.

## The numbers that do not exist yet

⚠️ Four milestones have completion conditions that cannot currently be written,
because the requirement they check has no number. ⚠️ M0 is one of them and is
not a `non-functional` milestone — two of its gates are constants, so this is
not only a measurement problem.

| Requirement | Milestone | Blocked on |
|---|---|---|
| NFR-13 aggregate ingest throughput | M14 | doc 10 #25 — also gates #7 and shared-WAL sizing |
| NFR-22 recovery time objective | M6 | doc 10 #15 — hot-standby and cold rebuild are separate numbers |
| NFR-23 availability target | M15 | no stakeholder figure |
| NFR-55 coverage constant | M0 | needs a workspace to measure |
| NFR-56 pre-commit time budget | M0 | needs a workspace to measure |

**NFR-13 is the one that matters most.** It gates an architectural decision
(#7, the fast tier), not just a benchmark, so it is wanted well before M14.
⚠️ Do not invent one — an invented number becomes an unexamined constraint the
moment somebody designs against it.


## The milestones

⚠️ **These headings are parsed.** `scripts/milestone-review.sh context` extracts
`^## M<n> ` through the next heading to tell a cross-cutting reviewer what the
milestone was *for*. A rewrite that drops them leaves every outer-loop packet
saying "no roadmap section" — which this file did, for one commit, and which is
why the shape is written down rather than left to convention.

## M-1 — AI development system

Build the system that builds everything else: standards, gates, skills, and the
two review loops. Nothing here is broker code. It is first because every later
milestone is executed by an agent against these rules, and a rule that arrives
after the code it governs has already been violated. Plan: [M-1](milestones/M-1.md).

## M0 — Workspace, contracts, and quality gates

The Cargo workspace, the eleven crates, `oqueue-core`'s trait seams with a fake
beside each, and the coverage and mutation gates wired to constants.
⚠️ Two of those constants (NFR-55, NFR-56) can only be chosen here, because
choosing them needs a workspace to measure. ⚠️ **Benchmark gates are not among
them**, though this sentence said so until M0 was decomposed: M0 has no hot
path to benchmark, `check-hot-path-bench.sh` already exists from M-1.29, and
`performance.md` rule 18's table is filled in as later milestones build the
paths it names — `check-hot-path-bench.sh`'s `NOT_YET_BUILT` allowlist assigns
its rows across M2, M3 and later work, not to M2 alone — and measured by M14.

## M1 — Object store seam and conformance suite

`ObjectStore` and its three implementations (in-memory, S3, GCS), plus the
backend-agnostic conformance suite. Carries three deferrals: the madsim
feasibility spike, real-S3 verification, and the region header's `alg` field.
⚠️ Doc 12 §8.1 proposes keeping `list()` **off** the trait so that "never LIST
on the read path" becomes a compile-time property rather than NFR-30's runtime
gate. That is a proposal for M1's ADR, not a decision — `architecture.md`
records no such split, and decision #14's recovery-scanner branch would need
enumeration.

## M2 — Kafka wire protocol: produce and fetch

`oqueue-codec` and the broker's protocol layer: framing, headers, flexible
versions, RecordBatch v2, and the minimum API set for produce, fetch, metadata,
and version negotiation. ⚠️ CRC-32C is **Castagnoli**, and the obvious crate
implements the wrong polynomial with no compile error.

## M3 — Coordinator: offset sequencing and the index

Offsets that are monotonic and gap-free under concurrent producers, the
offset→object index, the metadata cache, and the high watermark. The
serialization point is the log append, not the flush, which is what lets many
writers PUT concurrently without coordinating.

## M10 — Deterministic simulation and fault injection

The harness that makes every later milestone's failure claims testable: seeded
schedules, injected latency and faults, and **pauses and partitions rather than
only kills**. Doc 13 §8 records that every metastable finding in the reference
system came from pauses.

## M9 — Authentication, authorization, and tenant isolation

Principals, per-principal scoping of every operation, and the forward index from
principal to topic set. ⚠️ Its load-bearing constraint is that no code path may
materialize the global topic list to answer a client request.

## M4 — Consumer groups

Join, sync, heartbeat, rebalance, and durable committed offsets. Classic
protocol for v1; the group model is built on the three-epoch shape so KIP-848 is
an addition rather than a rewrite.

## M11 — Idempotent producers

Producer IDs, sequence numbers, and duplicate detection. ⚠️ Not optional in
practice: librdkafka enables the idempotent path by default, so real clients
reach for it whether or not the application asked.

## M5 — Compaction and retention

Compaction triggered on read amplification, time- and size-based retention
including on partitions nobody writes to, and the GC safety inequality that
makes deletion safe against a stale reader.

## M6 — Recovery and failover

Coordinator restart, snapshots, hot-standby promotion, and the degraded modes.
⚠️ Its RTO numbers do not exist yet, and hot-standby failover and cold rebuild
are separate numbers that must be designed and measured separately.

## M7 — Metadata sharding and scale

Internal, rebalanceable metadata shards; topic creation that provisions nothing;
and the catalog/read-path-index split. ⚠️ The ceiling that binds first is the
Kafka protocol and its clients, not storage.

## M8 — Encryption: BYOK and the FIPS build

The `KeyProvider` seam, envelope encryption with a DEK per topic, BYOK data
segregated into its own objects by key domain, the DEK cache that keeps KMS off
the per-batch path, and a separate FIPS artifact. ⚠️ The seam is wrap/unwrap,
not generate-data-key, because GCP has no equivalent of the latter.

## M12 — Admin API and operability

Topic lifecycle and configuration through the Kafka `AdminClient`, plus the
metrics, logs, and traces sufficient to diagnose a stalled partition.

## M13 — Release engineering and the artifact matrix

One binary per `(os, arch)`, the glibc floor, and a build needing only cargo and
a C compiler. ⚠️ Both Linux architectures are first-class; macOS is a
development platform and not a release target.

## M14 — Performance and cost validation

Where the project's public claims are either measured or withdrawn: produce and
read latency, the API-cost model, and aggregate throughput. ⚠️ Blocked on NFR-13
having a number at all.

## M15 — v1 hardening: chaos, soak, and the release gate

Chaos under load, a soak long enough to expose slow leaks, and the release gate
that ties them together. ⚠️ No acknowledged record is ever lost is the
requirement everything else yields to, and this is where it is attacked rather
than asserted.

---

Per-milestone provisional tasks and full completion conditions are in
[`milestones/`](milestones/README.md). ⚠️ All seventeen have plans. The design background is
[`docs/researches/`](../../researches/README.md); this file is the execution
view.
