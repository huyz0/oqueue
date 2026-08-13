---
title: "The AI Development Loop: Author/Reviewer Separation, Gate Allocation, and Anti-Slop"
slug: ai-development-loop
status: working-position
last_updated: 2026-08-13
tags: [sdd, ai-development, review, subagents, quality-gates, mutation-testing, slop, autonomous-loop, goal, agents-md, cycle-time, build-speed, budget]
related: [19-workspace-engineering, 18-rust-performance-methodology, 10-open-questions]
summary: >
  Design for an unattended, fully AI-authored development loop where no human
  reads the code. Two gaps in the inherited pgprox system: the reviewer is the
  same agent that wrote the code, and the review step is enforced by nothing.
  Fixes both — context-isolated reviewer subagent, and a review artifact keyed
  to the staged diff hash so a pre-commit gate can require it. Plus the rule
  that decides what is a script and what is an agent, and a taxonomy of AI slop
  mapped to the gate that catches each kind.
---

# The AI Development Loop

**⚠️ Not research — a design position**, in the sense of [15](15-scale-architecture-position.md). It extends the development system inherited from pgprox (see [19](19-workspace-engineering.md) and the 2026-08-13 methodology decision in [10](10-open-questions.md)) to the constraint that **no human reads the code**.

Marked **[pgprox]** (exists today), **[Gap]**, or **[Design]**.

---

## 1. What already exists

**[pgprox]** `docs/internal/standards/behavior.md` defines the cycle: read roadmap and backlog → take the top unblocked task → implement test-first → **review before committing** → commit with the task ID in the subject → tick the backlog. One task = one commit = one green tree, direct to `main`, no feature branches, *"because every commit is green and every commit is small… A bad commit is reverted, not merged around."*

**[pgprox]** The `crate-review` skill already states the governing principle for gate allocation:

> *"Run the checks first… Anything red means keep working. **The rest of this is for what the scripts cannot see.**"*

**[Assessment]** That is the right principle and it is already the design. Nothing below replaces it. Two things around it are missing.

---

## 2. The two gaps

**[Gap 1] The reviewer is the agent that wrote the code.** `crate-review` is a skill the authoring agent runs on its own work. A self-review inherits every blind spot that produced the defect: the same misreading of the task, the same assumption about what a function guarantees, plus a commitment bias toward work already done. It reliably catches typos and reliably misses the thing it was wrong about.

**[Gap 2] Nothing enforces that the review happened.** Step 4 of the cycle is a behavioural rule. In an unattended `/goal` loop, an agent can skip it, or — worse and more likely — *report* having done it. This is the same class as non-negotiable #3 (*"never claim a test passes without having run it. **No script enforces this.**"*), and pgprox is explicit that #3 is the rule the other six rest on.

**[Assessment]** With a human in the loop these are tolerable, because a person eventually reads the diff. **With no human reading code, gap 2 is the load-bearing failure**: an unrun review reported as clean is strictly worse than no review, because it consumes the budget that would have bought a real one and produces a false record.

---

## 3. The allocation rule

**[Design]** Two directions, both mandatory:

> **1. Anything a script can decide must never be delegated to an agent.**
> **2. The reviewer's budget must be spent only on what no script can reach.**

The first is obvious and widely violated. The second is not: a reviewer that re-checks formatting, lint-level issues, or test pass/fail spends its limited attention on noise and misses the semantic defect. **The review prompt must therefore be told which deterministic gates already ran and passed**, explicitly, so it does not re-derive them.

The deeper reason for direction 1: **a check performed by an agent cannot be regression-tested.** A script has a fixed behaviour you can prove fails when it should ([pgprox] `tests/gates/negative.sh` invokes each gate against a broken artefact to prove it fails). An agent's judgement drifts between runs, between models, and between context loads.

### 3.1 The allocation

| Deterministic — a script, always | Semantic — an agent, necessarily |
|---|---|
| fmt, clippy (incl. pedantic), compilation | Does the code do what the task specified — no more, no less? |
| Test pass/fail; coverage against a constant | Are the tests testing *behaviour*, or asserting the implementation back at itself? |
| **Mutation survivors vs. baseline** | Is this the simplest thing that works, or invented structure? |
| Layering, sans-I/O, contract atomicity | Is the abstraction at the right level, and does it duplicate one that exists? |
| Unsafe presence/absence; `forbid` | Is a `SAFETY:` argument actually sound? |
| Public API diff (`cargo-public-api`) | Does this introduce a concept that contradicts an existing one? |
| Supply chain (`cargo-deny`), secret scan | Do the comments state things that are true? |
| Benchmark regression (instruction counts) | Does this error message let an operator act? |
| Commit subject names a real backlog task | Is the *spec itself* right — does it serve the mission? |
| Tests-kept; drift; wiring (nothing dead) | Was scope silently widened? |

