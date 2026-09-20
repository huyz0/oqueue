# 0052. Enforce the milestone task budget

Status: accepted; 2026-09-20
Requirements: none — this governs the development loop, not the product

## Context

The development standards already say a milestone carries at most 20 tasks,
but `check-milestone-exit.sh` only reported the planned count and the current
row count. Review findings could therefore keep adding work to the milestone
being reviewed. M4's boundary review opened 8, 10 and 7 rows across three
rounds; M5 grew from 20 planned rows to 65 documented rows, with 49 generated
by M5 itself. Mutation survivors and review debt were then carried as ordinary
milestone work, so the loop's completion time had no mechanical upper bound.

## Decision

`check-milestone-exit.sh` fails the commit when the milestone identified by the
current task commit is `in progress` and its backlog contains more than 20 task
rows. The failure directs the author to hand additional findings to the next
milestone before continuing. The cap counts all rows, including `done`,
`todo`, and `dissolved`, because the reviewer's reading budget is spent on the
milestone's history and decomposition, not only on unfinished work.

## Alternatives considered

- **Keep the limit advisory.** Rejected: M4 and M5 demonstrate that the loop
  grows without a bound when the only control is judgement.
- **Freeze the opening decomposition completely.** Rejected: a small number of
  genuinely observable or gate-correcting findings may still belong to the
  active milestone; the 20-row ceiling leaves that bounded correction space.
- **Put review findings in a side list.** Rejected: `next-task` reads the
  backlog, so an unreferenced side list would turn required work into a claim
  nobody acts on.

## Consequences

The active milestone has a hard review budget and must hand off further work
before the next implementation commit. This makes milestone duration bounded
by policy, but requires the reviewer to classify findings earlier and makes a
handoff a normal part of closing an over-budget decomposition. Historical
milestones remain readable because the check applies only to the current
in-progress milestone.
