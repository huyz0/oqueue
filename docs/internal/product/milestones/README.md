---
title: "Milestone plans"
description: >
  Read when opening a milestone, or when deciding what the project builds next. One plan per milestone; the current milestone's authoritative tasks live in backlog.md, not here.
tags: [product, milestones, planning]
---

# Milestone plans

One file per milestone, M-1 through M15, ordered by execution in
[`../roadmap.md`](../roadmap.md) — that file is the index and carries every
milestone's completion condition; this directory is the detail.

⚠️ **Only [M-1](M-1.md) exists so far.** The rest land in M-1.41 through
M-1.43, and are deliberately not linked until they do.

## ⚠️ A plan is not a decomposition

This is the distinction that makes forward planning compatible with
[`sdd.md`](../../standards/sdd.md)'s rule that only the current milestone is
decomposed. Confusing the two is the failure this file exists to prevent.

| | a plan, here | [`backlog.md`](../backlog.md) |
|---|---|---|
| Covers | every milestone to v1 | the current milestone only |
| Authority | a working hypothesis | binding; gates read it |
| Task IDs | none | stable, cited by commits |
| Expected to change | **yes, and silently** | no; a change is a decision |

**Opening a milestone means re-deriving its tasks from the plan, not copying
them.** The plan's items are inputs to that decomposition. If they survive
unchanged the plan was lucky; if they do not, the plan worked — and the
divergence belongs in the milestone's notes, because it is the only evidence
this project will ever have about how far ahead it can usefully see.

A plan item that turns out to be wrong is **not a spec that turned out to be
wrong**, and needs none of that ceremony. Nothing was committed to it.

## What every plan states

1. **Goal** — one paragraph. What is true after this milestone that was not
   true before, in terms someone outside the project would recognise.
2. **Kind** — one of `crate delivery`, `functional`, `feature`, `build`,
   `AI-native development support`, `non-functional`. It decides how the
   milestone is verified; see the table in [`../roadmap.md`](../roadmap.md).
3. **Requirements served** — FR/NFR IDs from
   [`../requirements.md`](../requirements.md). ⚠️ A milestone serving none is
   unjustified and should be cut.
4. **Depends on** — the milestones that must land first, and why.
5. **Decisions required first** — the ADRs that must be written before this
   milestone's code, each naming the open question it closes. ⚠️ **A milestone
   that starts coding before its decisions are recorded acquires an
   architecture nobody chose.**
6. **Provisional tasks** — at most 20, each one commit's worth.
7. **Completion condition** — a command. If "done" cannot be an exit code, the
   milestone is not stated correctly yet.
8. **Risks and open questions** — what could invalidate the plan, and which
   numbers do not exist yet.

## How a plan is consumed

```
scripts/gates/m<n>-complete.sh              the previous milestone's own condition
scripts/check-milestone-review.sh           …and that its commits were all read
<read this milestone's plan>                goal, decisions, provisional tasks
the `adr` skill                             write the decisions it requires
the `spec` skill                            re-derive the tasks into backlog.md
the `milestone` skill                       drive them, one task per commit
```

⚠️ **The `milestone` skill's step 3 is where a plan is read.** That skill
decomposes a milestone that has no backlog rows yet, and *that* step opens this
directory — the plan's items, and the ADRs it says must come first, are inputs
to the decomposition. What the skill must never do is let a plan item reach the
loop without becoming a backlog row.

⚠️ **The first two are different questions and both are required.** The
completion condition asks whether the work is done; the second asks whether its
commits were read as a whole. Neither implies the other.

⚠️ **Use the gate, not `milestone-review.sh coverage`.** `coverage` is a report
and **exits 0 even when every commit is uncovered** — it prints "25 not yet
reviewed" and succeeds. `check-milestone-review.sh` is the one that fails. In a
project whose idiom is "done is an exit code", reaching for the reporting
subcommand here is how a boundary review gets skipped while looking discharged.

⚠️ The first five steps happen **before** the loop starts. The
[`milestone`](../../../../.agents/skills/milestone/SKILL.md) skill then drives
the decomposed backlog and reads the completion condition from
[`../roadmap.md`](../roadmap.md). ⚠️ **Its loop never reads a plan** — a plan is
a hypothesis, and an unattended loop must run on committed tasks. Reading one
happens once, at decomposition, before the loop starts.

## ⚠️ What a plan is not evidence of

It is not evidence the work is possible, correctly sized, or correctly ordered.
**Five architecture decisions are unmade**, blocking four of the seventeen
milestones, and four more have completion conditions that cannot yet be written
because the requirement they check has no number. Both are listed in
[`../roadmap.md`](../roadmap.md) rather than smoothed over, because a plan that
reads as more settled than it is will be trusted more than it should be.
