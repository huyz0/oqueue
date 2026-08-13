# Code structure

Crates, folders, files, functions, and the documents each crate carries.

The purpose is not tidiness. It is that **a change should be reviewable by
someone who has not read the rest of the codebase**, and that an agent working
on one crate should be able to load its context without loading the workspace.

Each rule names its gate, or is marked as having none.

## Crates

1. **Every crate depends on `oqueue-core` and on nothing else in the
   workspace**, with the composers as named exceptions. → `check-layering.sh`
2. **Cycles are impossible** (Cargo enforces it) but **layering violations are
   not** — a low-level crate depending on a high-level one is perfectly acyclic
   and perfectly wrong. That is what the gate is for.
3. **`[dev-dependencies]` are exempt from the layering rule.** A test may
   compose, and Cargo permits a dev-dependency cycle, which is what makes a
   shared testkit usable.
4. **Test scaffolding never appears in a runtime `[dependencies]`.** →
   `check-layering.sh` `DEV_ONLY` list
5. **A new crate requires a recorded decision.** Cheap to add, expensive to
   remove once everything depends on it.
6. **Split a crate when** it is a contract boundary, it can be tested alone, it
   removes work from the build critical path, or it gates an optional
   dependency. **Not when** it is merely a different concept — conceptual
   tidiness with no dependency benefit adds friction and nothing else.

## Folder structure inside a crate

```
crates/oqueue-foo/
├── Cargo.toml
├── README.md            why this crate exists  (rule 13)
├── AGENTS.md            notes for whoever edits it  (rule 14)
├── src/
│   ├── lib.rs           crate docs + module declarations ONLY
│   ├── thing.rs         one concept
│   └── thing/           submodules, when one concept needs several files
│       └── part.rs
└── tests/
    └── it/
        └── main.rs      one integration binary  (rule 10)
```

7. **`lib.rs` holds crate-level documentation, lint attributes, and `mod`
   declarations. No implementation.** A `lib.rs` with logic in it is the first
   stage of a god module.
8. **No `mod.rs`.** Use `thing.rs` beside `thing/`. `mod.rs` files make every
   editor tab say the same thing.
9. **One concept per file**, named for the concept.
10. **One integration-test binary per crate**, `tests/it/main.rs` with `mod`
    declarations. Each `tests/*.rs` is otherwise a separate crate, separate
    link, and separate copy of the debuginfo. → `build.md` rule 15
11. **Unit tests live at the bottom of the file they test**, in `#[cfg(test)]
    mod tests`. They may reach private items; that is the point of them.
12. **`pub(crate)` by default; `pub` is a decision.** The public surface is the
    contract, and every item in it is something you have promised not to break.
    → `unreachable_pub` lint denied workspace-wide

## Per-crate documents

13. **Every crate has a `README.md`** answering, in this order:

    - **What is it?** One paragraph.
    - **Why does it exist?** What would break or become worse if its contents
      were merged into a neighbour. If this question has no good answer, the
      crate should not exist.
    - **Upstream** — what it depends on, and why each dependency is needed.
    - **Downstream** — who depends on it, and therefore what a breaking change
      costs.
    - **Invariants** — what must stay true, and which gate holds each.
    - **Notes for whoever touches this** — what is easy to get wrong here. This
      is the section that earns the file.

14. **Every crate has an `AGENTS.md`** carrying context specific to it: which
    standards bite hardest here, which gates run, what a reviewer will check,
    and what has already gone wrong. It is loaded when an agent works in the
    crate, so it is short and specific, not a copy of the workspace rules.

15. ⚠️ **The README's stated dependencies must match `Cargo.toml`.** →
    `check-readmes.sh` compares them and fails on drift. A document that
    describes a dependency graph the code no longer has is worse than none,
    because it is authoritative and wrong.

## Files

16. **A file is at most 500 lines**, tests included. → `check-file-size.sh`
17. ⚠️ **The limit has an allowlist, and every entry carries a reason.**
    Generated protocol tables and large exhaustive `match` arms over wire types
    are legitimate; a file that grew because nobody split it is not. The
    allowlist is in the script, so adding to it is a diff someone reviews.
18. **A file approaching the limit is a design signal, not a formatting
    problem.** Split by concept, never by line count — two 300-line files that
    must both be read to understand one thing are worse than one 600-line file.

## Functions

19. **A function is at most 50 lines.** → `clippy::too_many_lines` with
    `too-many-lines-threshold = 50` in `clippy.toml`, denied
20. **Cognitive complexity is bounded.** → `clippy::cognitive_complexity`,
    threshold 20
21. **At most 5 arguments.** → `clippy::too_many_arguments`. More than that
    means a struct is trying to exist.
22. **One level of abstraction per function.** A function that both decides
    policy and manipulates bytes is two functions.

## Anti-patterns with gates

23. **No god module.** A file over the limit, a `lib.rs` with logic, or a module
    every other module imports from is the same failure at different scales.
24. **No `util`, `common`, `helpers`, or `misc` module.** These are names for
    "I did not decide where this goes", and they become the god module by
    accumulation. → `check-file-size.sh` also rejects these names
25. **No public struct with more than a handful of public fields.** Public
    fields are a contract with no method to deprecate.
26. **No `unwrap` or `expect` outside `#[cfg(test)]`.** → `clippy::unwrap_used`,
    `expect_used` denied. If an invariant is certain, encode it in a type; if it
    is not, it is a real error.
27. **No `unchecked_add`/`sub`/`mul`.** → grep gate. See `security.md` rule 4.

## What has no gate

**Whether a split is the right split.** A script can count lines; it cannot tell
whether the seam is in a sensible place. Splitting a 600-line file into two
300-line files that are always read together satisfies every gate here and makes
the code worse.

**Whether a crate's README answers "why does it exist" honestly.** The question
is checkable by a human and by a reviewing agent, not by a script.

## See also

- Crate map and the dependency rule: [architecture.md](../product/architecture.md)
- Build-time consequences of the DAG shape: [build.md](build.md) rules 12–16
