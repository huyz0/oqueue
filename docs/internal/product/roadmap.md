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
#40 requires the region header to name its AEAD algorithm from its first
commit, originally read as "from M1" and corrected in doc 10 itself
(2026-08-16) once `M1.7` found no region header exists for `M1` to add a field
to — `oqueue-store` stores opaque bytes and stays unaware of any object
structure. The timing lands on **M3** instead, the first milestone that
assembles a bundled object (multi-topic flush batching, FR-32); see the
deferred-into-a-later-milestone table below. ⚠️ Doc 22's *region-header*
requirement names no milestone — it says only that "the region header names
its algorithm", and the timing is doc 10's. Doc 22 does cite **M5** elsewhere,
for compaction re-sealing, so renumbering would strand both the decision log
and that line. M9 through M15 were added when planning end to end showed the
original ten did not reach a shippable v1. Sequence:

| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M-1](milestones/M-1.md) | AI development system | AI-native development support | — | see `backlog.md` ⚠️ | `scripts/gates/m-1-complete.sh` | complete |
| 2 | [M0](milestones/M0.md) | Workspace, contracts, and quality gates | crate delivery | M-1 | see `backlog.md` ⚠️ | `scripts/gates/m0-complete.sh` | complete |
| 3 | [M1](milestones/M1.md) | Object store seam and conformance suite | crate delivery | M0 | see `backlog.md` ⚠️ | `scripts/gates/m1-complete.sh` | complete |
| 4 | [M2](milestones/M2.md) | Kafka wire protocol: produce and fetch | functional | M0 | see `backlog.md` ⚠️ | `scripts/gates/m2-complete.sh` | complete |
| 5 | [M3](milestones/M3.md) | Coordinator: offset sequencing and the index | functional | M1, M2 | see `backlog.md` ⚠️ | `scripts/gates/m3-complete.sh` | in progress |
| 6 | [M10](milestones/M10.md) | Deterministic simulation and fault injection | AI-native development support | M3 | 14 | `scripts/gates/m10-complete.sh` | not started |
| 7 | [M9](milestones/M9.md) | Authentication, authorization, tenant isolation | feature | M2, M3 | 16 | `scripts/gates/m9-complete.sh` | not started |
| 8 | [M4](milestones/M4.md) | Consumer groups | functional | M3, M9 | 17 | `scripts/gates/m4-complete.sh` | not started |
| 9 | [M11](milestones/M11.md) | Idempotent producers | functional | M3 | 12 | `scripts/gates/m11-complete.sh` | not started |
| 10 | [M5](milestones/M5.md) | Compaction and retention | functional | M3, M10 | 20 | `scripts/gates/m5-complete.sh` | not started |
| 11 | [M6](milestones/M6.md) | Recovery and failover | non-functional | M3, M10 | 16 | `scripts/gates/m6-complete.sh` | not started |
| 12 | [M7](milestones/M7.md) | Metadata sharding and scale | non-functional | M6, M9 | 17 | `scripts/gates/m7-complete.sh` | not started |
| 13 | [M8](milestones/M8.md) | Encryption: BYOK and the FIPS build | feature | M3, M5, M9 | 18 | `scripts/gates/m8-complete.sh` | not started |
| 14 | [M12](milestones/M12.md) | Admin API and operability | functional | M4, M9 | 16 | `scripts/gates/m12-complete.sh` | not started |
| 15 | [M13](milestones/M13.md) | Release engineering and the artifact matrix | build | M12 | 15 | `scripts/gates/m13-complete.sh` | not started |
| 16 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | M5, M13 | 16 | `scripts/gates/m14-complete.sh` | not started |
| 17 | [M15](milestones/M15.md) | v1 hardening: chaos, soak, and the release gate | non-functional | M14 | 16 | `scripts/gates/m15-complete.sh` | not started |

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

