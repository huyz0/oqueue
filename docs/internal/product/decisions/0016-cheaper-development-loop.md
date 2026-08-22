# 0016. The loop spends prose faster than code: five repairs

Status: accepted
Date: 2026-08-23
Requirements: none — this governs the development loop, not the product

## Context

Measured on M1 (50 commits, 2026-08-16 → 2026-08-22, two of them
review-record bookkeeping):

- `M1.0`-`M1.19` — the seam, both backends, retry, governor, chunked
  reads — landed within the first **seven hours**. The last three product
  rows (`M1.20`-`M1.22`) landed on days five through seven, and 26 of the
  final day's 28 commits named no product row at all.
- Words added: ~44,000 of markdown and ~25,000 of commit messages against
  ~36,000 words of Rust (comments included). `backlog.md` alone reached
  ~69,000 words — more than half the size of the entire research corpus.
- Nineteen commits record 3-9 review rounds each. The closing milestone
  review's drift finding (`faf8f3631f9c` in
  `reviews/milestone-M1-4920f1685c9fde40.json`) named the pattern: rows
  took "three to nine rounds each", the per-commit reviewer was "spent
  fact-checking" prose, and *every gate in `scripts/` checks code, format
  or history; none checks whether a sentence about the tree is true*.
- Both real code defects the milestone shipped (`M1.53`, `M1.54`) were found
  by the **milestone** review, not by any of those per-commit rounds.

The causes, each observed repeatedly rather than inferred: facts written in
three to six places with review as the only consistency check; backlog rows
grown into narratives that duplicate commit messages and then need their own
maintenance; prose inaccuracy treated as blocking, so each fix re-opened
review on the prose the fix added; small debt carrying full task ceremony;
and ADRs kept current by rewriting their bodies, each rewrite a new reviewed
diff.

## Decision

Five rules, binding from `M2.1`. The amended standards state each where it
is enforced; this ADR owns the argument and the numbers above.

1. **One owner per fact.** Task state lives in `backlog.md`; cross-milestone
   obligations in `roadmap.md`'s deferral table; history in the commit that
   made it. Everything else links and never restates. No count derivable by
   a command is written in prose. → `sdd.md`
2. **Terse, frozen rows.** A row is a task, what it serves, an acceptance
   criterion, a state. A `done` row is frozen; corrections go in the
   correcting commit's message. → `sdd.md`
3. **A severity floor for prose, and a two-round cap.** A finding whose
   subject is a sentence is `minor` unless acting on it would change code
   behaviour or a live decision. Round one finds, round two verifies fixes;
   later non-blocking findings are recorded, not fixed inline. A blocking
   finding lifts the cap — correctness has no round limit. → `review.md`
4. **Sweep rows.** One row may carry several small fixes sharing a theme;
   one commit, one review. This also settles M0's open decision
   `bcf5d6f697f2`: the **milestone-boundary review harvests commit-body
   minors into sweep rows** — the step `M0.27`-`M0.29` and `M1.58` each
   performed by hand. → `sdd.md`, `review.md`, the `milestone-review` skill
5. **ADR staleness is a dated status line, never a body rewrite.** → the
   `adr` skill

## Alternatives considered

- **Keep M1's discipline unchanged** — rejected by the measurements above;
  the loop's cost grew while its defect yield moved to the milestone review.
- **Drop independent review** — rejected. The instrument works (`M1.53` and
  `M1.54` are real, reproduced defects it caught); the calibration was
  wrong, not the mechanism. Non-negotiables 3 and 4 stand untouched.
- **Gate prose truth with a script** — rejected as unbuildable in general:
  no script checks a claim against an intention (non-negotiable 3's class).
  The fix is fewer copies of each fact, not a checker for the copies.
- **Generate the backlog from commit history** — deferred, not rejected.
  Worth revisiting if rule 1 fails to hold in practice.

## Consequences

Easy: commits get cheap again; review attention lands where the defects
are; a small fix costs a small ceremony. Hard: less narrative inline — a
reader follows `git log --grep <ID>` for history, and loses the single-file
story `backlog.md` used to be. Foreclosed: nothing — every rule reverts by
superseding this ADR. The M1-era rows stay as they are: rewriting history
to the new format would be the exact churn this ADR exists to stop.
