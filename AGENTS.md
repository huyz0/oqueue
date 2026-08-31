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

⚠️ **A rule whose script is missing is a preference**, and some of the scripts
these standards name are still missing. Not because a milestone left a row open
— ⚠️ that was true when this paragraph was written and `M0.16` closed the row —
but because the standards name scripts no milestone was ever scoped to write.
Each of those needs something to check that does not exist yet, which is why
they are deferrals with receiving milestones rather than oversights.

⚠️ **Do not look for the list here.** A hardcoded list of what is outstanding
went stale four separate times elsewhere in this repository before the fix
turned out to be deleting it;
`docs/internal/product/backlog.md` is where the answer is true on the day you
read it, because each task updates it as it closes. So: **check whether
`scripts/` has the file** before treating a rule as enforced, and if it is
missing, look in **two** places — its backlog row, and
[`roadmap.md`](docs/internal/product/roadmap.md)'s **"Deferred into a later
milestone"** table. ⚠️ **"No backlog row" does not mean unscheduled**, which
this sentence used to say: an obligation with no owning milestone yet is
recorded in that table and in the receiving milestone's plan, and has no row by
design. A script in neither place is genuinely unscheduled — never "already
done".

## Skills

Procedures, in [.agents/skills/](.agents/skills/), written to the Agent Skills
spec so they work in any tool that reads `SKILL.md`. A skill calls a script in
`scripts/`, never a tool-specific built-in.

<!-- index:skills:start -->
| Skill | Use when |
|---|---|
| [`adr`](.agents/skills/adr/SKILL.md) | Write an architecture decision record |
| [`contract-change`](.agents/skills/contract-change/SKILL.md) | Change a pub trait's method set in oqueue-core |
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

Each rule below names the script that enforces it. **Every rule does except
rule 3, which cannot** — see rule 3 itself for why that one is the permanent
exception rather than a remaining task. M-1 is complete, so this list now
describes a working system rather than a checklist, and every script named
below exists and passes. ⚠️ That is a claim about *these seven rules only*;
the standards above name gates beyond them, and a rule whose script is missing
is still a preference.

⚠️ **A gate nothing invokes is a preference too**, and neither of the two ways
to invoke these covers everything on its own. `.github/workflows/gates.yml`
runs `pre-commit run --all-files` on every push — but with `SKIP:
check-reviewed`, because a verdict artifact lives under gitignored `target/`
and CI has nothing to read. **Rule 4 is therefore enforced only by the local
hook**, and that is by design rather than by omission: a commit cannot exist in
history unless it passed locally, which rests on rule 3. Locally the hooks are
tracked in [`.githooks/`](.githooks/) and installed by
`scripts/setup-hooks.sh`, which points `core.hooksPath` at them; a clone where
nobody ran it, pushing to CI, has rule 4 enforced by nothing at all.

⚠️ **The hooks are tracked because the untracked one went stale and nothing
could see it.** `M10.24` found this machine committing through a hand-written
`.git/hooks/pre-commit` predating `.pre-commit-config.yaml`, which ran **12**
of the config's **17** pre-commit gates — `check-crate`, `check-coverage`,
`check-mutants`, `check-conformance-matrix` and `check-budget` were silently
absent — so every commit passed a subset while the tree claimed the suite. An
untracked hook cannot be reviewed, updated by a commit, or seen to have gone
stale. ⚠️ **`.githooks/` is not a second definition of the gate**: each hook
delegates to `pre-commit run --hook-stage <stage>` against the same
`.pre-commit-config.yaml` CI reads, and decides only *where* the stage runs.

1. **One task equals one commit equals one change that leaves the tree green.**
   Split anything that cannot meet that. The commit subject starts with the
   backlog task ID. → `scripts/check-commit-msg.sh` (M-1.6)
2. **Never move a threshold in the direction that weakens its gate, and never
   delete a test, to make a check pass.** For a floor that is lowering; for a
   ceiling, *raising*; `m0-complete.sh`'s `NFR_CONSTANTS` table names the
   weakening direction per constant (`M2.7` — the old wording said "never
   lower", which named the safe direction for three of the seven constants it
   governs). Thresholds are constants no environment can move.
   → `scripts/check-drift.sh`, `scripts/check-tests-kept.sh` (M-1.7)
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

## Running anything that builds or tests

⚠️ **Run it in the container.** `scripts/docker-test.sh` is the local way to run
anything that invokes `cargo`:

