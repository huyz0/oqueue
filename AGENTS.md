# oqueue

A Kafka-protocol-compatible message broker in Rust that uses object storage
(S3/GCS) as its primary log storage. No local disk in the durability path, no
inter-broker replication, no partition leadership. The architecture pattern is
WarpStream's; the scale target is turbopuffer's.

This file is loaded into every session by every agent tool. It is deliberately
an index. Detail lives in the linked files, and the nearest `AGENTS.md` in the
crate you are working in adds context specific to that crate.

## Start here

- [docs/internal/product/mission.md](docs/internal/product/mission.md) what this is and what it must never do
- [docs/internal/product/architecture.md](docs/internal/product/architecture.md) crate map and dependency rules
- [docs/internal/product/roadmap.md](docs/internal/product/roadmap.md) milestones and their completion conditions
- [docs/internal/product/backlog.md](docs/internal/product/backlog.md) the current task list
- [docs/internal/product/decisions/](docs/internal/product/decisions/) architecture decision records

## The research corpus

[docs/researches/](docs/researches/) is 21 documents of cited background
compiled before any code existed: the reference architectures, object-storage
physics, the Kafka protocol, cost models, and this project's own engineering
standards. **It is upstream of the product docs, not parallel to them.** Read
[docs/researches/README.md](docs/researches/README.md) first; it maps the rest.

Two documents there are positions rather than research and carry our decisions:
[15](docs/researches/15-scale-architecture-position.md) (scale architecture) and
[21](docs/researches/21-ai-development-loop.md) (how this project is built).
[10](docs/researches/10-open-questions.md) is the decision log — check it before
assuming anything is settled.

## Standards

Rules that are always true. Read the one that covers what you are touching.

⚠️ **None of these exist yet — they are M-1.5.** Listed here as the plan, without
links, because a link to a file that is not there is a claim this repo does not
get to make. Until M-1.5 lands, the governing rules are the non-negotiables
below and the position documents in `docs/researches/`.

- `docs/internal/standards/behavior.md` how to work in this repo, read this first
- `docs/internal/standards/rust-style.md`
- `docs/internal/standards/error-handling.md`
- `docs/internal/standards/async-concurrency.md`
- `docs/internal/standards/testing.md`
- `docs/internal/standards/contracts.md`

Skills and scripts are in the same state: the directories exist, their contents
are M-1.13 and M-1.6 through M-1.12.

## Skills

Procedures, in [.agents/skills/](.agents/skills/), written to the Agent Skills
spec so they work in any tool that reads `SKILL.md`. Where a skill needs to run
something it calls a script in `scripts/`, never a tool-specific built-in.

## Non-negotiables

⚠️ **Bootstrap state, M-1.** In the project this system was ported from, six of
seven rules name the script that enforces them. **Here, none of them do yet** —
the scripts are M-1's own deliverable, and each rule below names the task that
will add its gate.

Until that task lands, the rule is a preference. That is precisely the state
this project exists not to be in, so treat the list below as M-1's checklist and
not as a description of a working system. A rule with no gate is a preference.

1. **One task equals one commit equals one change that leaves the tree green.**
   Split anything that cannot meet that. The commit subject starts with the
   backlog task ID. → `scripts/check-commit-msg.sh` (M-1.6)
2. **Never lower a threshold or delete a test to make a check pass.** Thresholds
   are constants no environment can move. → `scripts/check-drift.sh`,
   `scripts/check-tests-kept.sh` (M-1.7)
3. **Never claim a test passes without having run it.** **No script enforces
   this, and none can.** It is a rule about what you say, and nothing checks a
   claim against an intention. Every other rule rests on it: a green gate
   reported by someone who did not run it is worth less than no gate. It matters
   more here than in a human-written project, because no human reads the code.
4. **Every commit is reviewed by an agent that did not write it**, given the
   task and the diff but never the author's reasoning, and the verdict is bound
   to the staged diff by hash so the gate cannot be satisfied by claiming it.
   → `scripts/check-reviewed.sh` (M-1.9). See
   [docs/researches/21](docs/researches/21-ai-development-loop.md) §4–5.
5. **Business logic is sans-I/O.** No library crate names a concrete socket
   type, reads the real clock, or touches object storage outside the seams that
   exist to hold them. If it needs I/O to test, it is in the wrong layer.
   → `scripts/check-sans-io.sh` (M-1.8)
6. **Changing an `oqueue-core` trait means updating the trait, every fake, every
   implementation, and the ADR in one commit.** → `scripts/check-core-contract.sh`
   (M-1.10)
7. **`unsafe` lives in three crates only** — `oqueue-buf`, `oqueue-codec`,
   `oqueue-checksum` — and never in the async or concurrency layer. Everything
   else is `#![forbid(unsafe_code)]`. A fourth crate requires a recorded
   decision. → `scripts/check-unsafe.sh` (M-1.11). See
   [docs/researches/18](docs/researches/18-rust-performance-methodology.md) §5.7.

## Never

- Never push unless asked.
- Never commit a tree you know is broken, including "I will fix it in the next
  commit".
- Never widen scope silently. Doing more than the task asked is as much a
  problem as doing less, because it breaks the one-task-one-commit property.
- Never add a dependency that pulls in a C toolchain without recording why.
  Build portability is a design constraint here, not a packaging afterthought —
  see [docs/researches/20](docs/researches/20-build-and-release-portability.md) §1.
