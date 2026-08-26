---
title: "Spec-driven development"
description: >
  Read when specifying work, decomposing a milestone, writing acceptance criteria, or when a spec turns out to be wrong.
tags: [process, specs, requirements, decomposition, traceability]
applies_to: ["docs/internal/product/*", "docs/internal/specs/*", "docs/internal/standards/*"]
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

### A plan is not a decomposition

⚠️ That rule is about the **backlog**, and it is easy to over-read into "no
forward planning", which would leave the project unable to say what it is
building. Two artifacts, and confusing them is the actual failure:

| | [`milestones/M<n>.md`](../product/milestones/) — the **plan** | [`backlog.md`](../product/backlog.md) — the **decomposition** |
|---|---|---|
| Covers | every milestone to v1 | the current milestone only |
| Authority | a working hypothesis | binding; gates read it |
| Task IDs | none — a plan's items are unnumbered | stable, cited by commits |
| Expected to change | **yes, and silently** | no; a change is a decision |
| Read by | whoever is deciding what to build next, and the `milestone` skill at decomposition — **both before the loop starts** | every gate, `next-task`, both review loops, and the `milestone` skill's loop |

A plan says *what this milestone is for, which requirements it serves, what must
be decided before its code, and roughly how much work it is*. That is answerable
years ahead and is what makes sequencing and dependency arguments possible.
A decomposition says *do this exact thing next*, and is only answerable once the
milestone before it has actually landed.

⚠️ **The `milestone` skill reads a plan exactly once — when it decomposes a
milestone that has no backlog rows yet — and never again.** Its loop runs on the
backlog and takes the completion condition from `roadmap.md`. A hypothesis has
no place inside an unattended loop; it has every place in the step that produces
what the loop will run.

⚠️ **Opening a milestone means re-deriving its tasks from the plan, not copying
them.** The plan's items are inputs. If they survive contact unchanged, the plan
was lucky; if they do not, that is the plan working as intended, and the
divergence is worth a line in the milestone's notes because it is evidence about
how far ahead this project can usefully see.

A plan item that turns out to be wrong is **not a spec that turned out to be
wrong** and needs none of that ceremony — nothing was committed to it.

A task is **one commit's worth**: one coherent change leaving the tree green. If
it cannot be finished that way, split it before writing code, not after
discovering it.

**A row is terse, and a `done` row is frozen** (ADR-0016). A row carries the
task, what it serves, an acceptance criterion, and a state. History lives in
the commit that closes it (`git log --grep <ID>`), so closing a row edits its
state cell and nothing else, and a later correction goes in the correcting
commit's message, never back into the row. **One owner per fact**: task state
lives here, cross-milestone obligations in `roadmap.md`'s deferral table,
history in commits — everything else links and never restates, and no count a
command can derive is written into prose. M1 is the measured argument: rows
grew to hundreds of words duplicating commit messages, and keeping the copies
consistent cost more review rounds than the code did.

⚠️ **One exception to the freeze, and M3 is why it exists.** An acceptance
criterion the code does **not** meet is corrected in place, in the commit that
corrects it, which names itself in the cell. Everything else about a `done` row
stays frozen. Without it two rules that are each right cannot both hold: the
freeze says a correction goes in the correcting commit's message, and `M3.19`
established that the backlog is the artifact a correction must land in, because
that is where `next-task`, the completion gate and every later task actually
read. M3's checkpoint review found a `done` row whose criterion its own code
inverts, and under the unamended rule there was no legal move. ⚠️ **An
acceptance criterion is a claim about the code, not a record of what someone
intended** — that is the asymmetry that makes this one case different from
every other edit the freeze forbids.

⚠️ **And it names its direction, because without one the cheaper repair is
always legal.** Correcting a criterion means one of two things and they are not
interchangeable: making the code meet it, or striking a claim the code will
never meet. Striking one is permitted **only if the correcting commit names
where the obligation went** — a row, or a `roadmap.md` deferral entry with a
receiving milestone. ⚠️ **A plan file is not a receiver**: this standard says a
plan is a working hypothesis, expected to change and silently, so an obligation
that lands only there has been downgraded rather than moved. Non-negotiable 2
states the same discipline for thresholds and names the weakening direction;
this is that rule for claims.

**A sweep is one row carrying several small fixes that share a theme** — a
review's unowned minors, a batch of stale claims — one commit, one review
(ADR-0016). It exists so small debt stops carrying full row ceremony each; it
is not licence to batch unrelated scope, which `git.md` rule 1 still forbids.

**A milestone carries at most 20 tasks.** Past that it is two milestones wearing
one name, its completion condition stops being a single coherent claim, and the
cross-cutting review at its boundary exceeds what one reader can hold. ⚠️ M-1 is
over this and is the evidence for the rule rather than an exception to it.

⚠️ **The number is a heuristic for that reasoning, not the reasoning itself, and
exceeding it costs a recorded argument** — in the backlog section's own notes,
saying why the rows are still one milestone. Two things forced this to be
written down. A milestone at exactly 20 has **no legal way to act on its own
boundary review**, since `review.md` rule 16 requires a major finding to name a
backlog row; M0 hit that at its checkpoint, and a decomposition rule that
forbids the outer loop from working is the rule being wrong. And the test is
whether the rows are *one milestone*, which the count cannot see: M0 is named
"Workspace, contracts, and quality gates", so its gate tail is the third thing
it is named for rather than a second milestone in disguise.

⚠️ **This is not licence to grow a milestone rather than finish it.** The
argument must be about coherence — a milestone whose extra rows are new *scope*
is the case the cap exists for, and the answer there is still to split.

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
