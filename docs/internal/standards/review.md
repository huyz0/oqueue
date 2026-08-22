---
title: "Review"
description: >
  Read when writing a review prompt, deciding whether a finding blocks a commit, or wondering why the reviewer was not given the author's reasoning.
tags: [process, review, reviewer, escalation, isolation]
applies_to: ["*"]
---

# Review

No human reads this code. That is the premise every rule below exists to
answer: without a person catching what a script cannot, "reviewed" would mean
nothing more than "an agent produced it and moved on." See
[docs/researches/21](../../researches/21-ai-development-loop.md) §3–6 for the
argument this standard turns into rules.

Two review loops exist and both follow the same shape: a change goes to a
reviewer that did not produce it, and a deterministic gate refuses to let the
change land until a matching verdict exists. **Per-commit** review (this
document's main subject) checks one diff against one task. **Milestone**
review — the [`milestone-review`](../../../.agents/skills/milestone-review/SKILL.md)
skill, `scripts/milestone-review.sh`, `scripts/check-milestone-review.sh` —
checks a milestone's commits as a whole, for what only exists across them.
Everything below except "Allocation" applies to both; where they differ, the
milestone loop is called out.

## Allocation

1. **Anything a script can decide must never be delegated to an agent.**
   Formatting, compilation, test pass/fail, coverage against a constant,
   layering, sans-I/O, `unsafe` placement, commit-subject shape — each of
   these has a gate in this repository or a task that will add one. Doc 21
   §3.1 has the worked table; it is not repeated here because a table that can
   drift out of sync with the actual gate set is worse than no table.
2. **The reviewer's budget is spent only on what no script can reach.** The
   review packet names every deterministic gate that already passed, so the
   reviewer does not re-derive them. → `scripts/review.sh context` includes
   this list; `scripts/which-standards.sh` selects the standards, not the
   author.
3. ⚠️ **A recurring semantic finding is a bug report against the gate set.**
   The correct response to the same class of defect surviving review twice is
   to write a script that catches it the first time, not to trust the
   reviewer harder. A semantic category that stops needing a human is a
   permanent gain — it becomes cheap, repeatable, and provable.

## Isolation

4. **The reviewer is a separate subagent with an isolated context** — not
   merely a separate invocation. What matters is separation of *information*,
   not separation of process.
5. **The reviewer receives:** the task verbatim from the backlog with its
   acceptance criteria, the staged diff, the standards selected by path, and
   the list of gates already passed. ⚠️ A diff that edits a standard obliges
   the reviewer to read that standard **in full** — edited and unedited parts
   alike. Standards deliberately do not route to themselves (`M2.6`:
   `applies_to` means "governs", and a standard does not govern itself;
   `which-standards.sh`'s header carries the reasoning), so this sentence is
   the mechanism that puts the edited standard in front of its reviewer.
6. **The reviewer must never receive:** the author's transcript, plan, or
   reasoning; the author's own description of what the change does; any
   justification the author produced. ⚠️ This exclusion is the rule that
   carries the weight. An author's rationale is persuasive by construction —
   generated to make the change look correct — and a reviewer given it grades
   the rationale instead of the code. The defect self-review cannot see is
   *task says X, diff does Y*, and reconstructing intent from the task and the
   diff alone is the only way to find it.
7. **Adversarial framing.** The prompt asks what is wrong, never whether the
   change is acceptable — the second question reliably returns approval.
8. **A different model where practical.** Author and reviewer sharing a model
   share blind spots; decorrelating them costs nothing once the harness
   supports it.
9. **Every finding is structured**: `file:line`, a severity, and a concrete
   failure scenario. ⚠️ A finding that cannot say how it fails is a style
   opinion, and style is the linter's job, not the reviewer's.

9a. ⚠️ **A severity floor for prose** (ADR-0016). A finding whose subject is a
   sentence — a comment, a doc, a backlog row, a plan — is `minor` unless
   acting on it would change code behaviour or a live decision. ADR-0016
   owns the measured argument: M1's prose-heavy rows took three to nine
   review rounds each, while both real defects that milestone shipped were
   found by the milestone review, not those rounds. Rule 12a still holds — an
   unmeasured tool claim stays a legitimate finding — this rule is about
   what a prose finding may *block*.

## Non-skippable

10. **A review is a claim until a gate checks the artifact it produced.**
    `scripts/review.sh` hashes the staged diff, spawns the reviewer with the
    §Isolation context, and writes `target/review/<hash>.json`.
    `scripts/check-reviewed.sh` recomputes the hash and refuses the commit
    unless a matching artifact exists with zero unresolved blocking findings.
    Amending one byte after the review changes the hash, so satisfying the
    gate requires an actual review of the actual bytes being committed — the
    property non-negotiable 3 cannot have for test claims, obtained here for
    review claims. → `scripts/check-reviewed.sh`
11. **The milestone loop is the same pattern at a different scope.**
    `scripts/milestone-review.sh` writes a verdict keyed to the set of commits
    it read; `scripts/check-milestone-review.sh` refuses a milestone's
    completion while any of its commits — except one whose every changed path
    is the verdict itself — is not named by some artifact. → `scripts/check-milestone-review.sh`
12. ⚠️ **Neither gate can verify who reviewed, or that the review was any
    good.** The `reviewer` field is a string nothing checks, and a reviewer
    can read nothing and return an empty findings list. What the hash buys is
    that a review of the *actual* diff happened; what it cannot buy is that
    the reviewer was not the author, or that the reading was careful. That is
    bought by the harness — dispatching an agent that genuinely was not the
    author, with genuinely isolated context — and saying so plainly is the
    same discipline non-negotiable 3 already asks of test claims.

## Claims

12a. ⚠️ **A claim about compiler, cargo, or tool behaviour is measured before it
    is written, or it is not written** — in a comment, a commit body, an ADR, a
    standard, or a research document alike. Three were written in this project
    and were wrong, each caught in review: "edition 2024 implies resolver 3"
    (false for a virtual manifest), "`--all-targets` covers doc tests" (it
    excludes them), "`release` and `bench` evict each other" (they coexist).
    Each was plausible, each cost a review round, each took under a minute to
    check. If measuring is not worth the minute, the sentence is not worth
    writing. ⚠️ **Stated here rather than in `rust-style.md`** because two of
    the three defects above lived in a shell script and in a research document.
    Verified: `which-standards.sh` on those two paths selects this file and not
    `rust-style.md`, so this is the standard a reviewer of either is given.

## Fix or argue

13. **A blocking finding is resolved exactly one of two ways: fixed, or
    argued.** Fixed means the diff changes, the hash changes, review re-runs.
    Argued means an entry lands in `baselines/review.txt` naming the finding
    and the reason it is not a defect, and the entry must be **staged** — an
    unstaged one is ignored, because a line that never reaches the commit
    suppresses a finding while leaving no trace it existed.
14. **A `changes-requested` verdict extends the same rule to every `major`
    finding.** A reviewer who judges a major finding non-blocking says so by
    returning `pass` — which still records the finding and warns, but does
    not require it to be fixed or argued before the commit lands.
15. ⚠️ **A `minor` finding on a `pass` verdict is recorded, not re-reviewed.**
    Record it in the **commit body**, which is not hashed and is therefore
    free. ⚠️ Nothing else, and deliberately: staging a backlog row would change
    the hash and force the re-review this rule exists to prevent. If the minor
    is work worth scheduling, that is a decision and a later commit. Fixing it is
    permitted and usually wrong: the hash changes, review re-runs, and the new
    round's surface is the prose the fix just added.

15a. ⚠️ **Two rounds is the cap** (ADR-0016). Round one finds; round two
    verifies the fixes and may fail them. A non-blocking finding first raised
    in round three or later is recorded per rule 15, not fixed inline. A
    **blocking** finding lifts the cap — correctness has no round limit — and
    a reviewer who keeps finding new blocking defects past round two is
    describing a change that should be withdrawn and re-cut, not re-polished.

    ⚠️ **This rule and rule 16 were in tension, and M0's boundary review said
    so rather than resolving it** (finding `bcf5d6f697f2`): a commit body is a
    place nothing reads, and M0 recorded upwards of thirty minors there that
    became rows only when `M0.27`-`M0.29` harvested them by hand.
    ⚠️ **Settled by ADR-0016**: the **milestone-boundary review harvests
    commit-body minors into sweep rows** (`sdd.md`) — the step M0 and `M1.58`
    each performed by hand is now the procedure's. Within a milestone, a minor
    worth scheduling sooner is still written as a backlog row in the **next**
    commit. ⚠️ **Rule 13's
    "fixed means review re-runs" governs blocking findings only**; reading it
    onto minors is what turns one review into five; M0.3's commit body records
    that count.
16. **The milestone loop's argued baseline is the same file, the same
    discipline**, with one addition: a blocking or major finding must name a
    backlog `task_id` that exists, or be argued the same way — a finding that
    lives only in a review artifact is one nothing will act on, since
    `next-task` and everyone else reads the backlog, not `reviews/`.
17. ⚠️ **A growing argued list is itself a signal.** Reviewed at milestone
    boundaries, a `baselines/review.txt` that only grows means a reviewer is
    being systematically overruled, and one side of that is systematically
    wrong. Nobody may grow it quietly — the same discipline `testing.md`
    already states for the mutants baseline.
