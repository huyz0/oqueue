---
name: milestone
description: Drive a milestone to completion autonomously, one task per commit, without asking between tasks. Use when told to work a milestone, to continue, to keep going, or to auto-proceed. Defines the loop and — more importantly — the conditions under which it must stop.
---

# Milestone: the autonomous loop

Runs a milestone to completion without a human between tasks.

## Before starting

1. Read `docs/internal/product/roadmap.md`. Identify the current milestone and
   its **completion condition**, which is a command.
2. ⚠️ **If the completion condition is not a command, stop.** A milestone whose
   "done" is a judgement cannot be driven autonomously, and pretending otherwise
   produces a loop that never terminates or terminates on a guess. Fix the
   roadmap first.
3. If the milestone is not decomposed in `backlog.md`, decompose it now — with
   acceptance criteria, citing requirement IDs. Only this milestone; see
   [`spec`](../spec/SKILL.md). ⚠️ **Read
   `docs/internal/product/milestones/M<n>.md` first**: it names the decisions
   that must be recorded *before* this milestone's code, and skipping them is
   how a milestone acquires an architecture nobody chose. Its task list is an
   input to re-derive from, never a list to copy.
   ⚠️ **This is the only step that reads a plan.** The loop below runs on the
   backlog, because a plan is a hypothesis.

## The loop

```
until completion condition exits 0:
    next-task          → the top unblocked task
    spec check         → does it have acceptance criteria? if not, write them
    tdd                → failing test, watch it fail, implement, watch it pass
    deterministic gates→ scripts/check-crate.sh <crate>, coverage, layering
    review             → an agent that did not write it            ← never skip
    fix or argue       → blocking (and, on changes-requested, major) resolved;
                         minors recorded, not fixed-and-re-reviewed
    commit             → subject names the task ID
    tick the backlog   → with the commit reference
```

Run the completion condition after each commit.

## The outer loop

⚠️ **The inner loop above cannot see its own drift.** It reads one delta against
one task, so a convention quietly abandoned, two commits that contradict each
other, or a spec that should not have been written that way all pass it — every
individual step was faithful.

So at checkpoints, and always before the milestone is declared complete, run
[`milestone-review`](../milestone-review/SKILL.md) — ⚠️ **dispatching a fresh
agent, exactly as the inner loop does.** You drove these commits; you are the
one person who cannot see their drift. It reads the commits as a whole,
turn what is found into backlog tasks, amend the roadmap with what was learned,
and re-decompose what remains. `scripts/check-milestone-review.sh` is what stops
this from being a phase somebody has to remember.

⚠️ **Checkpoint rather than saving it for the end.** Nothing enforces the
cadence — reviewing thirty commits in one pass satisfies the gate and wastes it.

## ⚠️ Stop conditions

Stop and report. Do not work around, and do not pick a different task to avoid
the problem.

- **A gate fails and the fix is not obvious.** Three failed attempts is the
  ceiling; after that the problem is understanding, not effort.
- **The task turns out to need a decision** — a requirement is ambiguous, or two
  standards conflict. Decisions belong to the human.
- **The spec is wrong.** Amend it and say so; if the amendment changes an
  approach or a requirement, that is a decision.
- **A change would touch an `oqueue-core` trait** and the contract change was
  not planned. See [`adr`](../adr/SKILL.md) and the contract rule.
- **The completion condition passes but the milestone is obviously not done.**
  That is a bug in the condition and it is a finding, not a victory.
- **Anything destructive or outward-facing** — force push, deleting data,
  publishing. Never push unless asked.

## What must never happen in the loop

- ⚠️ **Never skip the review** because the change is small. The gate binds the
  review to the diff hash; a skipped review cannot be committed anyway, and
  attempting it wastes a cycle.
- ⚠️ **Never claim a gate passed without running it.** No script can catch this,
  which is exactly why it matters most here.
- **Never commit a tree you know is broken**, including "I will fix it next
  commit".
- **Never widen scope silently.** Something worth doing that is out of scope
  becomes a backlog task; that takes ten seconds.
- **Never lower a threshold or delete a test to make a check pass.**

## Reporting

At each stop, and at milestone completion, state plainly: what was done, what
was **not** done and why, and anything discovered that invalidates the plan. ⚠️
A task reported complete that is not complete is the most expensive failure mode
available here, because everything downstream is built on it.
