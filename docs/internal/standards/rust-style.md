---
title: "Rust style"
description: >
  Read when naming a type or function, choosing between a generic and `impl Trait`, deciding what a lint attribute or `clippy.toml` entry should say, or when a diff is hard to read for reasons `code-structure.md` doesn't cover.
tags: [code, style, naming, lints, idioms, formatting]
applies_to: ["*.rs", "Cargo.toml", "*/Cargo.toml", "clippy.toml", "rustfmt.toml"]
---

# Rust style

How code reads, not how it's organized into files — that's
[code-structure.md](code-structure.md) — and not what it does with an error
or a trait — those are [error-handling.md](error-handling.md) and
[contracts.md](contracts.md). This is naming, formatting, and lint
configuration.

## Formatting is not a decision

1. **`cargo fmt` is authoritative.** No manual reformatting, no arguing
   about brace placement in review — if `rustfmt` produced it, it's correct
   by definition, and a preference that disagrees with `rustfmt` is not a
   finding. → `cargo fmt --check` in CI
2. **The default `rustfmt` profile**, no project-local `rustfmt.toml`
   overrides, unless a specific rule below needs one. A style config that
   diverges from the ecosystem default is a tax every contributor and every
   agent trained on idiomatic Rust pays silently, forever.

## Lints are declared once

3. **Workspace lints live in `[workspace.lints]` in the root `Cargo.toml`,
   and every crate opts in with `lints.workspace = true`.** Not a
   `#![warn(...)]` block repeated at the top of every crate's `lib.rs` —
   that is the same "two places holding one fact" hazard
   [`M-1.33`](../product/backlog.md) already names for generated indexes,
   applied to lint configuration. → `scripts/check-layering.sh`, which reads
   every crate manifest already (`M0.21`)
4. **`clippy::pedantic` and `clippy::nursery` are denied workspace-wide.**
   An allowance is a named exception with a reason in the lint table, not a
   silent `#[allow]` at the call site — a silent one is indistinguishable
   from an oversight the next time someone reads that function.
5. **`missing_debug_implementations = "warn"`, `unreachable_pub = "deny"`,
   `unused_qualifications = "warn"`.** The first backs
   [contracts.md](contracts.md) rule 5; the second backs
   [code-structure.md](code-structure.md) rule 12.
6. **`clippy::unwrap_used` and `clippy::expect_used` are denied outside
   `#[cfg(test)]`** — already stated as [code-structure.md](code-structure.md)
   rule 26; restated here because it is a lint-configuration fact as much as
   a behavioral one.
7. **A threshold in `clippy.toml` is a constant, not a knob.** Raising
   `too-many-lines-threshold` or `cognitive-complexity-threshold` because a
   function grew past it is the exact move non-negotiable 2 forbids for any
   other gate — the fix is splitting the function, and the ADR, if any, is
   about why the *default* was wrong, never about this one function.
   → `scripts/check-drift.sh`'s pin map (`M2.9`) holds the values on the
   commit path.

## Naming

8. **A Kafka wire-protocol type is named for the wire spec's own name**,
   even where it reads oddly next to the rest of the codebase —
   `RecordBatch`, not `Batch`; `FetchRequest`, not `FetchReq`. A reader
   cross-referencing the Kafka protocol documentation should find the same
   name, not a Rust-idiomatic paraphrase they have to mentally translate.
9. **Everything that is not wire-protocol-facing uses plain Rust
   convention**: `snake_case` for functions and modules, `UpperCamelCase`
   for types, `SCREAMING_SNAKE_CASE` for constants, no Hungarian prefixes
   (`m_`, `str_`), no type name baked into a variable name the compiler
   already knows the type of.
10. **A boolean parameter or field is named so the call site reads as a
    sentence** (`retry_on_throttle: bool`, not `flag: bool`) — a positional
    `true`/`false` at a call site with no named-argument syntax is a bug
    waiting for the day the parameter order changes.
11. **An abbreviation is used only where the full word would be noise at
    every call site** (`idx`, `len`, `cfg` are fine; `ObjectStoreImpl`
    abbreviated to `OSI` is not). When in doubt, spell it out — an agent
    reading this code has no shared hallway convention to fall back on.

## Idiom

12. **No `println!`, `eprintln!`, or `dbg!` in library code.** Structured
    logging via `tracing` is the only sanctioned output path once a crate
    is past prototype; see [behavior.md](behavior.md) for what the log
    schema itself must look like. `bin/oqueue` may use them only before
    `tracing` is initialized, and never after.
13. **`impl Trait` in argument and return position by default.** Fall back
    to a named generic only when the caller needs to name the concrete type
    or the function needs to add a bound elsewhere that `impl Trait`
    can't express — and say why in a comment when that happens, since it's
    the exception.
14. **Prefer iterator combinators to a hand-rolled loop with a mutable
    accumulator**, except where [performance.md](performance.md) already
    requires the hand-rolled form on a named hot path — the two rules can
    conflict, and performance wins on the specific paths it names, nowhere
    else.
15. **No wildcard imports (`use foo::*`)** outside `#[cfg(test)]` modules and
    generated protocol tables where the alternative is hundreds of named
    imports for symbols with no ambiguity risk.
16. **A `match` over an enum this workspace defines has no catch-all `_ =>`
    arm.** An exhaustive match makes the compiler the regression test for a
    new variant; a catch-all silently absorbs it. The named exception is an
    enum whose source of truth is external and forward-compatible by
    design — an unrecognized Kafka API key, for instance — where "unknown"
    is a real, intended case rather than a missed one.
17. **Every `pub` item has a doc comment.** `# Errors` and `# Panics`
    sections are required wherever the function returns `Result` or has a
    documented panic condition; see
    [error-handling.md](error-handling.md) rule 1 for what "documented
    panic condition" is allowed to mean in this codebase (essentially
    nothing reachable from outside this crate's own invariants).

## What has no gate

**Whether a name is actually clear**, as opposed to merely compliant with
rules 8–11. A script can check casing; it cannot check whether
`resolve_pending` means what the reader thinks it means. That is review.

**Whether an `impl Trait` vs. generic choice (rule 13) was made for the
stated reason or out of habit.** Not mechanically distinguishable from the
diff alone.

## See also

- File, function, and module structure: [code-structure.md](code-structure.md)
- Trait shape and seam design: [contracts.md](contracts.md)
- Error type naming and classification: [error-handling.md](error-handling.md)
- Threshold rationale and gungraun benchmarking: [docs/researches/18](../../researches/18-rust-performance-methodology.md)