⚠️ **M-1 was `complete` with one row still `todo`, and that was not a
contradiction.** ⚠️ It is no longer the case — `M0.16` measured NFR-56 and
closed `M-1.12`, so M-1's rows are all `done`. The reasoning is kept because it
is the general rule, not a fact about one row: A milestone is done when its completion condition exits 0;
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
  lands in M3.** Per-region sealing changes the footer and the index entry, so
  the milestone is late. ⚠️ But the region header must name its AEAD algorithm
  from the first commit that defines one — `M3`'s multi-topic flush batching,
  not `M1`'s object-store seam, which has no object format to carry the field
  (`M1.7` found this; corrected from an original "M1" reading of doc 10 #40) —
  or a FIPS and a non-FIPS build become mutually unable to read each
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
| M1 | FR-30, FR-31 (⚠️ built for three backends and verified against **two** — the fake and MinIO; verification against **any GCS at all**, emulator or real, and against real S3, is `M15`'s, deferred by `M1.44`), NFR-30 |
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
| M15 | NFR-20, NFR-23, FR-10, FR-51, FR-31 (the verification `M1.44` deferred here) |

**Deferred, with nothing scheduled:** FR-15 (transactions and exactly-once) is
explicitly post-v1 — doc 10 #5. It is the largest single gap between oqueue v1
and Kafka, and saying so is the point of listing it.

## Deferred into a later milestone

⚠️ Recorded here because a deferral that exists in nobody's plan is indistinguishable
from a decision nobody made. Each must appear in the receiving milestone's plan.

| Deferred | Into | Why, and what it shapes |
|---|---|---|
| The madsim / `object_store` feasibility spike | M1 — **discharged** by `M1.22` | Deferred from M-1 because it needs a repo and gates to land properly. ⚠️ **Answered by `M1.22` (2026-08-21), and the premise on this row was wrong.** It said madsim "swaps the runtime via `cfg` rather than changing crate structure", per doc 10's resolved log. `M1.22` found madsim needs `--cfg madsim` **and** a shim per I/O crate, and none exists for `reqwest`/`hyper`, which is what `object_store` reaches the network through — so there is no `cfg` swap available here at all. What the spike found instead: `object_store` exposes a public `HttpService`/`HttpConnector` seam above the socket, so a deterministic harness needs no runtime swap and no crate-structure change. ⚠️ **Read `standards/testing.md`'s "Deterministic simulation" section, not this row and not doc 10 #32** — the conclusion (it shapes `testing.md`, not `architecture.md` or the crate split) survives; the reasoning that reached it does not |
| **aarch64 coverage of `oqueue-store`** | M13 (from M1) | `M1.42` found `m0-complete.sh`'s aarch64 workspace check red since `M1.15`: `ADR-0012` chose `ring`, whose build script runs `cc` for the **target**, so the crate needs `aarch64-linux-gnu-gcc`. Bisected — passes at `a961ae7~1`, fails at `a961ae7`, eighteen commits red before anything read the gate. `oqueue-store` joins `bin/oqueue` in the conditional exclusion, for the reason already recorded there: NFR-42 budgets a C compiler, not a *cross* toolchain, and `portability.md` rule 9 builds release artifacts **natively**, so the cross check is a fast type-check and never what ships. ⚠️ **The residue is real and is why this row exists**: after `M1.42` the crate holding every backend, plus `object_store`, `reqwest` and `rustls` beneath it, is type-checked for aarch64 by **nothing** on a host without that toolchain — `gates.yml` passes `--target` nowhere and never invokes `m0-complete.sh`. So NFR-40's "CI matrix building and testing on both" is a claim no automation holds today. ⚠️ M13 because that is where the artifact matrix is promised and where the answer is cheap: `portability.md` rule 9's own rationale is that arm64 runners are free, so an arm64 job restores the coverage natively and settles doc 10 #37 (build)'s musl half in the same pass. Until then a reader should treat NFR-40 as verified for x86-64 and asserted for aarch64 |
| **Eleven rows M1 opened after its own decomposition** — `M1.40`, `M1.41`, `M1.47`-`M1.51`, `M1.53`-`M1.56` | M2 (from M1) — **discharged** by `M2.0`, each row `dissolved` with a pointer to the M2 row that received it | Discovered debt, not unfinished scope: every one was written *during* M1, after its decomposition — `389dc00` added `M1.0`-`M1.32` and nothing more. ⚠️ Not all by review: `M1.40` and `M1.51` were found doing the work of `M1.30` and `M1.34`, and `M1.48` by a second agent session writing to the same git index. M1 closes **44** `done`, 3 `dissolved` (`M1.4`, `M1.7`, `M1.18`), **11** `deferred` — the closing review added four after `M1.52` stated 7, which is the growth that row predicted (`M1.58`, opened and closed *after* the close to give the verdict's three unowned minors owners, made the table 45/3/11; `M2.0`'s flip makes it 45 `done`/14 `dissolved`/0 `deferred`; the close's own tally stands). ⚠️ **The precedent is `M0`**, which closed by handing its boundary-review findings to `M1`, where they became `M1.23`-`M1.32`; the only difference here is that these were filed as M1 rows when found rather than as M2 rows. `M2.0` pulls them in as M2 rows exactly as `M1.0` pulled M0's, and until it does they are `deferred` in `backlog.md` — not `done`. ⚠️ **`M2.0` must re-derive the set** rather than trust this list: M1's closing milestone review runs after the commit that wrote it, so more rows are likely and no hand-maintained list here will name them. ⚠️ **The last four were added by M1's closing review, exactly as this row predicted** — `M1.53` (a conditional write over 8 MiB fails permanently on GCS and succeeds on S3, reproduced), `M1.54` (the out-of-bounds range contract holds for the fake and neither real backend, and the conformance suite is blind to it), `M1.55` (`m1-complete.sh` claims a matrix-honesty check nothing performs), `M1.56` (`ADR-0008`'s retry-configuration premise is unwired). The set is **the rows marked `deferred`, plus any `todo`** — not every row that is not `done`, which would also catch the three `dissolved` ones whose obligations this table has already given to `M3` and `M5`. ⚠️ **And `M2.0` flips each row it took to `dissolved`** — the eleven named here plus anything a later review adds — with a pointer to the M2 row that received it: closing an M2 row cannot flip an M1 cell, so without that step they read open forever. ⚠️ ****Five** are decisions rather than patches** and should not be settled to close a milestone faster — ⚠️ `M1.53` and `M1.54` say so in their own last sentences, and reading them as patch work would mean raising GCS's `max_part_size` to silence a divergence, changing a chunk size chosen to satisfy a 256 KiB-multiple constraint: `M1.53` (is the conditional-write ceiling a separate limit from the part size), `M1.54` (does the trait's contract change, do the backends close the gap at a `HEAD` per ranged get, or does a capability flag carry it), `M1.40` (should a persistently `compiling` suite fail rather than skip), `M1.41` (should a standard select itself for review), `M1.49` (non-negotiable 2 says "never lower a threshold", and for three of the seven thresholds `m0-complete.sh` pins, raising is the weakening move). **Four** are gate and document work — `M1.47` (`ADR-0005` contradicting itself about the seam's own contract), `M1.51` (two pinned container images, each named twice, with nothing comparing either pair), `M1.55` (`m1-complete.sh` claims a matrix-honesty check nothing performs) and `M1.56` (`ADR-0008`'s retry-configuration premise, unwired). ⚠️ Five decisions plus two shape-first plus four gate-and-document is eleven; `M2.md` partitions them identically. ⚠️ The last two need their **shape** decided before anything is written, and their rows say so: `M1.48` (two agent sessions sharing one git index undetected — its row rejects the obvious `target/` lockfile) and `M1.50` (`clippy.toml`'s five unpinned thresholds on the commit path — `NFR_CONSTANTS` greps `NAME=` and cannot read `key = value` TOML) |
| Verification against **real S3** | M15 (was M1) | The conformance suite runs against the fake and MinIO now. ⚠️ **Conditional-write behaviour stays marked unverified until it runs against real S3** — doc 10 #33. If the fake and MinIO are both more permissive than S3, the result is an architectural error, not a test gap. ⚠️ **Retargeted by `M1.44`: this row said "Into: M1" and M1 is closing without it.** A deferral pointing into the milestone that is closing is not a deferral — the whole purpose of this table is that an obligation crossing a boundary keeps an owner, and this one silently stopped having one the moment M1's last row went `done`. M15 because a release gate that has never run the storage layer against the storage it claims to support is not a release gate; `M15.md` task 15 receives it |
| **GCS live verification** (emulator, then real GCS) | M15 (was M1.21) | `M1.17` shipped `GcsStore` fully built and T0-tested, and `ADR-0014` deferred the live half after measuring two emulators against the real client: `fake-gcs-server` routes the XML PUT to a JSON handler demanding an `uploadType`, and Google's `storage-testbench` stores the object but returns no ETag or generation header. Both confirmed against live containers rather than inferred. ⚠️ **`ADR-0014` deliberately declined this table**, reasoning that `M1.21` was "still `M1`'s own task" so the deferral crossed no boundary. `M1.21` then closed with `gcs` recorded `not-yet-run` in `baselines/conformance-matrix.txt`, and at that moment the deferral did cross a boundary — with nothing to move it here. ⚠️ So `GcsStore` has **never executed against anything**, and the risk is shaped by `s3.rs` and `gcs.rs` being ~90% duplicated: the `s3` column being green makes GCS look covered, while the 10% that differs — `ifGenerationMatch` preconditions and the resumable upload path — is exactly the untested part (⚠️ **not** `compose`: `ADR-0015` records `object_store` exposing it for GCS nowhere, so there is no implementation to test, and `M1.17`'s row retracted that mention once already). The matrix row is the honest record and stays `not-yet-run` until something runs; `M15.md` task 15 receives it, the same task as the real-S3 row above |
| **A durable metadata log** (doc 10 #12) | M6 (from M3) | `ADR-0020` point 5 deferred the engine — `SQLite`, `redb`, `RocksDB`, `fjall` or `SlateDB` — because doc 13 §6 asks for a benchmark that has not been run, and picking one without it "would be an invented number wearing an ADR's clothes". ⚠️ **The cost of that deferral was never recorded, and M3's checkpoint review found it** (`M3.19`): `FakeMetadataLog` is the only implementation in the tree, so M3's coordinator is durable in **object storage** and not in its own log. A restart finds an empty log, `Coordinator::open`'s non-empty-log guard is unreachable, and the allocator re-bases at `Offset::ZERO` over objects that already hold those offsets. ⚠️ **FR-10 is unaffected** — it says "durable in object storage" and nothing about the metadata log — so what was wrong was `M3.md`'s Goal and `ADR-0020`'s prose, both corrected. M6 because it already owns doc 10 #12, cold start from a snapshot, warm restart from `applied_upto`, and the recovery scanner; a durable log without the recovery around it is half a mechanism. `M6.md` task 7a receives it |
| **A flush must not span metadata shards** (`ADR-0020`) | M7 (from M3) | `ADR-0020` requires it and `M3.13`'s row said "**enforced here, not assumed**". ⚠️ **It is assumed**, and this row is that admission: an object bundles topics within one shard so that committing it stays *one* append to one log, and a bundle spanning two would be a non-atomic append to two — a crash between them leaves the object committed for some partitions and not others. `MetadataShardId` does not exist (it is M7's, with the `SessionWatermark` row below), so `BundleBuilder` has nothing to check a topic's shard against and no gate can see the violation. ⚠️ **It is unreachable today** — one implicit shard — and reachable the moment M7 introduces a second, at which point the flush composer `M3.14` builds has no shard parameter and nothing would catch it. `M7.md` task 17b receives it, beside the shard identity it needs |
| **Shard identity on a `SessionWatermark`** (`ADR-0020` point 1) | M7 (from M3) | `ADR-0020` point 1 asks two things of a watermark and `ADR-0023` (`M3.10`) settles one: it carries a `CoordinatorEpoch`, so a line carried across a **rebalance** is incomparable rather than falsely comparable. The other — *"a `CommitVersion` never travels without its shard identity, and is never compared across shards"* — is **not** discharged, because `MetadataShardId` is M7's and does not exist. ⚠️ **The gap is a safety one, not a tidying one.** `CoordinatorEpoch` is defined per *log* and a log is per shard, so two shards' first coordinators both sit at `ZERO`: a watermark from shard A read against shard B's cache finds the epochs **equal** and falls through to a version compare between two independent counters — the failure `ADR-0023` calls meaningless, arriving through the mechanism built to prevent it. Nothing can enforce it while there is one implicit shard, which is why no M3 row owns it; `M7.md` task 17a receives it |
| **Snapshot bootstrap for a follower's index** (doc 12 §4.4) | M6 (from M3) | `M3.9` built doc 12 §4.4's **delta** half — a push subscription, and a follower that re-bootstraps by paging the log — and not its snapshot half: *"fetch a compacted index snapshot at version V, then subscribe from V"*, which that section introduces specifically to avoid *"a thundering herd of full-index queries on rolling restarts"*. ⚠️ **So N agents restarting together produce N full metadata-log replays today**, at the moment a cluster is least able to serve them, and `M3.9`'s row is `done` with that stated rather than implied. M6 because the mechanism is already in its plan and would otherwise be built twice: `M6.md` task 8 is cold start via *"read log → newest `SnapshotCommitted` → **one GET** → replay"* and task 15 is chunked replay with bounded transaction size. A snapshot record is a metadata-log record and a compacted index is what M6's recovery needs anyway; building a second, follower-only snapshot format in M3 would be the fork |
| **Enforcing the index growth quota** (NFR-11) | M5 (from M3) | `M3.11` was to make index growth an *enforced, alarmed quota with an explicit degraded mode*. ⚠️ **Enforcement is not achievable at M3's index keying, and two attempts were blocked on review proving it.** Entries are per `(object, partition)` — doc 14 §3's ~4M/s row — so a ceiling has to be met by evicting, and eviction gives back range a rebuild **cannot restore**: replaying the log reproduces the same entry count and sheds the same entries again, so the error's own advice is a loop that cannot converge. ⚠️ **And nothing in M3 ever shrinks the count** — retention and compaction are `M5`'s too — so a ceiling is terminal the moment it is reached. ⚠️ **The cause is the row directly below this one** and they land together: at the coarse per-object keying (~400/s, ~16 KB/s) the index is four orders of magnitude smaller — doc 14 §3 prices ~4M/s and ~160 MB/s against ~400/s and ~16 KB/s and a bound is either unnecessary or cheap. `M3` ships the growth **measured** — `MemoryIndex::entries()` — so it is a fact somebody can read rather than one discovered by running out of memory, ⚠️ **NFR-11 is `M7`'s requirement, not M3's** — `roadmap.md`'s own coverage table says so, and `M3.11`'s row claimed it wrongly from `M3.0` — so what moves here is a mechanism rather than a requirement's owner: `M5.md` task 8a receives the enforcement, and M7 still verifies |
| **The coarse per-object index** (doc 15 §7, doc 10 #8) | M5 (from M3) | `M3.6` built the two-tier *shape* and its own doc comment says so: entries are still keyed per `(object, partition)`, which is doc 14 §3's **~4M/s, ~160 MB/s** row, while the per-object-only keying that table prices at **~400/s and ~16 KB/s** leaves partition→range to the object's footer. ⚠️ **Dropping the `ByteRange` on demotion cut what an entry costs and not how many there are**, and `M3.6`'s review corrected a first cut that claimed the four orders of magnitude for the narrower entry — the difference is *granularity*, not field width. Recorded here by `M3.8` because `M3.6`'s commit body was its only home and `next-task` reads the backlog: it is a change to how entries are keyed, so no M3 row owns it, and `M3.11`'s quota is the row that meets the symptom rather than the cause. M5 because `M3.md`'s own Risks section says the ~97 GB over a 7-day retention "is what forces M5's compaction to be a metadata mechanism as much as an I/O one" |
| `ADR-0008`'s `RetryConfig` wiring | M3 (from M2.10) — **discharged** by `M3.0`, received by `M3.13` | `M1.56`'s wire-or-record chose record: a sans-I/O `RetryPolicy` sits in `oqueue-core` with no caller, above `object_store`'s unconfigured vendor retry, and no double-retry exists precisely because nothing invokes it. The seam's first real attempt-loop caller — M3's composition — configures the vendor `RetryConfig` from the policy's constants, or supersedes ADR-0008's premise with its own ADR; `M3.md` task 19 carries it. Recorded here by `M2.11` so the assignment stops being the ownerless prose pointer `M1.44` named |
| The region header's `alg` field | M3 (from M8) — **discharged** by `M3.0`, received by `M3.13` | A few bytes now against a migration later; doc 10 #40. ⚠️ **Moved here from a first reading of "M1" — `M1.7` found `M1`'s object-store seam has no object format to carry the field**; `oqueue-store` stores opaque bytes (`architecture.md`'s Encryption section). `M3`'s multi-topic flush batching (FR-32) is the first milestone that assembles a bundled object, so it is the first commit with a region header to put the field in |
| A streaming multipart writer, sealed with a `Precondition` | M5 (from M3, which had it from M1) — `M3.0` received it, and `M3.12` re-targeted it with `ADR-0026`; `M5.md` task 6 receives it. ⚠️ **Deferred twice for two different reasons**, which is the part worth reading: M1 had no caller, and M3's promised caller turned out not to need it — `M3.13`'s flush knows its size when the write starts, and `ADR-0020` point 4 forbids a conditional write on the offset stream, so neither half of "streaming, sealed conditionally" has a consumer until compaction merges objects into an output of unknown size. `M3.12` re-checked the upstream gap against the pinned 0.14.1's vendored source as `ADR-0013` asked: `CompleteMultipartMode` still `pub(crate)`, public paths still passing `Overwrite`, `PutMultipartOptions` still without a `PutMode` | `M1.16` found two things: `object_store` 0.14.1 exposes no public way to condition a `CompleteMultipartUpload` (ADR-0013, tracked upstream as `apache/arrow-rs-object-store#289`), and `ObjectStore::put`'s signature — a complete in-memory `Vec<u8>` — cannot express "unknown final size" at all regardless of that gap. `M1.16` ships unconditional multipart for large payloads only. The actual streaming-writer-with-atomic-seal capability doc 04 §5 motivates multipart with needs a new seam capability, decided where a real caller exists — `M3`'s multi-topic flush batching (FR-32), the same milestone receiving the region-header field above and for the identical reason |
| A durability conformance case, and the `Capabilities` flag to carry it | M3 (from M1) — the flag and the explicit skip **discharged** by `M3.0`, received by `M3.15`; ⚠️ **guarantee 1's crash clause is not, and now points at M15** (`M15.md` task 15, added by `M3.15`) | `ADR-0005` guarantee 1 promised the fake's crash clause is skipped "explicitly rather than by omission", and `M1.37` measured the opposite: `Capabilities` has two flags (`conditional_writes`, `ranged_reads`), no case exercises `crash_after_put_before_ack`, and `tests/it/fake.rs` asserts `report.skipped.is_empty()` — skipped by omission. What `M1.37` recorded — "whoever writes the durability case owes a capability flag" — lived only in that `done` row's prose, the shape `M1.44` names: an obligation pointing at a closed row is owned by nobody, and M1's closing review found it in neither this table nor the backlog (minor `834802880b6a`; recorded here by `M1.58`). M3 because its completion condition already demands the fault-injection test killing between PUT and ack (FR-10), and the fake has modelled exactly that since `M1.8` — writing that test as a conformance case gated by a durability capability flag is the same work done where a backend that cannot run it must skip *explicitly*; `M3.md` task 20 receives it. ⚠️ **`M3.15` did it and found the framing above half wrong.** The capability is `injectable_ack_loss` — being able to be *told* to lose an acknowledgement — which only a fake has, so the **fake runs** the case and **`S3Store` skips** it, the opposite of what "the fake's crash clause is skipped" predicted. And the case is `ADR-0005` guarantee **2**'s (a failed put is not proof of absence), not guarantee 1's crash clause: ⚠️ **that one is not M3's and is now named in `M15.md` task 15** rather than only here, because `Ok` surviving a process death is not something any single-process fake can show — `ADR-0005`'s own Consequences say so. What is discharged is the flag and the explicit skip; what is not is the crash clause, and ⚠️ **it was owned by nobody until this row's `Into` column stopped saying only "discharged"** — the very shape `M1.44` recorded two sentences above |
| `Dispatcher::dispatch` heap-allocates a `client_id` it drops, on every request | M14 (from M2) | `M2.30`'s review found it: the header decode needs `client_id` only for the byte offset, but the decoded `RequestHeader` owns a `String` for it that no caller reads. Recorded in the commit body per rule 15, never harvested into a row — `M2`'s closing milestone review found the gap. A dedicated pass belongs where a benchmark exists to measure it against, which `M14`'s performance work is the first milestone to build |
| Multipart's true per-request cost in `Operation`/`CountingObjectStore` | M14 (from M1) | `M1.14`'s own comment assumed `M1.16` would just add an `Operation` variant for `UploadPart`; found wrong writing `M1.16` — `CountingObjectStore` decorates the `ObjectStore` **trait**, so a multipart `put` is one counted call from that vantage point regardless of how many real HTTP requests the backend issued underneath it. Counting the true cost needs either a backend reporting it back through the trait (a contract change) or accounting living inside each backend — bigger than "add a variant," and belongs with `M14`'s API-cost model (NFR-31), the milestone that already turns this into a bound — `M14.md` task 5 |
| Server-side `copy_range` (S3 `UploadPartCopy`, GCS `compose`) | M5 (from M1) | `M1.18` found `object_store` exposes this for **neither** backend — S3's support is `pub(crate)`-only and whole-object-only even internally; GCS `compose` does not exist in the crate at all (ADR-0015, tracked upstream as `apache/arrow-rs-object-store#121`, open since 2023-10-20). No partial version is buildable through the crate `ADR-0008` chose; the only path is hand-rolled signed requests, the same infrastructure ADR-0013 already declined to build for a narrower problem. `M5.md` task 7 already names this capability and is the one real consumer, so `M1.18` dissolves into it rather than building unearned infrastructure ahead of a caller |
| `security.md` rule 5's `scripts/fuzz.sh` | M2 — **discharged** by `M2.26` (`scripts/fuzz.sh` runs a target per codec decoder module — frame, varint, batch, records, compress — seeded from the captured librdkafka corpus, refuses a codec module with neither target nor recorded reason, and `.github/workflows/fuzz.yml` runs it nightly). ⚠️ The end-to-end request-decoder target was `M2.27`'s: adding it found a real single-packet allocation DoS, and `M2.28`-`M2.34`'s rewrite (`ADR-0019`) was the fix, not a bound on the dependency — the reproducer now replays in 0 ms | Named by a standard, written by nobody, and scheduled nowhere until M0's checkpoint review found it. ⚠️ **The rule reads as enforced and is not.** It belongs with a decoder rather than with M0: `testing.md` rule 24 puts a fuzz target on every decoder, and the first decoder is M2's |
| **Group commit in `CoordinatorLoop::run`** | M6 (from M3, via `M3.18`) | Draining N queued requests into one `MetadataLog::append` — the slice signature is already there. ⚠️ **The number it optimizes against does not exist yet.** `FakeMetadataLog` is the only implementation in the tree and its `append` is in-memory, so the NFR-1 argument — a durable engine at single-digit-millisecond commits turns `COMMIT_QUEUE_DEPTH` into seconds of queueing — cannot be measured against anything today. `ADR-0020` point 5 declined to pick the engine for exactly that reason, and building the optimization before the thing it optimizes is the same invented number wearing different clothes. M6 because it already receives doc 10 #12's engine at `M6.md` task 7a; the group commit lands beside it, where a real `append` latency can be measured. `M6.md` task 7b receives it |
| **`CoordinatorLoop::rebuild` off the ack path** | M6 (from M3, via `M3.18`) | An unbounded full-log replay awaited **inside `serve`**, before the ack and inside the single serialization point: a dropped cache on a long log blocks every queued producer for the length of the replay, and a *failed* rebuild leaves `applied_upto` at `None` so the next commit replays again. ⚠️ **Nothing opens that door today** — `drop_cache` has no production caller, and the eviction that would have called it went to M5 with `M3.11`'s quota. So M3 would be building half of `M6.md` task 15's mechanism (chunked replay, bounded transaction size, periodic durable progress) against a caller that does not exist. `M6.md` task 15 receives it |
| **A dropped cache has no refill trigger but a commit** | M5 (from M3, `M3.23`'s minor) | A reader whose index was dropped is refilled only by the next commit through the coordinator, so a partition nobody is writing to stays cold. ⚠️ **Same shape as the row above and the same reason it is not M3's**: the only thing that would drop a cache is the quota `M3.11` proved unenforceable at M3's keying, which `M5.md` task 8a already receives. A trigger for an event nothing raises is a mechanism with no way to be tested. `M5.md` task 8a receives it, beside the quota |
| **A benchmark harness, and the five hot paths that need one** | M14 (from M2 and M3, via `M3.18`) | `performance.md` rule 18 says a hot path's benchmark arrives *with* the code, and `check-hot-path-bench.sh` allowlists eight rows. `M3.35` fixes the gate — the expiry mechanism, and the `find_batches` micro-benchmark rule 18 pins to `M3.8`'s own code. ⚠️ **The other five need something that has never existed here**: `crates/*/benches` is empty, no `criterion`/`divan`/`gungraun` dependency is in the tree, and `scripts/bench.sh`'s own header says its `cargo bench` invocation has never run against real code. Choosing the harness is a toolchain decision with `portability.md` and `build.md` consequences (a valgrind-based runner is not a dev-dependency, it is a platform requirement), which is not something a sweep row decides in passing. M14 is *Performance and cost validation* and already owns NFR-1/2/3; the five are RecordBatch encode/decode, CRC-32C over representative sizes, varint decode, the produce path end to end, and fetch tail-versus-cold. `M14.md` task 6a receives them, beside task 9's harness ADR |
| **The index refresh a 404 is supposed to trigger** | M7 (from M3, recorded by `M3.25`) | Doc 12 §4.6 asks a reader that finds an object the index named and the store does not have to refresh the index through a coordinator round trip *before* answering `OFFSET_OUT_OF_RANGE`. ⚠️ **`M3.22` shipped the answer without the round trip and argued why**: in a single-node broker the index a fetch reads is the coordinator's own, in-process, and nothing in M3 removes an entry from it, so a second read consults provably identical state and pays a second GET for it. ⚠️ **Recorded here because `read_or_refresh`'s own rustdoc said "`roadmap.md` carries it" and it did not** — an obligation whose only home was a plan paragraph, which `sdd.md` calls a working hypothesis expected to change silently. `M7.md` task 17b's closing paragraph receives it, where a follower's index *is* a cache and its 404 is genuinely ambiguous |
| **A parked fetch and the requests behind it on one connection** | M7 (from M3, recorded by `M3.25`) | ⚠️ **A judgement made while re-planning, not a numbered finding** — `M3.25`'s verdict artifact does not record it, and saying so is the point: a deferral whose stated source does not hold it is indistinguishable from an invented one. `MAX_PARK_MS` bounds how long one fetch parks; it does not bound what a *pipelined* connection sees, because the handler answers a connection's requests in order. ⚠️ **This may not be a defect** — Kafka's own broker orders responses per connection, and clients that care use separate connections — which is exactly why it is a deferral with a receiver rather than a row: what it needs is a measurement against a real client's socket timeout and `max.in.flight`, not a patch. M7 owns the connection and scale work and is where that measurement is affordable; `M7.md` task 17c receives it |
| `security.md` rules 6-7's `scripts/check-secrets.sh` | M8 | Same shape, same review. It needs secrets to check, and the first key material is M8's. ⚠️ Until it exists, rule 7 is held by review alone — and `M0.6` already had to shape a carve-out around its absence |
| **How a `minor` review finding gets scheduled** | M2 — **discharged** by `M2.1` (ADR-0016: the milestone-boundary review harvests commit-body minors into sweep rows) | ⚠️ `review.md` rule 15 sends a minor to the commit body and rule 16 just below it calls a finding living where nothing reads it one nobody will act on. M0's second half recorded upwards of thirty that way and none became a row until `M0.27`-`M0.29` harvested them by hand. The fix is a **procedure** — some step that harvests commit bodies at a milestone boundary — and choosing one is a decision for whoever owns the loop, not a repair a review may make; M0's boundary review recorded it as `bcf5d6f697f2` rather than re-specifying. ⚠️ **M2 because that is the first milestone whose minors will be about protocol code rather than about prose**, and because the interim rule that rule 15 now states — write the row in the *next* commit — is a workaround that removes the pressure to decide, so a date matters |
| The cross-tenant admin surface | M12 (from M9) | M9 forbids cross-tenant views going through `Metadata`, but `--list` over the whole catalog still needs a deliberate answer — pagination, prefix scoping, or refusal — ⚠️ **before someone finds it**. The constraint is set in M9; the answer is M12's |
| Key-domain-aware compaction planning | M8 (from M5) | Compaction must plan **within** a key domain, workers need unwrap capability for every topic they touch, and a revoked KEK blocks compaction of that topic's data. ⚠️ M5 must not close without either that path or a recorded deferral here; doc 10 #37 (encryption), #39 |

## The decisions that gate this plan

⚠️ **Eleven decisions were unmade** when this table was written; **three are
now resolved** — #9 without an ADR (doc 12 §4.4 already answers it), #1 by
`ADR-0020`, and #11 by `ADR-0021` — and the rest still block the milestone
that first depends on them. The first five
are architecture; the rest are constants and strategies that are no less
blocking for being smaller. They are listed here because a plan that hides
them reads as more settled than it is. Each becomes an ADR task inside its
milestone, where the alternatives are still live. ⚠️ This table is not swept
clean when a milestone besides the one that struck its own rows closes — #3
and #4 were resolved by `M1.1`/`M2.2` and are left unstruck here, a
pre-existing gap this edit does not attempt to close.

| # | Decision | Blocks | Background |
|---|---|---|---|
| 3 | `object_store` vs `opendal` vs provider SDKs | M1 | doc 05 §1 |
| 4 | Hand-roll `oqueue-codec` vs wrap `kafka-protocol` | M2 | doc 02 §6.4, doc 05 §2 |
| 1 | ~~Offset sequencing: external store vs object-storage CAS vs local consensus~~ | M3 | **resolved 2026-08-23, `ADR-0020`** |
| 12 | Materialized-state engine (SQLite / redb / RocksDB / fjall / SlateDB) | M6 | doc 13 §6 |
| 14 | The enumeration fork: recovery scanner vs `PREPARED`→`COMMITTED` | M6 | doc 13 §7 |
| 9 | ~~Metadata distribution: push tail deltas vs pull per fetch~~ | M3 | **resolved — doc 12 §4.4's own answer, no ADR needed; `M3.9`** |
| 11 | ~~The bounded-staleness limit~~ | M3 (set), M5 (consumed) | **resolved 2026-08-23, `ADR-0021`: 5 s** — sets the floor for M5's deletion delay |
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

⚠️ **#1 and #14 were coupled**, and #7 (the low-latency tier) couples to both:
adopting ack-before-sequencing *requires* the recovery scanner. `ADR-0020`
resolves #1 without tripping this coupling — it keeps the ordinary
assign-journal-ack sequence (`M3.7`: no offset is externally visible before
its log record commits), not WarpStream's Lightning-Topics-style
ack-before-sequencing, so #14 stays open and un-forced, M6's to decide when it
exists. Deciding #1 and #14 independently *would* be how a project acquires an
architecture nobody chose; deciding #1 in a way that does not need #14 is not
the same hazard.

## The numbers that do not exist yet

⚠️ Three milestones have completion conditions that cannot currently be written,
because the requirement they check has no number.

⚠️ **M0 was the fourth and no longer is.** Both of its gate constants are
measured: NFR-55's per-crate coverage floor at 85% (`M0.15`, lowest crate
carrying logic 91.63%) and NFR-56's pre-commit budget at 10 s (`M0.16`, suite
measured at 2.27 s across 14 hooks — 15 once the budget gate itself joined them, 16 next, and **17 today** at the `pre-commit` stage (`M1.21`) — `m0-complete.sh` asserts this sentence's number too, at the boundary). ⚠️ Both are **floors** — the workspace they
were measured on compiles no async runtime and no cloud SDK, which `M1` changes
— and neither is resolved by raising the literal when it is first breached.

| Requirement | Milestone | Blocked on |
|---|---|---|
| NFR-13 aggregate ingest throughput | M14 | doc 10 #25 — also gates #7 and shared-WAL sizing |
| NFR-22 recovery time objective | M6 | doc 10 #15 — hot-standby and cold rebuild are separate numbers |
| NFR-23 availability target | M15 | no stakeholder figure |

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
choosing them needs a workspace to measure. ⚠️ **Both are now chosen**: `M0.15`
measured line coverage per crate and set the floor at 85% (lowest crate carrying
logic: 91.63%), and `M0.16` measured the pre-commit suite at 2.27 s across 14
hooks and set the budget at 10 s. ⚠️ Both are **floors** — measured on a
workspace compiling no async runtime, which `M1` changes. ⚠️ **Benchmark gates are not among
them**, though this sentence said so until M0 was decomposed: M0 has no hot
path to benchmark, `check-hot-path-bench.sh` already exists from M-1.29, and
`performance.md` rule 18's table is filled in as later milestones build the
paths it names — `check-hot-path-bench.sh`'s `NOT_YET_BUILT` allowlist assigns
its rows across M2, M3 and later work, not to M2 alone — and measured by M14.

## M1 — Object store seam and conformance suite

`ObjectStore` and its three implementations (in-memory — in `oqueue-core`,
not `oqueue-store`, per `M1.37` — plus S3 and GCS), plus the
backend-agnostic conformance suite. ⚠️ **Carried two deferrals, discharged
*one*, moved the other, and hands a third set on**: the madsim spike was
answered by `M1.22` and is marked discharged below; real-S3 verification was
**not** discharged — `M1.44` retargeted it to `M15`, joined there by GCS live
verification, which is why `baselines/conformance-matrix.txt` still records
`s3-real` and `gcs` as `not-yet-run` and that is current rather than stale;
and `M1.52` defers to `M2` the rows M1's own work opened — seven when it was written, **eleven** after `M1.57` recorded what M1's closing review then found, two of them real defects rather than documentation. All are rows in
the table below. The original two: the madsim spike was discharged
by `M1.22`, and real-S3 verification — joined by GCS live verification, which
`ADR-0014` had pointed at `M1.21` and `M1.21` closed without doing — was
retargeted to `M15` by `M1.44`, since a deferral pointing into the milestone
that is closing has no owner at all. Both are rows in the
deferred-into-a-later-milestone table below; `M15.md` task 15 receives them. ⚠️ **Not** the region header's
`alg` field — `M1.7` found it belongs to `M3` instead, the first milestone
with an object format to carry it; see the deferred-into-a-later-milestone
table.
⚠️ Doc 12 §8 proposes keeping `list()` **off** the trait so that "never LIST
on the read path" becomes a compile-time property rather than NFR-30's runtime
gate. That is a proposal for M1's ADR, not a decision — `architecture.md`
records no such split, and decision #14's recovery-scanner branch would need
enumeration.

## M2 — Kafka wire protocol: produce and fetch

`oqueue-codec` and the broker's protocol layer: framing, headers, flexible
versions, RecordBatch v2, and the minimum API set for produce, fetch, metadata,
and version negotiation. ⚠️ CRC-32C is **Castagnoli**, and the obvious crate
implements the wrong polynomial with no compile error.

⚠️ **Mid-milestone, M2 reversed its message-layer decision.** The first pass
(`M2.12`-`M2.26`) built the protocol on `kafka-protocol` per `ADR-0017`, with
real-client round trips, a golden byte corpus, an FR-2 matrix, and a fuzz
harness. The fuzz harness then found an unbounded-allocation DoS the generated
decoder cannot be made not to have (a 64-byte frame demands ~30 GB), and M2's
milestone review ranked it blocking. `ADR-0019` supersedes `ADR-0017`: `oqueue`
now **owns its protocol codec**, hand-rolled for perf and security with
validate-before-allocate throughout, and `kafka-protocol` becomes a test-only
differential oracle beside the corpus. The rewrite (`M2.27`-`M2.34`) is the
milestone's second and larger half; the oracle the first half built is what
makes hand-rolling safe now when `ADR-0017` judged it too risky without one.

## M3 — Coordinator: offset sequencing and the index

Offsets that are monotonic and gap-free under concurrent producers, the
offset→object index, the metadata cache, and the high watermark. The
serialization point is the log append, not the flush, which is what lets many
writers PUT concurrently without coordinating. ⚠️ **Carried two deferrals**, and carries one:
the region header's `alg` field, moved here from an original "M1" reading of
doc 10 #40 — `M1.7` found `M1`'s object-store seam has no object format to
carry it, and M3's multi-topic flush batching (FR-32) is the first milestone
that assembles a bundled object, so its first commit is where the field must
land — and ~~a streaming multipart writer sealed with a `Precondition`, moved
here from `M1.16` (ADR-0013)~~. ⚠️ **That second one moved on to `M5`**
(`M3.12`, `ADR-0026`): `object_store` still exposes no public way to condition
`CompleteMultipartUpload` — re-checked against the pinned 0.14.1's vendored
source — but M3's promised caller turned out to need neither half of the
capability. `M3.13`'s flush knows its size when the write starts, and
`ADR-0020` point 4 forbids a conditional write on the offset stream, so the
first genuine caller is compaction's merge output; `M5.md` task 6 receives it.
So the streaming writer is **not** among the deferrals M3 discharges in its own commits; what remains of them is the region-header `alg` field and `ADR-0008`'s `RetryConfig` wiring, both received by `M3.13`, and the durability conformance case, received by `M3.15`.

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
makes deletion safe against a stale reader. ⚠️ **Carries one deferral**:
server-side `copy_range`, moved here from `M1.18` (ADR-0015) — `object_store`
exposes it for neither S3 nor GCS, so this is the first milestone that
decides how (or whether) to build it, with an actual caller in hand.

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
