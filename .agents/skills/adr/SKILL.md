---
name: adr
description: Write an architecture decision record. Use when making a choice that is expensive to reverse, when changing an oqueue-core contract, when a requirement changes, or when a future reader would otherwise ask "why on earth is it done this way".
---

# Architecture decision record

An ADR captures **why**, at the moment the alternatives are still live. Six
months later the code shows what was chosen and nothing shows what was rejected
— and the rejected options are the part people actually need, because they are
what stops the same debate reopening.

## When

- The choice is **expensive to reverse** — a format, a protocol, a dependency at
  the centre of things.
- It **changes an `oqueue-core` contract**. Required, not optional, and it
  travels in the same commit as the trait change.
- It **changes an `agreed` requirement**.
- A future reader would ask "why on earth".

Not for choices that are cheap to undo. An ADR for everything is an ADR for
nothing.

## Format

`docs/internal/product/decisions/NNNN-short-title.md`:

```markdown
# NNNN. Title

Status: proposed | accepted | superseded by NNNN
Date: YYYY-MM-DD
Requirements: FR-n, NFR-n

## Context
What forced a decision. The constraints, with numbers where they exist.

## Decision
What was chosen, stated so someone can act on it.

## Alternatives considered
Each one, and **why it was rejected**. This section is the reason the file exists.

## Consequences
What this makes easy, what it makes hard, and what it forecloses.
```

⚠️ **An ADR listing no rejected alternative is a description, not a decision.**
If nothing else was seriously considered, either say so explicitly — that is
information — or reconsider whether this needed an ADR.

## Numbering

Sequential, never reused. Superseding does not delete: mark the old one
superseded and link forward, because the reasoning that was later overturned is
often the most instructive thing in the directory.

## Staleness

When the tree diverges from an accepted ADR without overturning the decision,
record it as a **dated note on the `Status:` line** — e.g. `Status: accepted;
2026-08-23: consequence 2 overtaken, see M2.10` — never by rewriting the body
(ADR-0016). A body rewrite is a new reviewed diff, erases what was believed at
decision time, and is how four of M1's ADRs went stale twice.
