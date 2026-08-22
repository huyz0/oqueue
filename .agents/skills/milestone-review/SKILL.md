---
name: milestone-review
description: Review a milestone's commits as a whole, turn what is found into backlog tasks, and re-plan. Use at a checkpoint during a milestone and always before declaring one complete. Catches drift, contradiction, a standard that stopped being followed, and the spec being wrong — none of which per-commit review can reach.
---

# Milestone review: the outer loop

⚠️ **This is not per-commit review repeated.** Per-commit review reads a delta
against a task, by an agent that has the task and the diff. That is the wrong
instrument for anything that only exists across commits, and running it again
here costs more and finds the same things.

The question is not *was each step right* — that was asked and answered. It is
**is where we arrived coherent**.

## ⚠️ Who runs it

**An agent that did not drive the milestone.** Dispatch a fresh one and give it
the packet; do not review a milestone you built. Non-negotiable 4 buys this for
every commit, and the case is stronger here — drift, a contradiction each half
of which passed review, and a spec that should not have been written that way
are exactly the defects invisible to whoever produced them.

⚠️ **No gate checks this**, and none can: the artifact records a reviewer name
it cannot verify. Same class as non-negotiable 3, and worth saying rather than
implying the packet buys it. In Claude Code the adapter is
[`.claude/agents/milestone-reviewer.md`](../../../.claude/agents/milestone-reviewer.md);
in any other tool, dispatch whatever it calls a subagent.

## Running it

```
scripts/milestone-review.sh coverage                     what has not been read
scripts/milestone-review.sh context                      the packet for the rest
scripts/milestone-review.sh record --file v.json         validate and store it
git add reviews                                          ⚠️ unstaged does not count
scripts/check-milestone-review.sh                        the gate
```

Verdicts land in tracked [`reviews/`](../../../reviews/README.md), not in
gitignored `target/`: a milestone's coverage is a durable claim about history,
and a completion condition that only passes on one machine is not a gate. The
gate reads them **from the index**, the same way `baselines/review.txt` is read
and for the same reason.

Each takes `--milestone M-1`; without it, the milestone is the one HEAD is
working in — from the most recent commit whose subject names a task. ⚠️ It
therefore keeps naming a milestone until the *next* one's first commit lands,
which is what lets a milestone's completion be checked after its last commit.

## When

- **Always before a milestone is declared complete.** The completion condition
  should call `check-milestone-review.sh`, so this cannot be skipped by nobody
  thinking of it.
- **At checkpoints during a long milestone**, so the batch stays small enough to
  hold at once. ⚠️ **Nothing enforces the cadence and no honest constant exists**
  — it depends on how much a reviewer can actually read. Reviewing thirty
  commits in one pass satisfies the gate and is a bad way to use it.

⚠️ **A commit that only touches `reviews/` is not itself reviewed.** Recording a
review produces a commit, which names a task in the milestone, which would then
need covering — by another review, in another commit. One that carries real work
*and* a review file is still reviewed.

## Read the current state, not the diffs

The packet gives commit messages for intent and sequence, and a diffstat for
shape. ⚠️ **The subject is the files as they now stand.** A cross-cutting review
that reads deltas has become per-commit review again.

## What to look for

- **Drift** — a convention followed early and quietly abandoned.
- **Contradiction** — two things that each passed review and disagree.
- **Abstraction** — something now duplicated enough to extract, or an
  indirection that never earned itself and should collapse.
- **A standard that stopped being followed**, including one nothing enforces.
- ⚠️ **The spec being wrong.** No per-commit review reaches this, because every
  commit was faithful to a task that should not have been written that way.
- **A gate that passes for the wrong reason**, or whose failure path nobody has
  run. A gate nobody has watched fail is a gate nobody has tested.
- **A claim in a document that the code no longer supports.**

## Findings become backlog tasks

⚠️ **This is the step that makes the loop a loop.** A finding recorded only in
a review artifact is one nobody will act on — not because the file is fragile
(`reviews/` is tracked) but because **nothing reads it**. `next-task` reads the
backlog, and so does everyone else.

Every **blocking** or **major** finding names a `task_id` that the backlog
already lists: write the row first, then cite it. The gate checks the row
exists. A finding you judge non-actionable is `minor` and needs no task.

⚠️ **Harvest rule 15's minors** (ADR-0016, closing `bcf5d6f697f2`): sweep the
commit bodies since the last checkpoint for minors recorded there, and turn
the ones worth work into **sweep rows** (`sdd.md`). This is the step `M0.27`-
`M0.29` and `M1.58` each performed by hand before it was anyone's job.

The alternative is to **argue** it in `baselines/review.txt` with a reason, the
same escape a blocking per-commit finding has. It takes two steps, because the
`id` you argue is assigned by `record`:

1. `record` the verdict with the finding's `task_id` left empty. It is stored,
   and its `id` is printed. ⚠️ **Recording is not resolving** — the gate refuses
   it in exactly this state.
2. Stage a line in `baselines/review.txt`: `<id>  <why this is not a task>`.

Use it where the right outcome genuinely is not a backlog row — `spec-wrong` at
blocking severity is the case, since §"Then re-plan" says that is a decision.

## Then re-plan

1. **Amend the roadmap with what was learned**, not with "complete". A milestone
   that taught nothing was either trivial or unexamined.
2. **Re-decompose what remains.** Findings that became tasks change the order,
   and sometimes the plan — see [`spec`](../spec/SKILL.md).
3. If the review found the **spec** wrong, that is a decision, not a task.
   Report it; do not quietly re-specify. Record the finding with no `task_id`
   and argue it, per the two steps above — that leaves the decision visible in a
   tracked file instead of in a gitignored artifact.

## What this cannot do

⚠️ **It cannot tell who reviewed, or whether the review was any good.** The
`reviewer` field is a string nothing verifies, and a reviewer can read nothing
and return an empty findings list. What the gate buys is that the
commits were *named* — a milestone cannot be declared complete while some of its
work has never been looked at as a whole. Same class as non-negotiable 3.
