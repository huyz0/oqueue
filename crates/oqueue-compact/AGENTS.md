# `oqueue-compact` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh` (including its `read_amp` no-store leg), `check-unsafe.sh`, `check-file-size.sh`,
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
cost paid at sweep cadence.

⚠️ **What holds it is `read_amp`'s signature and one leg of `check-sans-io.sh`
written for that file** (`M5.38`), and it is worth knowing what does *not*:
depending on `oqueue-core` alone leaves a store one `use` away, because
`oqueue-core` exports `ObjectStore` and `FakeObjectStore`; and the gate's four
SDK patterns look for `object_store::`, `aws_sdk_s3`, `aws_config`,
`google_cloud_storage` and `opendal::`, none of which a call through the core
trait matches. `M5.4`'s merge executor will bring a store into this crate
legitimately.

⚠️ **What the leg catches, and what it does not.** It catches a store named in
`read_amp.rs` — any of the five types whose name ends in the seam's, the fake
and the three wrappers included, because the pattern is a substring rather than
a word match. It does **not** catch a store reached through a generic: a
`Trigger<S>` declared in another module, with `read_amp.rs` holding only
`self.store.get(..)` on a bound it never spells, passes. That is a real hole and
it is left open on purpose — a grep cannot see a type it is not shown, and the
rule that would close it is a reviewer noticing that the trigger acquired a
field. Which is to say the leg holds the *signature*, and judgement still holds
the path.