**[Assessment]** The right-hand column is short by design. Every row that migrates leftward is a permanent gain, because it becomes cheap, repeatable, and provable. **A recurring semantic finding is a bug report against the gate set** — the correct response to catching the same class twice by review is to write a script, exactly as pgprox's M10/M12/M13 milestones did.

---

## 4. Author/reviewer separation

**[Design]** The reviewer is a **separate subagent with an isolated context**. Separation of *invocation* is not enough; what matters is separation of *information*.

**The reviewer receives:**
- the task, verbatim from the backlog, with its acceptance criteria
- the staged diff
- the relevant standards documents
- **the list of deterministic gates that already passed** (§3)

**The reviewer must not receive:**
- the authoring agent's transcript, plan, or reasoning
- the author's own description of what the change does
- any justification the author produced

**[Assessment]** That last exclusion is the one that carries the weight. An author's rationale is *persuasive by construction* — it was generated to make the change seem correct — and a reviewer given it grades the rationale rather than the code. The reviewer should reconstruct intent from the task and the diff alone, because a mismatch between what the task asked and what the diff does is exactly the defect class self-review cannot see.

Three further rules:

1. **Adversarial framing.** Ask *"what is wrong with this?"*, never *"is this acceptable?"* The second reliably returns approval.
2. **A different model where practical.** Decorrelates blind spots at near-zero cost. Author and reviewer sharing a model share failure modes.
3. **Structured output.** Each finding anchored to `file:line`, with a severity and a concrete failure scenario. A finding that cannot name how it fails is a style opinion, and style is column one's job.

---

## 5. Making the review non-skippable

**[Design]** This is the mechanism that closes gap 2, and it is the core proposal of this document: **convert the review from a claim into an artifact, then let a deterministic gate check the artifact.**

```
scripts/review.sh
  1. hash = sha256(git diff --cached)
  2. spawn reviewer subagent with the §4 context (isolated)
  3. write target/review/<hash>.json  → { task_id, findings[], verdict }

scripts/check-reviewed.sh          ← pre-commit hook
  1. recompute hash of the staged diff
  2. require target/review/<hash>.json to exist
  3. require zero unresolved blocking findings
```

**[Assessment] Keying on the diff hash is what makes this tamper-evident.** Amend one byte after the review and the hash changes, the artifact no longer matches, and the commit is refused. There is no way to satisfy the gate except by actually running a review against the exact bytes being committed — which is precisely the property non-negotiable #3 cannot have for test claims, obtained here for review claims.

It also makes review history queryable: `target/review/` accumulates a record of what was examined and what was found, which is the artifact a later audit needs.

**[Assessment]** Generalize the pattern: **every claim that matters should leave an artifact keyed to the thing it claims about.** `mutants-baseline.txt`, `wired.txt`, and coverage constants are already this shape. Reviews join them.

---

## 6. Escalation: fix, or argue

**[Design]** With no human adjudicator, a disputed finding needs a resolution that is not "the author decided." Reuse the vocabulary pgprox already applies to mutation survivors — *"155 survivors, 137 killed and 18 argued."*

A blocking finding is resolved in exactly one of two ways:

1. **Fixed** — the diff changes, the hash changes, review re-runs.
2. **Argued** — an entry is added to a review baseline naming the finding and the reason it is not a defect. The gate then passes.

The baseline is a file nobody may grow quietly, which is the same discipline as the coverage constant and the mutants baseline. **A growing argued-list is itself a signal** — worth reviewing at milestone boundaries, since it is where a reviewer is being systematically overruled and one side is systematically wrong.

---

## 7. AI slop, and which gate catches it

**[Assessment]** Naming the failure modes concretely, because "slop" is otherwise unactionable:

