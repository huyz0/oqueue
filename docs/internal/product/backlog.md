# Backlog

One task equals one commit equals one change that leaves the tree green. If a
task cannot be finished in one green commit, split it before writing code.

Task IDs are stable. Completed tasks stay here with their commit reference so
the history of why something was done survives.

Decomposition rule: only the current milestone is decomposed in detail. Future
milestones stay as roadmap entries until their turn.

## M-1: AI development system

Build the system that builds everything else. Ordered by dependency — the
product docs come first because everything else references them, and `lib.sh`
comes before any gate because every gate sources it.

| ID | Task | Acceptance | State |
| --- | --- | --- | --- |
| M-1.0 | Repo foundation: `git init`, Apache-2.0 `LICENSE`, `.gitignore`, `rust-toolchain.toml`, and the research corpus placed under version control | Files exist; toolchain pins 1.97.1 and both Linux targets | done |
| M-1.1 | `AGENTS.md` + `CLAUDE.md` adapter, non-negotiables with honest enforcement marking | Every rule names its gate task or is marked unenforceable; no rule claims a gate that does not exist, and no link points at a file that is not there | done |
| M-1.2 | `mission.md` — what this is, what it costs, what it must never do | States the latency position and the scale target with citations into the corpus | done |
| M-1.3 | `architecture.md` — crate map, dependency rule, the seams | Names all eleven crates, the star rule with its two exceptions, and the `unsafe` budget | done |
| M-1.4 | `roadmap.md` + `backlog.md` — milestones with executable completion conditions | Every milestone's "done" is a command; only M-1 is decomposed | done |
| M-1.5 | `standards/` — remaining code standards: `behavior`, `rust-style`, `error-handling`, `async-concurrency`, `contracts` | ⚠️ Every carried-over threshold is re-derived against oqueue or explicitly marked underived | todo |
| M-1.6 | `scripts/lib.sh` + `check-commit-msg.sh` | Commit subject must name a task this file lists; `require_tool` skips with a named remedy rather than failing | todo |
| M-1.7 | `check-drift.sh` + `check-tests-kept.sh` | A threshold made settable fails; a deleted test without `Removes-test:` fails | todo |
| M-1.8 | `check-layering.sh` + `check-sans-io.sh` | Sideways dependency fails; a concrete socket type, real clock read, or object-store call in a library crate fails | todo |
| M-1.9 | `review.sh` + `check-reviewed.sh` — the isolated reviewer and its gate | Review artifact keyed by staged-diff hash; amending one byte after review fails the commit | todo |
| M-1.10 | `check-core-contract.sh` | A `pub trait` method-set change without every implementor and an ADR in the same commit fails | todo |
| M-1.11 | `check-unsafe.sh` | `unsafe` outside the three named crates fails; every `SAFETY:` block has a baseline entry | todo |
| M-1.12 | `check-budget.sh` — the pre-commit time budget as an enforced constant | Suite over budget fails; timings written as an artifact so erosion shows as a trend | todo |
| M-1.13 | `.agents/skills/` — `next-task`, `spec`, `tdd`, `adr`, `contract-change`, `review` | Each parses as the Agent Skills spec; each calls `scripts/`, never a tool built-in | todo |
| M-1.14 | `.pre-commit-config.yaml` (direct-to-main) + push-triggered CI | ⚠️ No gate keyed to `origin/main...`; PR-triggered gates rebased onto the previous commit | todo |
| M-1.15 | `tests/gates/negative.sh` — prove every gate can fail | Each gate invoked against a broken artefact and observed to fail | todo |
| M-1.16 | `scripts/gates/m-1-complete.sh` — the milestone's own completion condition | Asserts every non-negotiable names a passing script, except rule 3 | todo |
| M-1.17 | Make the corpus and product docs self-contained before the repo goes public | No reference to any other repository, no absolute local path, no verbatim quotation of an external private source; every practice stated as this project's own standard | done |
| M-1.18 | Public-facing files: `README.md`, `CONTRIBUTING.md`, `SECURITY.md` | README states plainly that no implementation exists; contributing says code is not yet accepted and why; security gives a private reporting route | done |
| M-1.19 | Record the BYOK and FIPS requirements across mission, architecture, roadmap, and the corpus | New milestone M8; `KeyProvider` seam and `oqueue-crypto` crate in the architecture; the AEAD-algorithm-in-region-header constraint recorded against M1 | done |
| M-1.20 | Revise the encryption design for the clarified BYOK volume (~10K topics, not catalog-wide) | Segregation replaces universal per-region sealing; the KMS key-count conclusion corrected and marked as corrected | done |
| M-1.21 | `requirements.md` — functional and non-functional, with stable IDs | Every entry names a verification; every NFR carries a number or is marked UNDERIVED with what blocks it; nothing invented | done |
| M-1.22 | `standards/sdd.md` — the process standard | Defines the requirement→spec→task→commit chain, what a spec must contain, acceptance-criteria rules, definition of done, and what to do when a spec proves wrong | done |
| M-1.23 | `standards/review.md` — the operational review standard | Turns [docs/researches/21](../../researches/21-ai-development-loop.md) §3–5 into a standard: reviewer context isolation, the deterministic/semantic split, fixed-or-argued resolution | todo |
| M-1.24 | Trace every milestone to the requirements it serves | Every roadmap entry names FR/NFR IDs; a gate fails on a milestone that names none | todo |
| M-1.25 | `standards/security.md`, `performance.md`, `build.md`, `portability.md` | Each rule names its gate or is explicitly marked as having none; rationale delegated to the corpus rather than restated | done |
| M-1.26 | `standards/code-structure.md` + `standards/testing.md` + `clippy.toml` | File ≤500 lines with a reasoned allowlist; function ≤50 lines, cognitive complexity ≤20, ≤5 arguments, all via `clippy.toml`; per-crate `README.md` and `AGENTS.md` required; fakes over mocks; the no-flake rules | done |
| M-1.27 | `check-file-size.sh` + `check-readmes.sh` | File-size limit with an allowlist whose entries carry reasons; every crate has both documents, and the README's stated dependencies match `Cargo.toml` | todo |
| M-1.28 | `scripts/profile.sh` + `scripts/bench.sh` | Every profiling mode is one command: instructions, flamegraph, heap, massif, cache, allocation counts. **Never gated** — available on demand | todo |
| M-1.29 | `check-hot-path-bench.sh` | Every hot path named in `performance.md` rule 18 has a benchmark, so the list cannot silently rot | todo |