```
scripts/docker-test.sh gate                    # the whole pre-commit suite
scripts/docker-test.sh                         # cargo test --workspace
scripts/docker-test.sh cargo test -p oqueue-core
scripts/docker-test.sh scripts/gates/m3-complete.sh
scripts/docker-test.sh bash                    # a shell inside it
```

⚠️ **This is containment, not tidiness.** On WSL2 the VM has no per-process
ceiling, so a build or test that allocates without bound exhausts the whole
machine and takes the running session with it — recovery is a manual
`wsl --shutdown`. The container caps memory with swap disabled, CPUs, pids and
`/tmp`, so the kernel kills something inside it instead. Verified by watching
it: a deliberate allocator loop under `MEM=256m` exits 137, and the host does
not move.

⚠️ **The container is the *more* capable environment, which is worth knowing
before treating a skip as normal.** It carries the cross toolchain
`m0-complete.sh` needs to check the whole workspace for aarch64, a JDK and
`confluent-kafka` so `kafka-client-harness.sh` runs both real clients, and a
nightly toolchain with `cargo-fuzz`. ⚠️ **One leg cannot run inside**:
`m1-complete.sh` starts MinIO through a Docker daemon the container has no
socket for, so it skips and says the gate proved nothing about S3 — run that
one on the host.

⚠️ **Reviewing stays on the host**, because `scripts/review.sh` needs no
toolchain — but the verdict it writes is bind-mounted into the container, since
`check-reviewed` is a pre-commit gate and the commit path now runs in there.
⚠️ `docker-test.sh gate` still skips it, for a different reason than CI does:
`gate` runs `--all-files` and `check-reviewed` answers a question about *staged*
bytes. CI skips it because a runner holds no verdicts at all.

⚠️ **The commit hook runs in here too** (`M10.24`) — which is what
`scripts/setup-hooks.sh` installs, and why it exists. `check-crate`,
`check-coverage` and `check-mutants` are all `stages: [pre-commit]`, so
`git commit` runs `cargo test`, an instrumented `cargo llvm-cov` and
cargo-mutants; before `M10.24` all three ran on the host, uncontained.
⚠️ **The whole stage in one container, not a container per hook**: `lib.sh`
keys a run by process group and `check-budget.sh` groups on it, and inside a
PID namespace every run is pgid **1** — so `docker-test.sh` sets an
`OQUEUE_RUN_ID` that both prefer, and one container per stage keeps a stage's
gates under one key. ⚠️ **`CI`, `OQUEUE_NO_CONTAINER`, or an unreachable Docker
fall back to a native run**, and the last of the three says so in yellow first:
a gate that quietly stopped protecting the machine is worse than one that
failed. ⚠️ **`target/review` is bind-mounted back in** over the volume, because
`check-reviewed` is a pre-commit gate reading a verdict the host wrote.

⚠️ **CI does not use the container.** GitHub Actions runners are already
isolated VMs, so containing them again would only cost build time;
`.github/workflows/gates.yml` runs the same commands natively. `gate` expands
to the same `pre-commit` invocation rather than to a list kept here, so the two
cannot drift into different definitions.

⚠️ **NFR-56's budget moves with the CPU cap.** `check-budget.sh` times the
suite against a 10 s ceiling non-negotiable 2 forbids raising, and the
container's share changes the measurement. ⚠️ **The spread is wide and the
ranges overlap**: across repeated runs 8 CPUs gave 8153-9255 ms and 12 gave
7577-8904 ms, so `CPUS=12` buys roughly a tenth of the ceiling rather than the
quarter a single pair of readings suggested — 16 measured 7508 ms and is not
worth the host's cores. 12 is the default on that basis, not on a clean
separation. ⚠️ **Lowering it can fail
the budget inside while the host passes**, and the reading is that the caps
were tightened, not that the suite grew — the constant is not the repair. The
container also keeps its own `target/`, so its own trend history.

## Never

- Never push unless asked.
- Never commit a tree you know is broken, including "I will fix it in the next
  commit".
- Never widen scope silently. Doing more than the task asked is as much a
  problem as doing less, because it breaks the one-task-one-commit property.
- Never add a dependency that pulls in a C toolchain without recording why.
  Build portability is a design constraint here, not a packaging afterthought —
  see [docs/researches/20](docs/researches/20-build-and-release-portability.md) §1.
