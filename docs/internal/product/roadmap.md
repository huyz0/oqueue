# Roadmap

Every milestone carries a completion condition that is **a command, not a
judgement**. A milestone whose "done" cannot be expressed as an exit code cannot
be driven autonomously, so it does not belong here in that form.

Only the current milestone is decomposed in detail, in
[backlog.md](backlog.md). Future milestones stay as entries here until their
turn, because decomposing them early produces tasks that are wrong by the time
they are reached.

The design background is [docs/researches/](../../researches/README.md); this
file is the execution view.

## Status

| Milestone | Name | State |
| --- | --- | --- |
| M-1 | AI development system | in progress |
| M0 | Workspace, contracts, and quality gates | not started |
| M1 | Object store seam and conformance suite | not started |
| M2 | Kafka wire protocol: produce and fetch | not started |
| M3 | Coordinator: offset sequencing and the index | not started |
| M4 | Consumer groups | not started |
| M5 | Compaction and retention | not started |
| M6 | Recovery and failover | not started |
| M7 | Metadata sharding and scale | not started |
| M8 | Encryption: BYOK and the FIPS build | not started |

## M-1 — AI development system

Build the system that builds everything else: the standards, the gates, the
skills, and the loop. Nothing in this milestone is broker code.

It exists first because every later milestone is executed by an agent against
these rules, and a rule that arrives after the code it governs has already been
violated.

**Completion condition:**

```bash
scripts/gates/m-1-complete.sh
```

which asserts that every non-negotiable in `AGENTS.md` names a script that
exists and passes, that `tests/gates/negative.sh` proves each gate can fail,
and that no rule remains marked "not yet enforced" except rule 3, which cannot
be.

> **Requirements** live in [requirements.md](requirements.md) with stable IDs.
> Every milestone below serves specific FR/NFR entries, and a milestone serving
> none is unjustified. ⚠️ Several requirements are **UNDERIVED** — most
> importantly NFR-13 (aggregate throughput), which gates architectural decisions
> in M1 and M3.

## M0 — Workspace, contracts, and quality gates

The Cargo workspace, the eleven crates, `oqueue-core`'s trait seams with a fake
beside each, and the coverage/mutation/benchmark gates wired to constants.

**Completion condition:** `scripts/gates/m0-complete.sh` — the workspace builds
on both Linux targets, every `pub trait` in `oqueue-core` has a fake, layering
and sans-I/O gates pass, and the coverage constant holds.

## M1 — Object store seam and conformance suite

`ObjectStore` and its three implementations (in-memory, S3, GCS), plus the
backend-agnostic conformance suite. Includes the **madsim/`object_store`
feasibility spike** — deferred here from M-1 because it needs a repo and gates
to land properly, and its answer shapes `standards/testing.md`.

**Completion condition:** `scripts/gates/m1-complete.sh` — the conformance suite
passes against the in-memory fake and against MinIO, and records which backends
it has been run against. ⚠️ Real S3 is deferred; conditional-write behaviour
stays marked unverified until it runs. Open question #33.

## M8 — Encryption: BYOK and the FIPS build

The `KeyProvider` seam, envelope encryption with a DEK per topic, per-region
sealing inside shared objects, the DEK cache that keeps KMS off the per-batch
path, and the separate FIPS artifact.

Sequenced after the object format is stable, because per-region sealing changes
the footer and the index entry. ⚠️ But the **region header must name its AEAD
algorithm from M1 onward**, or a FIPS build and a non-FIPS build become mutually
unable to read each other's data — a cheap field now, an expensive migration
later.

**Completion condition:** `scripts/gates/m8-complete.sh` — round-trip against
AWS KMS and GCP Cloud KMS through the same seam, a FIPS build that asserts
`fips_mode_enabled()` at runtime, and a differential test proving FIPS and
non-FIPS builds read each other's data.

## M2–M7

Not decomposed. See [docs/researches/10](../../researches/10-open-questions.md)
for what remains undecided in each; several are blocked on requirements not yet
stated, most importantly the aggregate throughput target (#25).