| Failure mode | Caught by |
|---|---|
| Test executes the line but asserts nothing that would fail | **Mutation testing** — definitively |
| Test asserts the implementation back at itself | Mutation testing (partly); semantic review |
| Code exists, nothing reaches it | `check-wired.sh`, dead-code lints |
| Coverage reached by exercising, not by checking | **Mutation testing** |
| Trait with one implementor; generic never varied | Semantic review |
| Scope widened past the task | Semantic review vs. acceptance criteria; diff-size heuristic |
| Error swallowed (`unwrap_or_default` on a real failure) | clippy + semantic review |
| Comment that contradicts the code | Semantic review |
| Logic duplicated because an existing helper wasn't found | Semantic review |
| Solved an adjacent problem to the one specified | Semantic review vs. acceptance criteria |
| Plausible but unsound `SAFETY:` argument | Semantic review + the artifact gate ([19](19-workspace-engineering.md) §5.7) |

**[Assessment] Mutation testing is the primary anti-slop gate, and this reframes it.** [19](19-workspace-engineering.md) §9 treats it as a test-quality audit — third priority after property tests and DST. **Under fully-AI-authored code it is promoted**, because the single most characteristic AI failure is a test that executes code without constraining it, and that is *definitionally* a surviving mutant. It is the only mechanical instrument that distinguishes a test suite that checks behaviour from one that merely visits lines.

This raises the value of the diff-narrowed per-commit run ([19](19-workspace-engineering.md) §9.2) specifically: it makes mutation testing affordable inside the inner loop rather than only nightly, which is where AI-authored tests need it.

---

## 8. Two loops

**[Design]**

**Inner loop — per task, per commit:**

```
plan (decompose to acceptance criteria)
  → implement test-first
  → deterministic gates          ← red means keep working; never negotiate
  → review by an isolated subagent, artifact written   ← §4, §5
  → fix or argue                 ← §6
  → commit (pre-commit re-runs gates + check-reviewed.sh)
  → tick the backlog
```

**Outer loop — per milestone:**

```
completion condition runs as a command, not a judgement
  → cross-cutting review over the milestone's commits
  → findings become backlog tasks
  → amend roadmap with what was learned
  → decompose the next milestone
```

**[Assessment]** The outer loop catches what per-commit review structurally cannot: drift accumulated across commits, two concepts that each passed review but contradict each other, an abstraction that should now be extracted or collapsed, a standard that quietly stopped being followed — and **the spec being wrong**, which no amount of code review reaches.

pgprox demonstrates this loop working, but **initiated by inspiration rather than by schedule**: M10 ("the claims nothing enforces"), M12 ("the gates that count files"), M13 ("the non-negotiables that nothing enforces") are outer-loop reviews that became milestones. The improvement available is to make it **structural — a required phase of every milestone's completion**, not a milestone someone thought to write.

---

## 9. Cycle time: how speed is ensured, not merely achieved

**[Assessment] In an unattended loop the gates are serial.** A human waiting five minutes on a check switches to something else; the loop simply blocks. **Gate latency divides throughput 1:1**, and it degrades monotonically — every commit adds tests, dependencies, and gates, and nothing removes them. Techniques for *making* the build fast are in [18](18-rust-performance-methodology.md) §3.7.5 and [19](19-workspace-engineering.md) §1.1. This section is about *keeping* it fast.

### 9.1 The tiers

**[pgprox]** Three local tiers already exist — agent hooks call the same scripts for in-session feedback, `pre-commit` binds every commit, and `pre-push` carries heavier work — with the rule that *"a check implemented in two places drifts, and the version that matters is whichever one the developer did not run."*

| Tier | When | Target |
|---|---|---|
| `cargo check`, rust-analyzer | every save | < 2 s |
| scoped `check-crate.sh <crate>` | after editing a crate | < 30 s |
| **pre-commit** | **every commit — the serial bottleneck** | **the stated 2 min** |
| pre-push | before push | minutes |
| CI tier 1 | every push | ~10 min |
| CI tier 3 (mutants, fuzz, miri) | scheduled | hours |

**[Design]** Scoping is permitted in pre-commit and forbidden in CI. Pre-commit is feedback; CI is truth. That resolves [19](19-workspace-engineering.md) §5's warning against path-filtered CI without giving up fast local iteration.

### 9.2 ⚠️ The budget is a comment, not a gate

**[pgprox]** `.cargo/config.toml` states *"the pre-commit budget is two minutes."* Nothing measures it. No script times the suite and fails when it exceeds the number.

**[Assessment]** By this system's own thesis, **the pre-commit budget is currently a preference** — and it is the single constraint that determines whether an autonomous loop remains viable as the codebase grows. It should become a constant enforced exactly like the coverage threshold: `scripts/check-budget.sh` times the suite and fails over N seconds, and the measured timings are written as an artifact (§5's pattern) so erosion shows up as a trend rather than arriving as a crisis.

