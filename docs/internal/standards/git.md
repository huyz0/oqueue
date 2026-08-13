---
title: "Git operations"
description: >
  Read before committing, when unsure whether a change is one commit or several, or before amending anything already pushed.
tags: [process, commits, atomicity, bisect, main]
applies_to: ["*"]
---

# Git operations

Commits go directly to `main`. There are no feature branches, no pull requests,
and no merge commits.

⚠️ **That only works because every commit is small, green, and atomic** — the
rules below are what make it true rather than aspirational. Without them,
direct-to-main is just an unreviewed trunk.

Each rule names its gate, or is marked as having none.

## Why atomicity matters here

With no human reading every diff, **`git bisect` is the substitute for a
reviewer.** It works only if every commit builds and passes, and it is useful
only if each commit contains one change — bisecting to a commit that did five
things tells you almost nothing.

A bad commit is **reverted, not merged around**. That is cheap when commits are
atomic and nearly impossible when they are not.

## Atomicity

1. **One task equals one commit equals one coherent change that leaves the tree
   green.** → `check-commit-msg.sh` holds the traceable half; the hooks are what
   "green" means
2. **Split before writing code, not after discovering.** If a task cannot be
   finished in one green commit, it was decomposed wrong — fix the backlog.
3. ⚠️ **Never mix a refactor with a behaviour change.** This is the most common
   way an atomic commit stops being one: the diff becomes unreviewable, and a
   bisect landing on it cannot tell you which half broke. Rename in one commit,
   change behaviour in the next.
4. **Mechanical churn travels alone.** Formatting, a bulk rename, a dependency
   bump — each is its own commit, so the commits that carry meaning stay
   readable.
5. **Never commit a tree you know is broken**, including "I will fix it in the
   next commit". There is no next commit from the perspective of a bisect.
6. **Know what is staged before committing.** `git diff --cached` is the thing
   being committed and the thing being reviewed. ⚠️ An unrelated file swept into
   the stage breaks atomicity *and* changes the diff hash the review is bound
   to.

## Subject

7. **`<task-id>: <what changed>`** — the ID resolves to the backlog, the text
   says what this commit produced. → `check-commit-msg.sh`
8. **The ID must exist in the backlog.** A regex alone accepts `M-9.99`, and a
   link to a task nobody wrote is worse than no link, because it reads as
   traceable.
9. **Say what changed, not that something did.** ⚠️ `wip`, `fix`, `update`,
   `changes`, `cleanup`, `refactor` as the first word are rejected — they are
   the words that survive when nobody decided what to write. → the denylist in
   `check-commit-msg.sh`
10. **Both a noun phrase and an imperative are fine** — "the gate plumbing and
    the commit-message rule" and "correct the corpus document count" are both
    specific, which is the property that matters. Grammatical mood is not worth
    a rule; specificity is.
11. **Keep it under 72 characters** so `git log --oneline` stays readable. →
    warning, not a failure
12. **One ID normally.** Several are accepted so a commit closing two genuinely
    inseparable tasks stays traceable, but ⚠️ it is a smell: rule 1 still says
    one task, one commit.

## Body

13. **The body explains *why*.** The diff already shows what. A body that
    narrates the diff has spent the reader's attention and told them nothing.
14. **Record what was tried and rejected**, when it is not obvious. The next
    person to touch this — including a future agent with none of your context —
    will otherwise try it again.
15. **Say what is *not* done**, and why. ⚠️ A commit that implies completeness it
    does not have is the most expensive kind of message here, because everything
    downstream is built on it.
16. **Wrap at 72 characters.**
17. **A finding that corrects an earlier belief belongs in the body**, not only
    in a document. The commit is where someone lands when they run `git blame`.

## Trailers

18. **`Removes-test: <reason>`** is required when a test disappears. →
    `check-tests-kept.sh`
19. **`Co-Authored-By:`** records AI authorship. Every commit here carries one;
    that is a fact about the project, not decoration.
20. **Reference an ADR by number in the body** when the commit implements a
    decision.

## Amending, reverting, rewriting

21. **Amend freely before pushing.** It is the cleanest way to fix a message or
    fold in a review finding.
22. ⚠️ **Never amend or rebase anything already pushed.** `main` is public and
    rewriting it breaks every clone. A pushed commit with a wrong message gets a
    follow-up commit naming the same task ID and saying what it corrects.
23. **Never force-push `main`.** There is no situation in this project's normal
    operation that requires it.
24. **Revert rather than fix-forward-and-hide.** A revert is honest history; a
    quiet corrective commit that does not say what it undoes is not. Revert
    subjects are exempt from the ID rule, since the reverted commit already
    carries one.
25. **Never push unless asked.** Committing is routine; publishing is not.

## Branches

26. **Commits go directly to `main`.** Branching happens only on explicit
    request.
27. **No merge commits.** A merge on `main` means the workflow was bypassed. →
    `check-commit-msg.sh` warns rather than fails, because refusing it in the
    hook would leave no way to record that it happened.
28. **No gate may be triggered by a pull request.** With no PRs, such a gate
    silently never runs — the failure mode that is hardest to notice because
    everything stays green. → `check-drift.sh`

## What has no gate

**Whether a commit is genuinely atomic.** A script can check that the tree is
green and the subject is well-formed. It cannot tell that a commit did a
refactor and a behaviour change together — that is a review judgement, and it is
worth spending review attention on, because it degrades bisect for everyone
afterwards.

**Whether the body is honest about what is unfinished.** No script reads intent.
This is the same class as *never claim a test passes without having run it*, and
it rests on the same discipline.

## See also

- The task→commit chain: [sdd.md](sdd.md)
- Commit-message rule as implemented: `scripts/check-commit-msg.sh`
