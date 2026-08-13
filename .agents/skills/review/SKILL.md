---
name: review
description: Review a staged change as an independent agent that did not write it. Use before every commit. Defines what the reviewer is given, what it is deliberately denied, and what to look for that the deterministic gates cannot see.
---

# Review

⚠️ **This skill is run by an agent that did not author the change.** A
self-review inherits every blind spot that produced the defect — the same
misreading of the task, the same assumption about what a function guarantees,
plus a commitment bias toward work already done.

## What the reviewer receives

- The task, verbatim from the backlog, with its acceptance criteria
- The staged diff
- The relevant standards
- **The list of deterministic gates that already passed**

## What the reviewer must NOT receive

- The author's transcript, plan, or reasoning
- The author's description of what the change does
- Any justification the author produced

⚠️ **This exclusion is the point.** An author's rationale is persuasive by
construction — it was generated to make the change look correct — and a reviewer
given it grades the rationale instead of the code. Reconstruct intent from the
task and the diff alone, because *task says X, diff does Y* is precisely the
defect self-review cannot see.

## Do not re-check what the gates already checked

Formatting, clippy, compilation, test pass/fail, coverage, layering, sans-I/O,
`unsafe` placement, and public-API drift are **already decided by scripts**.
Spending attention there means missing what only a reader can catch.

## What to look for

**Conformance** — the questions no script can answer:
- Does the change do what the task specified — no more, and no less?
- Was scope silently widened? Something out of scope is a backlog task.
- Do the acceptance criteria actually hold, and is each one checkable?

**Tests:**
- Do they assert **behaviour**, or the implementation back at itself?
- Would any of them fail if the logic were subtly wrong? If not, they are
  coverage theatre.
- Is there a test for the failure path, not only the happy one?

**Design:**
- Is this the simplest thing that works, or invented structure?
- Does it duplicate something that already exists?
- Does it introduce a concept that contradicts an existing one?
- Is the abstraction at the right level?

**Correctness of claims:**
- Are the comments true?
- Does a `SAFETY:` comment's argument actually hold?
- Does the error message let an operator act?

**Against the standards** — the highest-severity items:
- Anything sized by client-supplied input has a bound
- No secret can reach a log, span, metric label, or error variant
- AEAD nonces constructed, never random
- Business logic touches no socket, clock, or object store

## Output

Structured findings, each with `file:line`, a severity, and **a concrete failure
scenario**. ⚠️ A finding that cannot say how it fails is a style opinion, and
style is the linter's job.

Write the verdict to `target/review/<staged-diff-sha256>.json`. The pre-commit
gate recomputes that hash, so amending one byte after review invalidates it —
which is what makes the review a fact rather than a claim.

## Resolution

Each blocking finding is **fixed** (the diff changes, the hash changes, review
re-runs) or **argued** (an entry in the review baseline naming the finding and
why it is not a defect). ⚠️ A growing argued-list is itself a signal: somebody is
being systematically overruled, and one side is systematically wrong.
