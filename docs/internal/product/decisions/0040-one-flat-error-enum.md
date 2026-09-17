# 0040. One flat error enum, and a size limit that does not measure a list

Status: accepted
Date: 2026-09-18
Requirements: none — this governs the code's structure, not the product

## Context

`oqueue-core`'s `Error` is 46 variants in one file, and `M5.5` needed two more
against a file already at 499 lines. `check-file-size.sh` gained its first
ALLOWLIST entry in that commit, granted at 523 lines, and `M5.42` was opened to
decide the thing the entry was deferring. ⚠️ **The row predicted its own
trigger**: "the next commit to add a variant hits the same wall", and `M5.8`'s
composite manifest needs three.

`code-structure.md` rule 18 says a long file is a design signal and to split it
by concept. There is no concept boundary *inside* one enum, so the split that
would work is per-domain sub-enums — `Error::Bundle(BundleError)`,
`Error::Storage(StorageError)` — with `#[from]` conversions.

## Decision

**The enum stays flat, and `error.rs` is exempt from rule 16's line limit as a
*list* rather than at a granted line count.**

1. ⚠️ **Splitting it would weaken a property the type documents.** `Error`'s
   own doc says it is deliberately not `#[non_exhaustive]` because "adding a
   variant breaks every exhaustive match, which is the compiler doing the
   review". With sub-enums, a caller matching `Error::Bundle(_)` without
   descending keeps compiling when a bundle variant is added — so the compiler
   stops doing the review for exactly the callers that chose to handle a domain
   coarsely. Splitting a type to satisfy a **size** gate, at the cost of a
   safety property, is what non-negotiable 2 forbids in spirit even though the
   number being moved is not a threshold.
2. ⚠️ **Rule 16's limit measures a module you can hold in your head.** A file
   that is one flat list — a generated table, an enum of independent variants —
   is not that, and rule 17 already names generated tables as the legitimate
   case. A 46-entry list is not a file anyone reads top to bottom; it is a file
   people grep.
3. ⚠️ **So the exemption is structural, not numeric.** A granted line count
   would have to be raised by the commit that adds the forty-seventh variant,
   and by the one after that — which is the churn `M5.42` was opened by, and
   which makes every raise indistinguishable from the raise that is really
   somebody giving up. Instead `check-file-size.sh` requires an exempt list to
   hold **exactly one top-level item and no module declaration**. The moment
   the file grows an `impl`, a helper or a second type, it is a module again
   and the limit applies to it.
4. The crate's `Result` alias moves to `result.rs`, because it was the second
   item and it is not what made the enum long.

## Alternatives considered

- **Per-domain sub-enums with `#[from]`.** Rejected on point 1. It also changes
  every construction site in the workspace, which is a large diff whose only
  motivation would be a line count.
- **Raise the granted count to fit.** Rejected: raising a limit so that one
  particular file fits is the move that makes every limit advisory, and this
  one would be raised again three variants later.
- **`#[non_exhaustive]` plus a split.** Rejected twice over — it is the
  attribute `Error`'s doc rejects, for the reason quoted in point 1.
- **Shorten the variant docs.** The docs are where each failure mode's
  reasoning lives; deleting them to satisfy a line count is the worst trade on
  offer.

## Consequences

- Adding a variant to `oqueue-core::Error` needs no allowlist edit and no row.
  `M5.8` is the first commit to benefit.
- ⚠️ **`error.rs` can now grow without a number noticing**, and that is the
  cost, stated rather than hidden. What replaces the number is the structural
  condition and the fact that every variant still breaks every exhaustive
  match in the workspace — a variant nobody needs does not land quietly.
- A second file could take this exemption, and the same condition would govern
  it. The granted-count form stays for genuine deferrals, which are a different
  thing: a file that is over the limit and should not be.
- Forecloses: nothing. If the enum is ever split, the entry comes out and the
  condition stops mattering.
