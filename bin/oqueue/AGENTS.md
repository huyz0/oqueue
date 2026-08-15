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
3. ⚠️ **Assuming `cargo build --workspace` covers aarch64.** It does not link
   there — no cross-linker until `M13`.