### Notes on specific tasks

**M-1.5** carries the hazard the hybrid decision named: a ported rule justified
by the other project's wire protocol, or a threshold derived from a test suite
that does not exist here, is **stale but authoritative** — worse than absent.

**M-1.9** is new to oqueue and has no precedent to port. The mechanism is in
[docs/researches/21](../../researches/21-ai-development-loop.md) §5. The
reviewer must not receive the author's reasoning; separation of invocation is
not separation of information.

**M-1.12** has no constant yet. Two minutes is a common figure for a pre-commit
budget, but it is usually written as a comment and never measured, and oqueue's
dependency graph is heavy. Derive it from measurement once M0's workspace
exists; until then the task is blocked rather than guessed.

**M-1.14** ⚠️ the CI adaptation that is easy to miss: with no pull requests, any
gate triggered by one silently never runs.

**M-1.26** makes the structural limits real rather than aspirational.
`clippy::too_many_lines`, `cognitive_complexity`, and `too_many_arguments` are
all in the `pedantic` group, which is already denied workspace-wide, so the
thresholds in `clippy.toml` are enforced the moment a crate exists. ⚠️ The
500-line file limit needs its own script and an allowlist, because generated
protocol tables and exhaustive `match` arms over wire types are legitimately
large — M-1.27.

**M-1.28** is deliberately **not** a gate. Profiling is slow, needs a quiet
machine, and produces output requiring judgement; gating on it would either
stall the loop or produce alerts nobody trusts. The requirement is that it be
runnable at any moment without ceremony.

**M-1.25** closed a gap in the standards plan. M-1.5's list was adapted from a
service of a different shape — one that ships a single architecture, carries no
benchmarking methodology, and holds no customer key material. Four of the
categories oqueue actually needs had no standard planned at all. The remaining
M-1.5 set is now code standards only.

**M-1.21** deliberately leaves several NFRs **UNDERIVED** rather than choosing
plausible numbers. NFR-13 (aggregate throughput) blocks NFR-31 and gates
architectural decisions; NFR-55 and NFR-56 are constants that cannot be picked
before there is a workspace to measure. ⚠️ An invented number is
indistinguishable from a measured one a month later, which is why the standard
forbids it outright.

**M-1.20** corrected a conclusion rather than extending one. The first
encryption draft assumed BYOK might apply catalog-wide and derived a design from
AWS KMS's 100,000-key ceiling. At ~10,000 BYOK topics that ceiling is not
binding, and the resolution changes from *seal every region of every object* to
*segregate BYOK data into its own objects*. Both the corpus and the decision log
mark this as a correction, because a reader arriving at the earlier reasoning
would otherwise inherit a constraint that does not apply.

**M-1.17** was executed out of ID order, immediately before the first push. Task
IDs are stable, so the table is ordered by ID rather than by execution. The
substance of every borrowed practice survives — it is now stated as this
project's standard under a `[Practice]` marker, which keeps the honest
distinction from `[Assessment]` (our own reasoning) and `[Documented]`
(externally cited).
