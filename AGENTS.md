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
- [docs/internal/product/requirements.md](docs/internal/product/requirements.md) functional and non-functional requirements, with the IDs specs cite
- [docs/internal/product/architecture.md](docs/internal/product/architecture.md) crate map and dependency rules
- [docs/internal/product/roadmap.md](docs/internal/product/roadmap.md) every milestone to v1, in execution order, each with a completion condition
- [docs/internal/product/milestones/](docs/internal/product/milestones/README.md) one plan per milestone — ⚠️ a *plan*, not a decomposition
- [docs/internal/product/backlog.md](docs/internal/product/backlog.md) the current task list — authoritative, current milestone only
- [docs/internal/product/decisions/](docs/internal/product/decisions/) architecture decision records

## The research corpus

[docs/researches/](docs/researches/) is 22 documents of cited background
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

Four families, all binding. Each rule names its gate, or is marked as having
none — the second kind matters more, because it is where judgement is still
required.

<!-- index:standards:start -->
| Family | Standard | Read when |
|---|---|---|
| Process | [git.md](docs/internal/standards/git.md) | Read before committing, when unsure whether a change is one commit or several, or before amending anything already pushed. |
| Process | [review.md](docs/internal/standards/review.md) | Read when writing a review prompt, deciding whether a finding blocks a commit, or wondering why the reviewer was not given the author's reasoning. |
| Process | [sdd.md](docs/internal/standards/sdd.md) | Read when specifying work, decomposing a milestone, writing acceptance criteria, or when a spec turns out to be wrong. |
| Quality | [performance.md](docs/internal/standards/performance.md) | Read before optimizing, when adding or changing a benchmark, when a change touches a hot path, or when profiling. |
| Quality | [security.md](docs/internal/standards/security.md) | Read when touching the wire protocol, anything parsing untrusted input, secrets, key material, tenant isolation, or unsafe. |
| Quality | [testing.md](docs/internal/standards/testing.md) | Read when writing any test, choosing a tier, or when a test is slow, flaky, or passes without constraining anything. |
| Delivery | [build.md](docs/internal/standards/build.md) | Read when changing Cargo profiles, adding a dependency, bumping the toolchain, or when builds are slow or the disk is filling. |
| Delivery | [portability.md](docs/internal/standards/portability.md) | Read when a change is OS- or architecture-specific, when touching release artifacts, or when a test behaves differently on macOS. |
| Code | [async-concurrency.md](docs/internal/standards/async-concurrency.md) | Read when writing anything that spawns a task, holds a lock, awaits, or shares state across connections — the coordinator, the broker's I/O shell, and anywhere producers or consumers run concurrently. |
| Code | [behavior.md](docs/internal/standards/behavior.md) | Read when a change is visible to a Kafka client, an operator, or another tenant — protocol responses, defaults, degradation under a dependency failure, logs, or the shape of anything persisted to object storage. |
| Code | [code-structure.md](docs/internal/standards/code-structure.md) | Read when adding a crate, module, or file; when a file nears 500 lines or a function nears 50; or when writing a crate's README and AGENTS.md. |
| Code | [contracts.md](docs/internal/standards/contracts.md) | Read when defining or changing a `pub trait` in `oqueue-core`, adding a fake, or deciding whether a new type belongs on a seam or beside one. |
| Code | [error-handling.md](docs/internal/standards/error-handling.md) | Read when defining an error type, deciding whether something is a `Result` or a panic, propagating a failure across a crate boundary, or writing an error message a client or operator will see. |
| Code | [rust-style.md](docs/internal/standards/rust-style.md) | Read when naming a type or function, choosing between a generic and `impl Trait`, deciding what a lint attribute or `clippy.toml` entry should say, or when a diff is hard to read for reasons `code-structure.md` doesn't cover. |
<!-- index:standards:end -->

⚠️ Most gate scripts these standards name do not exist yet — that is
M-1.7 through M-1.12. Until then, a rule that names a script it can't yet
run is a preference, same as the non-negotiables below.

## Skills

Procedures, in [.agents/skills/](.agents/skills/), written to the Agent Skills
spec so they work in any tool that reads `SKILL.md`. A skill calls a script in
`scripts/`, never a tool-specific built-in.

<!-- index:skills:start -->
| Skill | Use when |
|---|---|
| [`adr`](.agents/skills/adr/SKILL.md) | Write an architecture decision record |
| [`milestone`](.agents/skills/milestone/SKILL.md) | Drive a milestone to completion autonomously, one task per commit, without asking between tasks |
| [`milestone-review`](.agents/skills/milestone-review/SKILL.md) | Review a milestone's commits as a whole, turn what is found into backlog tasks, and re-plan |
| [`next-task`](.agents/skills/next-task/SKILL.md) | Choose what to work on next and confirm it is genuinely ready |
| [`research`](.agents/skills/research/SKILL.md) | Find whether a question is already answered in the research corpus before investigating it fresh |
| [`review`](.agents/skills/review/SKILL.md) | Review a staged change as an independent agent that did not write it |
| [`spec`](.agents/skills/spec/SKILL.md) | Write a spec before implementing, and decompose it into commit-sized tasks |
| [`tdd`](.agents/skills/tdd/SKILL.md) | Implement a task test-first |
<!-- index:skills:end -->

**Progressive disclosure.** This file is layer 0 and is deliberately an index.
Skill *descriptions* are layer 1 and cost a few hundred words. A skill's *body*
is layer 2, loaded when invoked. Standards, product docs, and the 22-document
research corpus are layer 3, loaded only when a skill says to read one — never
wholesale. See [.agents/skills/README.md](.agents/skills/README.md).

⚠️ **`.claude/` is an adapter layer and holds no procedures.** A command file
that contains a procedure rather than a pointer is a fork waiting to drift.

## Non-negotiables

⚠️ **Bootstrap state, M-1.** Each rule below should name the script that
enforces it. **Every rule now does except rule 3, which cannot** — see rule 3
itself for why that one is the permanent exception rather than a remaining
task.

Until that task lands, the rule is a preference. That is precisely the state
this project exists not to be in, so treat the list below as M-1's checklist and
not as a description of a working system. A rule with no gate is a preference.

⚠️ **And a gate nothing invokes is a preference too.** `.git/hooks` is not
tracked, so a fresh clone runs none of these until the hooks are installed by
hand. M-1.14 is what makes a checkout inherit them.

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
   → `scripts/review.sh` builds the packet and records the verdict;
   `scripts/check-reviewed.sh` refuses a commit whose staged bytes are not the
   ones reviewed. ⚠️ The hash binds a verdict to a diff; it does **not** prove
   the reviewer was not the author — that is bought by the harness, and saying
   so is the same discipline as rule 3. See
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
