---
name: review
description: Review a staged change as an independent agent that did not write it. Use before every commit. Defines what the reviewer is given, what it is deliberately denied, and what to look for that the deterministic gates cannot see.
---

# Review

⚠️ **This skill is run by an agent that did not author the change.** A
self-review inherits every blind spot that produced the defect — the same
misreading of the task, the same assumption about what a function guarantees,
plus a commitment bias toward work already done.

## Running it

```
scripts/review.sh context --task <ID>    the packet: task, standards, gates, diff
scripts/review.sh record --file v.json --task <ID>   validate and store it
scripts/check-reviewed.sh                the gate
```

⚠️ **`review.sh` does not spawn the reviewer**, because no script can do that in
a way that works across tools. It owns the hash, the packet, the schema, and the
artifact; the agent owns the judgement.

## What the reviewer receives

- The task, verbatim from the backlog, with its acceptance criteria
- The staged diff
- The relevant standards — selected from the staged paths by
  `scripts/which-standards.sh`, not chosen by the author
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

Return the verdict as JSON to
`scripts/review.sh record --file <path> --task <ID>`, which validates it and
writes `target/review/<staged-diff-sha256>.json`. The gate recomputes that hash,
so amending one byte after review invalidates it — which is what makes the
review a fact rather than a claim.

`record` refuses a verdict whose `diff_sha256` is stale, a finding with no
failure scenario, and a `pass` that coexists with a blocking finding. ⚠️ An
empty findings list is a valid and expected outcome; invented findings are worse
than none.

## Resolution

Each blocking finding is **fixed** (the diff changes, the hash changes, review
re-runs) or **argued** (an entry in `baselines/review.txt` naming the finding's
id and why it is not a defect).

⚠️ **A `changes-requested` verdict extends that to every `major` finding too.**
The gate refuses the commit until each one is fixed or argued — otherwise a
reviewer asks for changes, the commit lands anyway, and the findings exist only
in a gitignored directory. A reviewer who judges major findings non-blocking
says so by returning `pass`, which records them and warns.

⚠️ The entry must be **staged**. An unstaged one is ignored, because a line that
never reaches the commit suppresses a finding while leaving no trace of it.

⚠️ **On a `pass` verdict, a `minor` finding is recorded in the commit body and
the commit lands.** Nothing is staged for it — a staged row changes the hash and
re-runs this whole procedure, which is the loop the rule exists to stop. Fixing
it is permitted and usually wrong: the new round's surface is the prose the fix
just added. `review.md` rule 15.

⚠️ A growing argued-list is itself a signal: somebody is being systematically
overruled, and one side is systematically wrong.
