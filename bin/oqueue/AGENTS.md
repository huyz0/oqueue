# `oqueue` (the binary) — working notes

Read [`README.md`](README.md) first. This file is what is specific to *changing*
it.

## Which gates run

`check-layering.sh` (as a composer), `check-unsafe.sh`, `check-file-size.sh`,
and `scripts/check-crate.sh oqueue`.

⚠️ **`check-sans-io.sh` does not scan `bin/` at all.** Everything this crate is
allowed to do that no other crate may — name a backend, read a clock, open a
socket — is unguarded by construction. That is correct, and it means the "only
place a concrete type is chosen" property rests entirely on review.

## Easy to get wrong here

1. **Putting logic here.** A composition root wires; it does not decide
   behaviour. Anything with a branch worth testing belongs in a library crate
   where it can be tested without a process.
2. **`println!` after `tracing` is initialised.** `rust-style.md` rule 12 allows
   the macros here only before that point. Nothing initialises `tracing` yet.
3. ⚠️ **Setting a global allocator anywhere but here.** A library that sets one
   imposes it on every consumer with no way to opt out. ADR-0007. Nothing gates
   this; `grep -rn global_allocator crates/` should stay empty.
4. ⚠️ **Assuming `cargo build --workspace` covers aarch64.** It does not link
   there — no cross-linker until `M13`.
5. ⚠️ **Assuming the `heap-profiling` build is *run* by anything.** Every gate
   compiles it (`check-crate.sh` lints `--all-features`); none executes it,
   because what a gate should run is what ships. ⚠️ Not because it would abort —
   jemalloc bakes in the *build host's* page size, so a gate that builds and
   runs on one machine never mismatches itself. The trap is a build host and a
   deployment host with different page sizes. ADR-0007.
