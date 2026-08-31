# 0028. A seeded runtime, rather than a biased select or a simulator

Status: accepted
Date: 2026-09-01
Requirements: NFR-20, FR-51

## Context

`M10.md` task 2 asks that "a run is reproducible from a single seed printed on
failure", and adds that "a failing seed that cannot be replayed is not a test".
`M10.26` measured whether the tree already provides that and found it does not:

⚠️ **`tokio` randomises `select!` branch order from a per-runtime RNG.**
Measured — 200 iterations of a two-arm `select!` with both arms already ready,
under `#[tokio::test(start_paused = true)]`, which implies a `current_thread`
runtime — the branches split **102/98**, independently reproduced at 105/95 and
104/96. So a paused clock and an in-process duplex socket, which this workspace
has had since `M2.17`, give a *deterministic clock* and not a deterministic
*run*.

There are four `tokio::select!` sites in the workspace — `connection.rs:177`,
`fetch/park.rs:146`, `session.rs:130`, `bin/oqueue/src/serve.rs:126` — and no
`biased;` and no `tokio_unstable` anywhere, so no seed a harness holds reaches
that RNG today.

## Decision

**Seed the runtime's RNG: `--cfg tokio_unstable` workspace-wide, and
`tokio::runtime::Builder::rng_seed` in `oqueue-testkit`'s `seeded_runtime`.**

⚠️ **Measured before adoption, not after.** A probe built two runtimes from the
same seed and one from another, each `current_thread` with time paused, and
recorded 40 `select!` picks: the two same-seed runs produced identical
sequences and the third differed. `seed.rs`'s own tests are that probe, kept —
one asserting a seed reproduces, one that a *different* seed diverges (a
`seeded_runtime` ignoring its argument would pass the first and be useless),
and one driving the loop the module exists for: a schedule-dependent assertion
that fails under one seed, fails **identically** on replay from it, and passes
under a seed reaching the other arm.

**No production *source* changes.** ⚠️ **But not "no effect on the release
binary"**, which an earlier draft of this said: `[build] rustflags` applies to
every profile, and `cfg(tokio_unstable)` is not API-only inside `tokio` — the
**both** schedulers build a `TaskMeta` and dispatch poll-start and poll-stop
hooks on every poll under that cfg — `current_thread/mod.rs` and
`multi_thread/worker.rs` alike — and `cfg_unstable_metrics!` swaps mock
`SchedulerMetrics` for the real one plus histograms. ⚠️ The multi-thread half is
the one that matters here, because that is the flavour `bin/oqueue` runs. Nothing here measured the cost, so a later
latency regression on the serving path must not rule this out on the strength
of a "no behaviour change" claim. `M10.6` and `M14` are where it would be
measured.

**The reproducibility criterion is over the *schedule*, not over bytes.**
⚠️ **`ADR-0027` point 5 left this open and it is decided here**: `writer_id.rs`
reads `SystemTime::now()` deliberately — a shared `FakeClock` would collide two
`WriterId`s in the test written to prove them distinct — so two runs of one
seed produce different object *keys* by construction. A replay claim stated
over key names would be a flake generator. It is stated over the interleaving
and over what the log and the assertions see, and a harness that needs stable
keys must fix the `WriterId` at its own composition root rather than expect the
seed to do it.

## Alternatives considered

**`biased;` on the four `select!` sites.** Measured to work — the same probe is
200/0 with it — and rejected because it is not free and not even sufficient on
its own:

- ⚠️ **Two sites would need their arms reordered, and that is client-visible.**
  `session.rs:130` and `fetch/park.rs:146` both put `sleep_until(deadline)`
  *first*, so `biased;` alone makes each prefer its deadline: an empty fetch
  where a commit landed in the same poll gap, and a staleness refusal where the
  version had in fact arrived.
- ⚠️ **A third site changes behaviour without needing a reorder.**
  `connection.rs:177` is reader-first, and its own comment records the tie
  happening live; under `biased;` a connection the handler closed cleanly would
  report `TimedOut { waiting_for: "frame" }` instead.
- ⚠️ **And reordering introduces a starvation risk.** With the watch arm first,
  `park.rs` polls its deadline only while the watch is pending, so a busy shard
  could run a parked fetch past `fetch.max.wait.ms` — the loop's deadline check
  is outside it.

Buying determinism with three behaviour changes to the serving path, to make
tests reproducible, is the wrong trade in that direction.

**`turmoil`.** Not rejected on its merits, and two drafts of `ADR-0027`'s note
rejected it for reasons that did not hold — most recently that it seeds network
delivery and not `select!`, which was true of 0.6.x and is false of 0.7:
`turmoil-0.7.2/src/rt.rs:235-246` derives a `RngSeed` from its own seed for
every host runtime, under the same `tokio_unstable` flag this decision adopts.
It is deferred rather than refused, because what it adds *over* this decision
is **seeded network delivery between hosts**, and the one use `M10.md`
enumerated for that — the agent-to-coordinator partition — `M10.0` deferred to
`M7`. ⚠️ **Adopting the flag now is a step toward `turmoil`, not away from it**:
`M7` can take the crate without revisiting this.

**Doing nothing and calling the harness "usually deterministic".** Named
because `M10.md`'s first risk is precisely that this is worse than an honestly
non-deterministic one: failures get dismissed as flakes.

## Consequences

**Makes easy.** A seeded run with no production *source* change, no new
dependency, and no reordering of anything a client can observe. `M10.12`'s
shrinking needs this and nothing more; `M10.11`'s corpus needs this **plus** the
pin below.

**Makes hard.** ⚠️ **A seed's *meaning* is not stable, and the dangerous
failure is the silent one.** `RngSeed::from_bytes` hashes its input with
`std::collections::hash_map::DefaultHasher` (`tokio/src/util/rand/rt_unstable.rs`),
whose output std documents as unstable across releases, and `select!`'s
`thread_rng_n` is equally unpinned behind `tokio_unstable`. So a
`rust-toolchain.toml` bump — which `build.md` rule 1 makes routine — or a
`tokio` 1.x patch touching the RNG remaps every recorded seed to a different
schedule, with no compile error and no failing test. ⚠️ **That is a direct
constraint on `M10.11`'s corpus**: a stored `u64` means "this schedule" only
against the toolchain and `tokio` version it was recorded under, so the corpus
needs that pin recorded beside it or an artefact stronger than a seed. Removal
of `rng_seed` outright would at least break the build; the remap will not.
`M10.14` carries both into `testing.md`.

⚠️ **And `oqueue-testkit`'s `tokio` is a normal dependency**, because `seed.rs`
is in `src/`, so `test-util` unifies into `cargo build --workspace` — measured
with `--unit-graph` — where every other `test-util` in this workspace is a
`[dev-dependencies]` entry and stays out. `cargo build -p oqueue` is
unaffected, so **release packaging must build the binary by name**, and
`m0-complete.sh`'s link check (which uses `--workspace`) is linking a `tokio`
the shipped binary would not have. `M14` owns turning that from a sentence into
a gate.

⚠️ **Every build that does not set the flag has a different fingerprint and
recompiles.** `.cargo/config.toml` is where it lives so a developer, the
container and CI cannot each remember it differently — but a `cargo` invocation
that overrides `RUSTFLAGS` bypasses that file entirely, and
`check-budget.sh`'s trend history restarts once because the first build after
this lands is cold.

**Forecloses.** Nothing. `biased;` remains available if a *specific* site turns
out to want a deterministic order for its own reasons, and this decision is the
reason that would then have to stand on its own rather than being smuggled in
as a test affordance.
