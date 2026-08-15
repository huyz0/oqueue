# 0001. Build profiles

Status: accepted
Date: 2026-08-15
Requirements: NFR-2, NFR-40

## Context

The workspace acquired its root manifest in `M0.2` with no `[profile]` sections
at all. That is a defensible state for one crate and an untenable one for
eleven: by `M0.8` the hot path is split across `oqueue-buf`, `oqueue-codec`,
`oqueue-checksum` and `oqueue-index`, and the compiler's default behaviour at
that boundary decides whether the split costs anything.

**A crate boundary is an optimization barrier.** From the
[rustc dev guide](https://rustc-dev-guide.rust-lang.org/backend/monomorph.html),
by function kind:

| Function kind | Inlinable across a crate boundary? |
|---|---|
| Non-generic, no attribute | **No** — its MIR is not encoded in the rlib |
| Non-generic + `#[inline]` | Yes |
| Generic | Yes — monomorphized in the caller's crate |

So a `pub fn read_varint(&mut self) -> u32` in `oqueue-codec`, called from
`oqueue-broker`, is a real function call with a real prologue unless something
intervenes. ⚠️ **[ADR-0003](0003-leaf-crate-split-and-inlining.md) later
qualified this**: measured on 1.97.1, a *small* such function inlines with no
LTO and no annotation. Thin LTO stays mandatory here — it is what makes a callee's IR available
across the boundary at all, though ⚠️ it does not by itself inline the 28-35
statement band, where `#[inline]` is the difference (ADR-0003) — but the argument is
narrower than this paragraph first made it. In a monolith, link-time optimization is worth a few percent. Here
it decides whether the codec inlines at all.

The requirement that makes this matter is **NFR-2**: tail-read p99 ≤ 10 ms with
the data in cache. That budget contains no object-storage round trip by
construction, so it is *entirely* CPU, and a lost inline in the decode loop is
paid on every batch. `M0.7` (ADR-0003) decides the four-crate split itself; this
ADR decides what the compiler does with whatever split ADR-0003 lands on.

Three further constraints:

- **NFR-40** makes Linux x86-64 and aarch64 both first-class, so no profile may
  be host-specific.
- `security.md` rule 4 requires `overflow-checks` in **release**, not only in
  development — a silently wrapping offset is a correctness failure this project
  has no other detector for.
- Benchmarks must measure the code that ships. Doc 18 §3.6: *"the bench profile
  should differ from the shipping profile in exactly one way."*

⚠️ **NFR-56 does not decide the LTO question, and an argument that leans on it
would be wrong.** The pre-commit suite compiles under `dev` and `test` and never
touches `release`, `dist` or `bench`, so the LTO settings below cost it nothing.
The build cost LTO imposes is paid by `cargo build --release` and by the release
CI job, which are not what NFR-56 measures. Saying "fat LTO is out of `release`
to protect the pre-commit budget" would be a reason that sounds rigorous and is
false.

⚠️ That is narrower than "this table is free at commit time", which is not true:
`[profile.dev.package."*"] opt-level = 2` is in this table, applies to exactly
the profiles pre-commit builds, and becomes the most expensive line in it the
moment a crate first takes an async dependency — ⚠️ not `M0.4`, which decides
the runtime without adding it (ADR-0002), and no other M0 task adds one either. It is a deliberate trade — pay once, cached
forever, against integration tests that are unusably slow with an unoptimized
compression or crypto dependency in the loop — and `M0.16` measures NFR-56's
constant with it already in place.

## Decision

Five profiles, at the workspace root only, as `build.md` rule 10 requires. The
three that carry a decision rather than a default:

```toml
[profile.release]
lto = "thin"
codegen-units = 16
overflow-checks = true
debug = "line-tables-only"
strip = "none"

[profile.dist]
inherits = "release"
lto = "fat"
codegen-units = 1

[profile.bench]
inherits = "dist"
debug = true
```

plus `dev` (with dependencies at `opt-level = 2` and no debuginfo) and
`release-checked` (release codegen, assertions on), and `panic = "unwind"` and
`opt-level = 3` in `release`, which are the cargo defaults restated. The
manifest is the authority; the excerpt above is every key that carries a
decision — including `strip = "none"`, which commitment 5 turns into a rule —
written as valid TOML so the two can be compared directly.

Five commitments follow, and they are the substance:

1. **Thin LTO in `release` is mandatory, not tuning.** It is not to be traded
   away for build time; if release builds become too slow, the answer is fewer
   dependencies or a smaller unit of rebuild, never this line.
2. **Fat LTO is in `dist` and nowhere else.** Tagged artifacts get it. Nothing a
   developer or a per-commit CI job runs does.
3. **`bench` differs from `dist` in exactly one key.** Adding a second key to
   `[profile.bench]` is a change that needs an argument, because it makes the
   benchmark measure a program that is not the one shipped.
4. ⚠️ **Anything that benchmarks sets its own `CARGO_TARGET_DIR`.** Cargo gives
   the built-in `bench` profile **no directory of its own**: it writes to
   `target/release`, the same path `--profile release` writes to. Verified —
   `--profile dist` produces `target/dist`, `--profile release-checked`
   produces `target/release-checked`, and `--profile bench` produces
   `target/release`. So a benchmark run silently replaces `target/release/`
   with a fat-LTO, full-debuginfo binary that is not the release build, and any
   later step that packages "whatever is in `target/release`" ships it. This is
   a constraint on the *harness*, not on the table above, because no profile
   key can express it. ⚠️ **It applies now, not at `M14`.**
   `scripts/bench.sh` and `scripts/profile.sh` already exist and already run
   `cargo bench`; both set `CARGO_TARGET_DIR` to `target/bench` as of this
   commit, which is the same commit that makes `bench` codegen diverge from
   `release` and therefore the commit that would otherwise have introduced the
   hazard. `M13` inherits the rule for whatever cuts release artifacts.
5. ⚠️ **`release` must not strip** — a constraint on a *different* profile than
   the three above. `dist` and `bench` both inherit `strip` from `release`, and
   a stripped binary destroys the symbol attribution that `debug = true` in
   `bench` exists to provide. Doc 18 §3.6 guards this by writing
   `strip = false` into `[profile.bench]`; that is declined here because
   `strip = "none"` is already inherited and the two are the same value — they
   emit byte-identical rustc invocations, neither passing `-C strip` at all —
   so the line would be a second key that changes nothing while breaking
   commitment 3 on sight. This rule is the guard instead. ⚠️ Nothing enforces
   it — see *Consequences*.

## Alternatives considered

**Fat LTO in `release`, no `dist` profile.** Rejected on measured cost: doc 18
§3.3 puts fat LTO with `codegen-units = 1` at **~2.3× build time**, against
**~8.6%** wall-clock for LTO in general over none. That multiplier is paid by
every developer running a release build and by every release CI job, to buy the
increment of fat over thin — which the same table does not separate out, and
which is *"some over thin"* rather than a number. Paying 2.3× on every build for
an unquantified fraction of 8.6% is not a trade to make by default. It is a
trade worth making once per tagged artifact, which is what `dist` is.

**No LTO at all, plus `#[inline]` on every boundary-crosser.** Rejected as
insufficient rather than wrong — doc 18 §3.1 asks for the annotations *as well*,
so non-LTO builds behave sanely. It fails as a substitute because it is a
convention with no gate: an un-annotated `pub fn` added later compiles, passes
every check, and costs a call in the decode loop with nothing anywhere reporting
it. LTO is the mechanism that does not depend on anyone remembering.

**Merging the four leaf crates so there is no boundary to optimize across.**
Not rejected here — deferred, because it is ADR-0003's decision and `M0.7`'s
task. Recorded because it is the alternative that makes this ADR unnecessary,
and doc 19 §1.2 already holds the position that the tension *"is resolved by
thin LTO in release, not by merging crates"*. This ADR is that position being
acted on; ADR-0003 is free to overturn it, and would then be overturning
something stated rather than something assumed.

**`codegen-units = 1` in `release` without touching `lto`.** Rejected, and
recorded because it is the trap rather than a serious candidate: doc 18 §3.2 and
`build.md` rule 6 — `lto = false` with `codegen-units = 1` performs **no LTO at
all**, not even the thin-local LTO the stock profile does. It reads as tuning,
it is strictly worse than changing nothing, and it is exactly what a future
edit that deletes `lto = "thin"` while keeping the low `codegen-units` would
produce.

**Leaving `bench` at its cargo default.** Rejected because the default is to
inherit `release` — `codegen-units = 16`, thin LTO — so out of the box you
benchmark codegen you never ship (doc 18 §3.2). This one is invisible: the
benchmark runs, produces numbers, and the numbers are about a different binary.

**PGO, and BOLT.** Both deferred, neither rejected on merit. PGO measures 14%
(resvg) to 36% (typos) on top of LTO, which is larger than everything above
combined, and doc 18 records the encouraging result that a profile trained on
one input generalized to another. It is deferred because it needs a training
workload, and this project has no runnable broker to train on until `M2` — the
decision belongs to `M14`, with the profile in hand. BOLT is closer to a
rejection: 0.8–3%, doubles the binary, and its instrumentation is **broken on
aarch64**, which NFR-40 makes a first-class target.

**`target-cpu` and `target-feature`.** Out of scope deliberately. Doc 18 §3.4
recommends an `x86-64-v2` baseline (which makes SSE4.2 `crc32` statically
available) and `+lse,+crc` on aarch64, where LSE atomics are measured by AWS at
*"over 3x"* throughput on larger Graviton systems. Those belong in
`.cargo/config.toml` as per-target rustflags rather than in a profile, they
interact with the runtime-dispatch design in doc 18 §4.2, and getting them wrong
means a **SIGILL on a customer's machine** rather than a slow build. They are
`M13`'s, with the release artifacts.

## Consequences

**Easy.** A `pub fn` crossing a crate boundary is inlinable in every shipping
build, so `M0.7` can split the leaf crates on design grounds. ⚠️ **Not
*without anyone annotating it*, which this ADR originally claimed and
[ADR-0003](0003-leaf-crate-split-and-inlining.md) measured false**: LTO makes
the callee's IR available, but `#[inline]` separately raises LLVM's inline cost
threshold from 225 to 325, and there is a size band — around 28-35 statements,
which is the size of a codec entry point — where the annotated function inlines
under thin *and* fat LTO and the unannotated one does not. ADR-0003 is the
authority on the annotation policy; this ADR is the authority on the profiles. Benchmarks, once `M14` writes
any, measure `dist` codegen by construction. A release build that overflows an
offset panics instead of wrapping.

**Hard.** Release builds are slower than a no-LTO workspace and will get
noticeably slower as the dependency graph grows, because thin LTO's cost scales
with the graph. `dist` is ~2.3× on top of that. Doc 18 §3.7.2 is blunt about the
second-order cost — **budget 30–60 GB of `target/` steady state per worktree**,
because each profile is an independent tree and `bench` carries fat LTO *and*
full debuginfo. Delete `target/dist` and `target/bench` between uses; both
rebuild from nothing and are needed only when an artifact is being cut or a
benchmark run.

⚠️ Doc 18 §3.7.2 says "five profiles are five independent trees" and that is
**four**: `bench` has no tree of its own, which is commitment 4 above. That
sounds like a saving and is not one. Compilation units are keyed by a metadata
hash that includes the profile settings, so both profiles' rlibs and objects sit
side by side in `target/release/deps` — the disk is spent either way, and only
the *uplifted* artifact at `target/release/<name>` is a single slot the two
compete for. Measured on 1.97.1: alternating `release` → `bench` → `release`
rebuilds **nothing**, 0.01 s each way, while that one file flips between two
different binaries. ⚠️ That the swap is free is precisely what makes it
dangerous — a collision that cost a rebuild would announce itself. The 30–60 GB
figure stands; it is dominated by dependency debuginfo, not by profile count. Commitment 4 buys the fifth tree back at `target/bench` for
anything run through `scripts/bench.sh` or `scripts/profile.sh` — which is what
makes "delete it between uses" a followable instruction — but a bare
`cargo bench` still lands in `target/release`, so the collision is avoided by
using the harness rather than by anything in this table.

**Foreclosed.** Nothing, and that is worth stating plainly: every line here is a
key in one TOML table, and reversing any of it is a one-line edit with no
migration. This ADR exists because the *reasoning* is expensive to reconstruct,
not because the decision is expensive to reverse — three of the five entries
above are here to prevent a specific silent failure, and a silent failure is one
whose absence leaves no evidence that anything was avoided.

⚠️ **What has no gate.** ⚠️ **Most of it** — this said "all of it" until
`M0.21`, which gated the fourth and fifth bullets below. The sixth is still
ungated, and deliberately: see **The last is neither** at the end. Nothing in `.pre-commit-config.yaml`
reads the *table*, so every commitment above about `lto`, `codegen-units`,
`strip` or `debug` remains a preference in this project's own sense of the
word; the two correctness properties are now checks:

- Deleting `lto = "thin"` leaves the tree green, and the resulting binary is
  slower in a way only `M14`'s benchmarks would reveal.
- Adding a second key to `[profile.bench]` leaves the tree green.
- Setting `strip` on `release` leaves the tree green and silently removes the
  symbols `bench` needs.
- ⚠️ **Deleting `overflow-checks = true` from `[profile.release]` leaves the
  tree green**, and this one is different from the three above because
  `security.md` rule 4 does not merely imply a gate, it **names** one —
  "→ gate on the profile setting". ⚠️ **`M0.21` wrote it** — into
  `check-layering.sh`, which already parses every manifest; see the resolution
  at the end of this bullet. Until then no such script existed. The failure it
  guards is the one that standard cites: an unchecked addition producing an
  out-of-bounds slice from entirely safe code, where **the debug build panics
  and the release build wraps**, so every test in this repository passes while
  the shipped binary is wrong. ⚠️ **`M0.21`'s acceptance carries the check** —
  it was `M0.8`'s until M0's checkpoint review found that row had grown past one
  commit and split the manifest assertions out. It lands alongside the
  member-`[profile]` one and for the same reason: in `check-layering.sh`, which
  already parses manifests, with that row's obligation to add a
  `tests/gates/negative.sh` case. ⚠️ **Done, in `M0.21`** — the assertion is
  there and its negative case plants a release profile that sets `lto` and not
  `overflow-checks`. Nothing enforced it between here and there, and no task in
  that window touched a profile.
- A `[profile]` section in a **member** crate is ignored by cargo, which warns
  on stderr and exits 0 — so it is invisible unless someone reads the warning.
  `M0.8` added ten member manifests at once, which is where this became likely;
  ⚠️ the check itself was `M0.21`'s, in `check-layering.sh`, which already
  parses every crate manifest — split out of `M0.8` by M0's checkpoint review,
  and **done there**, including an indented `  [profile.release]`, which cargo
  still reads as a real table and still ignores.
- ⚠️ **Nothing builds four of the five profiles, ever.** `check-crate.sh`,
  `.pre-commit-config.yaml` and `gates.yml` compile `dev` and `test` and
  nothing else. `M0.3` observed `cargo check` passing under all five on both
  targets by hand, and that observation is a snapshot: a later change that
  breaks `--profile dist` — a dependency that fails under fat LTO, a member
  crate that overflows only with `overflow-checks` on — surfaces at `M13`, when
  someone tries to cut an artifact, and not before. This is the one entry in
  this list a gate could actually close, at the cost of one `cargo check
  --profile dist` step and one `--profile release-checked` step in CI — two
  invocations, because cargo rejects a repeated `--profile`.

The first three are review's job and are stated here so a reviewer has something
to check against; a gate asserting a TOML table matches a TOML table it is
generated from would be a tautology, and the interesting property — "is thin LTO
still buying anything" — is a benchmark, which is `M14`. **Two of the six are
gated** rather than left to review: the member-`[profile]` check and the
`overflow-checks` assertion, both landed in `M0.21` in `check-layering.sh`,
which already parses every manifest. **The last is neither**, and it is recorded as a
finding for M0's boundary review rather than acted on here: adding a CI step is
not what `M0.3` was asked for, and `M0` should not end having decided by
omission that four profiles go unbuilt.
