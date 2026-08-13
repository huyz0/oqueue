---
name: next-task
description: Choose what to work on next and confirm it is genuinely ready. Use at the start of any working session, after finishing a task, or when unsure what to do. Prevents starting work that is blocked, unspecified, or already done.
---

# Next task

## Choose

1. Read `docs/internal/product/backlog.md`. Only the current milestone is
   decomposed; if it is not, decompose it before writing code.
2. Take the **top unblocked task** — top, not the most interesting one. Order is
   dependency order and was chosen deliberately.
3. A task is **blocked** if it depends on an unfinished task, on an UNDERIVED
   requirement, or on a decision nobody has made. Blocked tasks are skipped, and
   ⚠️ **if the top three are all blocked, stop and report** — that is a planning
   problem, not a work problem.

## Confirm before starting

- **Does it cite a requirement?** A task serving no FR/NFR is unjustified work.
- **Does it have acceptance criteria, and are they checkable by something other
  than an opinion?** If not, write them first — see [`spec`](../spec/SKILL.md).
- **Is it one commit's worth?** One coherent change leaving the tree green. If
  not, split it now rather than discovering it half-way.
- **Has it already been done?** Check the backlog state and `git log`.

## Then

Implement with [`tdd`](../tdd/SKILL.md). Do not start editing before the
acceptance criteria exist — that is how scope drifts.