### 9.3 The four cost centres

| Cost | Dominated by | Bounded by |
|---|---|---|
| **Compilation** | dependency count, DAG depth, debuginfo | [18](18-rust-performance-methodology.md) §3.7.5, [19](19-workspace-engineering.md) §1.1 |
| **Coverage** | `-C instrument-coverage` changing the fingerprint | **[pgprox]** already runs llvm-cov under a separate `CARGO_TARGET_DIR`, so the instrumented and normal builds do not evict each other. Costs another artifact tree — see [18](18-rust-performance-methodology.md) §3.7.2. |
| **Mutation testing** | one **rebuild** per mutant | Crate granularity: incremental rebuild scope *is* the mutation budget. Keeps it in CI, never pre-commit — even 30 diff-narrowed mutants is 30 × (build + test). |
| **Review subagent** | token latency, **not CPU** | Run it **concurrently** with the deterministic gates. Otherwise the cores idle while waiting on tokens. |

**[Assessment]** The mutation-testing row is the useful connection: [19](19-workspace-engineering.md) §1's argument for many small crates was made for build parallelism, and it turns out to buy mutation-testing throughput by the same mechanism — a mutant in a small leaf crate rebuilds only that crate.

### 9.4 A slow test is a sans-I/O violation

**[Assessment]** If business logic touches no socket and no clock ([19](19-workspace-engineering.md) §4.1), its tests are sub-millisecond — **[pgprox]** measures fourteen of sixteen crates finishing under 0.4 s. It follows that **a test which suddenly takes 500 ms has acquired I/O somewhere it should not have.**

So a per-test time threshold is not only a speed gate; it is a second detector for the architectural rule that matters most. One constant, two properties, and the failure it reports is the more useful of the two.

⚠️ Distinguish it from the mutation-run timeout in [19](19-workspace-engineering.md) §9.3, which must be *generous* because a tight cap there produces silent false kills. These two timeouts point in opposite directions and must be derived separately.

### 9.5 Monotonic growth

**[pgprox]** `check-tests-kept.sh` names any test that disappears and requires a `Removes-test:` line in the commit message — deletion is possible but recorded, which is the correct shape. **[Assessment]** No change needed, but the milestone-boundary review (§8) should treat suite runtime as a reviewable quantity: a suite that has doubled without the crate count doubling is a finding, not a fact of life.

---

## 10. Open questions

- **Cost of per-commit review.** (see also §9.3 — it is network-bound, so it parallelizes with the gates)
- **What is the actual pre-commit budget for oqueue?** pgprox states two minutes; the number for a broker with a heavier dependency graph has to be derived from measurement, and §9.2's gate needs a constant to hold.
- **Cost of per-commit review.** A reviewer subagent on every commit is real token spend. Is it every commit, or gated by diff size / touched-crate risk? A cheap heuristic (any diff touching `oqueue-core`, any `unsafe`, any public API change → always review) may capture most of the value.
- **Reviewer prompt drift.** The reviewer is defined by a prompt, and a prompt is not version-pinned the way a script is. How is a change to the review prompt itself reviewed? Candidate: treat the prompt as source, subject to the same gates, with `tests/gates/negative.sh`-style cases proving the reviewer *fails* on known-bad diffs.
- **Who reviews the spec?** §8 puts it in the outer loop, but a wrong spec is the most expensive failure and the outer loop is slow. Possibly a spec-review subagent before implementation, against mission and architecture.
- **Model diversity in practice.** Is a different model for review worth the operational complexity, and does it measurably decorrelate findings? Testable: run both configurations over a set of known-bad diffs.
- **When does an argued finding become a gate?** §6 says a growing argued-list is a signal. What threshold triggers writing a script instead?

---

## Sources

**Primary — the pgprox development system** (read 2026-08-13): `AGENTS.md` (non-negotiables and their honest enforcement marking), `docs/internal/standards/behavior.md` (the cycle, one-task-one-commit, direct-to-main), `.agents/skills/crate-review/SKILL.md` (the deterministic-first principle), `docs/internal/product/roadmap.md` (M10/M12/M13 as outer-loop reviews), `scripts/mutants.sh` and `docs/internal/product/mutants-baseline.txt` (the fixed-or-argued pattern), `tests/gates/negative.sh` (proving a gate can fail).

**External** — [Agent OS v2](https://buildermethods.com/agent-os/v2) (Standards + Product + Specs structure, assessed as a subset — see [10](10-open-questions.md)).
