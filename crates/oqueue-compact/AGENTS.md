# `oqueue-compact` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-compact` for fmt, clippy and tests.

## Easy to get wrong here

1. **Compaction rewrites data that has already been acknowledged.** NFR-20 — no acknowledged record is ever lost — is the requirement everything here yields to.
2. **A denominator in bytes needs a GET.** The history tier carries no byte
   range by design, so `COMPACTED_OBJECT_RECORDS` is in records. Changing it to
   bytes is a change to what this crate is allowed to do, not a unit swap.

## ⚠️ Nothing here may reach object storage

`M5.1` filled the skeleton `M0.8` created, and the first thing in it is the
compaction trigger. `read_amp` is evaluated for every candidate partition on
every sweep (`ADR-0036` decision 1), so a store call here is a per-partition
cost paid at sweep cadence — `check-sans-io.sh` is what holds it, and the crate
depending on `oqueue-core` alone is what makes the guarantee structural rather
than a rule someone remembers.

