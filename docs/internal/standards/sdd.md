---
title: "Spec-driven development"
description: >
  Read when specifying work, decomposing a milestone, writing acceptance criteria, or when a spec turns out to be wrong.
tags: [process, specs, requirements, decomposition, traceability]
---

# Spec-driven development

How work is specified, decomposed, and traced. The other standards in this
directory govern *code*; this one governs *the process that produces it*.

It applies to every turn, including autonomous ones.

## The chain

```
requirement (FR-n / NFR-n)     requirements.md — what must be true
  └── milestone                roadmap.md — completion condition is a command
       └── spec                what to build and how it will be checked
            └── task           backlog.md — one commit's worth
                 └── commit    subject names the task ID
                      └── test names the requirement it verifies
```

Every link is mandatory in both directions. **A spec citing no requirement is
unjustified work. A requirement reachable from no milestone is unscheduled
work.** Both are findings, not conditions to tolerate.

## Requirements

A requirement states **what must be true**, never how. It carries:

- a **stable ID** that is never reused, so a five-year-old commit still resolves;
- a **verification** — the test, gate, benchmark, or conformance suite that
  would prove it holds;
- a **status**: `agreed`, `provisional`, `underived`, or `deferred`.

Two rules do the real work:

1. **A requirement that cannot be verified is not a requirement.** If no
   mechanism could show it false, it is a preference and belongs in the mission
   statement or nowhere.
2. **A non-functional requirement without a number is not a requirement.** "Low
   latency" is a sentiment. ⚠️ Where the number is unknown, write **UNDERIVED**
   and name what blocks it. **Never invent one** — an invented number becomes an
   unexamined constraint as soon as somebody designs against it, and it is
   indistinguishable from a measured one a month later.

Changing an `agreed` requirement is a decision, and decisions get an ADR.

## Specs

A spec is written before implementation, and covers one milestone or one
coherent piece of one. It must contain:

| Section | Must answer |
|---|---|
| **Requirements** | Which FR/NFR IDs does this serve? |
| **Scope** | What is being built — and explicitly, what is not |
| **Design** | The approach, and the alternatives rejected with reasons |
| **Acceptance criteria** | How will we know it works? Each one executable or observable |
| **Risks** | What could make this wrong, and what would reveal it |
| **Tasks** | The decomposition, each one commit's worth |

**Acceptance criteria are the load-bearing section.** Each must be checkable by
something other than an opinion — a test that passes, a gate that goes green, a
number that lands inside a bound. "Works correctly" is not an acceptance
criterion. "A fetch at the high watermark issues zero GETs" is.

### Specs get reviewed before code

A wrong spec produces correct code solving the wrong problem, and every
downstream gate passes. **Spec review is therefore the cheapest gate available**
and the only one that catches this class.

Review a spec against the mission and the architecture, not against taste. The
questions worth asking are: does it serve the requirements it cites, does it
contradict a decision already recorded in `decisions/`, and are its acceptance
criteria actually checkable.

## Decomposition

**Only the current milestone is decomposed in detail.** Tasks written three
milestones ahead are wrong by the time they are reached — not because the plan
was bad, but because the intervening work changes what the right task is.
Future milestones stay as roadmap entries with a completion condition.

A task is **one commit's worth**: one coherent change leaving the tree green. If
it cannot be finished that way, split it before writing code, not after
discovering it.

Task IDs are stable. Completed tasks stay in the backlog with their commit
reference, because the record of why something was done is worth more than a
tidy list.

## Definition of done

A task is done when **all** hold:

- every acceptance criterion is met, and was **observed** to be met;
- the deterministic gates pass;
- an isolated reviewer has examined the diff and its findings are fixed or argued;
- the commit subject names the task ID;
- the backlog entry is ticked and carries the commit reference.

⚠️ **"I believe it works" is not done, and neither is "the code looks right."**
The distinction between having run something and expecting it to pass is the
one thing no gate can enforce, which is exactly why it is written here.

## When the spec turns out to be wrong

This is normal and expected. What matters is the response.

**Stop and amend the spec.** Do not silently implement something adjacent to
what was specified — that is the failure mode this whole structure exists to
prevent, because it produces work that passes every gate while serving no
requirement.

If the correction is small, amend the spec and say so in the commit. If it
changes an approach or invalidates a recorded decision, write an ADR. If it
changes a requirement, that is a decision too.

## Scope

**Never widen scope silently.** Doing more than the task asked is as much a
problem as doing less: it breaks the one-task-one-commit property, it makes the
diff harder to review, and it delivers work no requirement asked for.

Something worth doing that is out of scope becomes a backlog task. That takes
ten seconds and preserves the property that every change traces to a reason.

## Architecture decision records

Write an ADR when a choice is **expensive to reverse**, when it **changes a
contract**, or when a future reader would otherwise ask "why on earth is it done
this way".

An ADR captures the alternatives **while they are still live**. Six months on,
the code shows what was chosen and nothing shows what was rejected or why — and
the rejected options are the part people actually need, because they are what
stops the same debate being reopened.

An ADR that lists no rejected alternative is a description, not a decision.

## What this standard does not cover

- **Code-level rules** — see the other files in this directory.
- **The review mechanism** — how the isolated reviewer is invoked, what it is
  given, and how its verdict is bound to the diff — see
  [docs/researches/21](../../researches/21-ai-development-loop.md) §4–5.
- **Gate allocation** — what must be a script rather than an agent's judgement —
  see [docs/researches/21](../../researches/21-ai-development-loop.md) §3.
