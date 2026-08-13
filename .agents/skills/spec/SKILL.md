---
name: spec
description: Write a spec before implementing, and decompose it into commit-sized tasks. Use when starting a milestone, when a task lacks acceptance criteria, or when what to build is clearer than how it will be checked.
---

# Spec

Written before implementation. Covers one milestone or one coherent piece.

## Contents

| Section | Must answer |
|---|---|
| **Requirements** | Which FR/NFR IDs does this serve? |
| **Scope** | What is being built — and explicitly, what is not |
| **Design** | The approach, and the alternatives rejected with reasons |
| **Acceptance criteria** | How will we know it works? |
| **Risks** | What could make this wrong, and what would reveal it |
| **Tasks** | The decomposition, each one commit's worth |

## Acceptance criteria carry the weight

Each must be checkable by something other than an opinion — a test that passes,
a gate that goes green, a number inside a bound.

- ❌ "Produce works correctly"
- ✅ "A produce request is acked only after the PUT succeeds; a kill injected
  between PUT and ack loses no acked record"
- ❌ "Fetch is fast"
- ✅ "A fetch at the high watermark issues zero GETs"

⚠️ If you cannot write a checkable criterion, you do not yet understand the
task well enough to implement it. That is the useful signal, not an obstacle.

## Decomposition

- **Only the current milestone**, in detail. Tasks written three milestones
  ahead are wrong by the time they are reached — not because the plan was bad,
  but because the intervening work changes what the right task is.
- **One task equals one commit** — one coherent change leaving the tree green.
- **Stable IDs**, never reused.
- Each task cites the requirement it serves.

## Get the spec reviewed before writing code

⚠️ **A wrong spec produces correct code solving the wrong problem, and every
downstream gate passes.** Spec review is the cheapest gate available and the
only one that catches this.

Review against the mission and the architecture: does it serve the requirements
it cites, does it contradict a recorded decision, are its criteria checkable.

## If it turns out to be wrong

Normal, and expected. **Stop and amend the spec.** Do not silently implement
something adjacent — that produces work passing every gate while serving no
requirement. If the correction changes an approach or a requirement, it is a
decision: write an [`adr`](../adr/SKILL.md).
