# 0027. The simulation harness is three pieces, each placed by what it depends on

Status: accepted
Date: 2026-09-01
Requirements: NFR-20, FR-51

## Context

Doc 10 `#32` asks **where deterministic simulation sits in the crate graph**,
and calls it "the largest single testing investment the project needs". It was
written assuming `madsim`: a compile-time runtime substitution driven by
`--cfg madsim`, which "touches every crate that does async" and might therefore
"partly subsume" the sans-I/O split. That framing is why the question is an
architecture decision at all rather than a library choice.

⚠️ **`M1.22` answered half of it and refuted the premise for that half.**
`madsim` needs a shim per I/O crate; none exists for the `reqwest` → `hyper` →
`tokio::net` stack `object_store` reaches the network through, so the
substitution the entry describes is unavailable here. What works instead is
`object_store`'s own public `HttpService`/`HttpConnector` seam, above the
socket. Recorded with its reasoning in `testing.md`, "Deterministic simulation:
the answer is a transport seam, not a runtime swap".

⚠️ **The other half was left open and is the reason for this ADR.** The broker's
own socket I/O is a different stack, `turmoil-net` is a `tokio::net` drop-in,
and a drop-in *is* crate-graph-relevant in the way a trait implementation is
not. `M10.md` lists it as needing an ADR; `M10.0` found it owned by no row and
made it part of this one.

⚠️ **The first draft of this ADR answered "nowhere — sans-I/O already put the
seams where the harness needs them", and review refuted it by running the
gates.** The seams are indeed already there, and that observation is true and
load-bearing below. But "no crate changes" did not follow from it, and the
constraints that decide the placement are gates this project already enforces:

1. **`check-sans-io.sh` scans `crates/*.rs`** — including
   `crates/oqueue-testkit/src/lib.rs`, verified with the gate's own
   `git ls-files` — and its `STORE_RE` matches `object_store::`. So an
   `HttpService` implementation in `oqueue-testkit` **fails the gate**, and so
   does a concrete simulated socket, which `SOCKET_RE` catches.
   ⚠️ **The exemptions are directory prefixes, and that is what decides two of
   the three placements below.** `oqueue-broker` is exempt from all three
   patterns and `oqueue-store` from `STORE_RE`, and the test is
   `[[ "$f" != "$DIR"/* ]]` — so a crate's `tests/` tree is exempt exactly as
   its `src/` is.
2. **`check-layering.sh` names exactly two composers**, `oqueue-broker` and
   `oqueue`. Every other member's `[dependencies]` may name only `oqueue-core`
   and the dev-only crates. So a harness in `oqueue-testkit` that *builds a
   broker* would need `oqueue-broker` in its `[dependencies]` and **fails the
   gate**.
3. **`bin/oqueue` has no `[lib]` target** — only `main.rs` and `serve.rs` — so
   nothing outside the binary can link against its accept loop and supervision
   `select!`. ⚠️ **That is not the same as untestable**, which this said until
   review checked: `serve.rs:238` already has a `#[cfg(test)] mod tests`
   reaching private items, and `accept_loop`'s arguments are all constructible
   there. What is out of reach is driving it from a *simulated* run, not
   asserting its behaviour.
4. **The broker reads time in two ways the socket seam does not cover.**
   `writer_id.rs:58` calls `SystemTime::now()` *deliberately* — its own doc
   says "the real clock, deliberately, and not the `Clock` seam", because a
   shared `FakeClock` would collide two `WriterId`s in the test written to
   prove them distinct — and `session.rs`, `fetch/park.rs` and `connection.rs`
   reach `tokio::time` directly. ⚠️ **Not `fetch/target.rs`**, whose every time
   call is below its `#[cfg(test)]` at line 229; this list named it until
   review checked.

The harness therefore is not one thing, and the question "where does it sit"
has no single answer. What decides each piece is which dependency it needs.

## Decision

**Three pieces, each placed by the dependency that constrains it. No crate is
added, no gate gains an exemption, and nothing test-only enters a crate's
shipped `src/`.**

1. **The seed, the scheduler, the fault schedule and the invariant checks go in
   `oqueue-testkit`.** They need `oqueue-core` and nothing else — no
   `object_store`, no socket, no broker — so they clear both gates as they
   stand, and this is the role `architecture.md` already gives that crate:
   "Harness and generators. **Dev-only.**" ⚠️ This is the part every crate's
   tests share, which is what makes a shared crate worth having at all.

2. **The simulated object-store transport goes in `oqueue-store`'s `tests/`
   tree.** It implements `object_store::HttpService`, and `oqueue-store` is the
   only crate depending on `object_store` — so its own tests are the only
   consumer, which is exactly the scope `testing.md` gives the seam: it
   exercises this project's request *construction* and response *handling*, and
   nothing below HTTP. ⚠️ **Not behind a `src/` feature, which this said until
   review priced it**: `check-crate.sh` runs `cargo test` with **default**
   features by design — its header lists "a failing test behind a feature flag
   reports green" as a known cost, and `oqueue-codec`'s gzip and snappy tests
   are the standing example — so a feature-gated transport would compile under
   `clippy --all-features` and be executed by nothing. In `tests/` it runs
   under the same `cargo test` every other gate does, and no test-only code
   reaches the shipped crate at all. ⚠️ **The round-one objection is dissolved
   rather than answered**: nothing here can ship, so "how a fake reaches
   production" no longer applies, and `FakeObjectStore` stays beside
   `ObjectStore` in `oqueue-core` under `contracts.md` rule 9, untouched.

3. **The simulated socket and anything that composes a broker go in
   `oqueue-broker`'s own `tests/` tree, beside `support.rs`.** That file is
   already this shape and says so: "the *composition-root* shape instead: only
   the public API, exactly what `bin/oqueue` calls, so a change that broke the
   wiring would fail here rather than only in a unit test that could reach past
   it." An integration test may dev-depend on anything, so a simulated listener
   lives there without touching the layering rule and without a library crate
   naming a socket type.

4. **Simulation composes with the sans-I/O split; it does not subsume it**, and
   this is the first draft's observation, which survives. `oqueue-broker`'s
   `connection.rs` is generic — `S: AsyncRead + AsyncWrite + Send + 'static` —
   and the only `tokio::net::TcpListener` in the workspace is in
   `bin/oqueue/src/serve.rs`. The substitution point for the broker's socket is
   a bound sans-I/O created four milestones ago, not a seam M10 adds.
   ⚠️ **Non-negotiable 5 is unaffected and `check-sans-io.sh` keeps its two
   exemptions**, which is a property of the placement above rather than a
   happy accident of it.

5. **Determinism needs a third substitution the seams above do not give:
   time.** `M10.5` owns routing `session.rs`, `fetch/park.rs` and
   `connection.rs` through a controllable clock. ⚠️ **`writer_id.rs` is
   excluded by name and must stay excluded** — see the constraint above — so
   `M10.4`'s "one seed reproduces one run" cannot mean byte-identical object
   keys, and either the harness fixes the `WriterId` at a composition root or
   the seed criterion is stated over the log rather than over key names.
   `M10.4` and `M10.5` decide which; recording the constraint here is what
   stops it being discovered as a flake.

6. **Which library provides the simulated socket is not decided here.**
   `turmoil` is the candidate and this ADR does not adopt it. ⚠️ **It is more
   than an `AsyncRead + AsyncWrite` implementation**: it drives hosts inside
   `Sim::run` on per-host current-thread runtimes with paused time and resolves
   addresses through its own registry, so a harness using it cannot be a plain
   `#[tokio::test]` and owns the runtime. `M10.2` measures that cost against
   the alternative of an in-process duplex stream, which needs no new
   dependency at all. Placement point 3 holds either way, which is what makes
   it an architecture decision rather than a dependency choice.
   ⚠️ **But point 1 does not hold either way, and `M10.2` must say which.**
   Under `turmoil` the interleaving is decided inside `Sim::run`, which
   `oqueue-testkit` can seed and cannot observe — so "the scheduler" there is a
   seed and a fault schedule, not an executor. Under a duplex stream the
   testkit owns the interleaving outright. Whichever is chosen, `M10.4`'s
   acceptance criterion is about *replay*, and a seeded run that reproduces
   only the unsimulated half fails it.

## Alternatives considered

**All of it in `oqueue-testkit`.** This was the first draft's decision and it
does not survive the gates: `object_store::` there fails `check-sans-io.sh`,
and depending on `oqueue-broker` there fails `check-layering.sh`. Adopting it
would have meant a third sans-I/O exemption and a third composer — two
amendments to non-negotiable 5's and the layering rule's own definitions, to
place test code. ⚠️ **The rule that reads like the obstacle is not the one that
is**: `contracts.md` rule 9's "that crate should stay nearly empty" is about
*fakes* drifting ("growth there means **fakes** have drifted away from the
contracts they stand in for"), and point 1 above is not a fake. The gates are
the real constraint, and they were checked rather than reasoned about only
after the first review.

**A new `oqueue-sim` crate.** Rejected, and note it does not actually solve the
problem: `check-layering.sh` would let other crates dev-depend on it once it
joined `DEV_ONLY`, but its *own* `[dependencies]` would still be limited to
`oqueue-core`, so it could no more compose a broker than `oqueue-testkit` can.
Making it work needs the same two amendments, for a crate that would hold what
point 3 already has a home for.

**A `#[cfg(test)]` module per crate, as `oqueue-core` does with
`test_executor.rs`.** Rejected for point 1 on a hard constraint: a
`#[cfg(test)]` module is invisible across a crate boundary, and the seed and
invariant checks must serve three crates. `crates/oqueue-broker/tests/it/support.rs`
records the same finding from the other side — `src/testing.rs` is
`#[cfg(test)]` and therefore "invisible from an integration test". It remains
right for anything whose only consumer is its own crate.

**`madsim`, or any `cfg`-swapped runtime.** Rejected by `M1.22` with
measurements rather than by argument: no shim exists for `reqwest`/`hyper`, so
the swap would be a global `[patch.crates-io]` of `tokio` beneath three
third-party crates that never agreed to it, with DNS, TLS and the wall clock in
that path. ⚠️ That `madsim-aws-sdk-s3` exists at all is the tell — the
ecosystem's own answer for S3 replaces the *SDK*, not the transport under it.

## Consequences

**Makes easy.** No crate is added and no gate is amended, so the layering and
sans-I/O rules keep the definitions they have. Every crate's tests can take the
shared half as a dev-dependency. The broker needs no change to be driven over a
simulated socket, which is a return on the sans-I/O rule paid four milestones
after it was written.

**Makes hard.** The harness is in three places, which is a real cost: a reader
looking for "the simulation" finds a third of it, and there is no single crate
whose README describes the whole. ⚠️ **Each of the three must name the other
two**, in `oqueue-testkit`'s README, a header in `oqueue-store`'s test tree and
one in `oqueue-broker`'s — `M10.14` owns that alongside its `testing.md`
paragraph, because `check-readmes.sh` checks presence and shape and cannot see
a missing cross-reference.
⚠️ **And a `tests/` tree is not a published surface**, so nothing here is
reusable by another crate: if `oqueue-broker`'s tests ever need the simulated
transport rather than `oqueue-core`'s `FakeObjectStore`, it has to move, and
`oqueue-testkit` is not where it can move to. That is a real bound on this
decision and the point at which to reopen it.

**Forecloses.** Compile-time runtime substitution, for as long as this stands.
If a future need genuinely requires simulating below the HTTP layer — TLS
behaviour, connection pooling, `object_store`'s own retry timing, all of which
`testing.md` already says this seam does *not* buy — this decision has to be
reopened rather than worked around.

⚠️ **And it forecloses simulating `bin/oqueue`'s accept loop**, which has no
`[lib]`, so no *simulated* run reaches it. The `select!` racing the
coordinator's `serving` handle against `listener.accept()` — whose own comment
says a failure there would make the broker "answer every produce
LEADER_NOT_AVAILABLE forever while looking healthy" — is exactly the class
NFR-20 and FR-51 name. ⚠️ **The cheap half is already available and should be
taken first**: `serve.rs:238`'s existing `#[cfg(test)] mod tests` can construct
`accept_loop`'s arguments and assert the `joined` arm, which buys that
assertion without a crate change. Extracting a lib from `bin/oqueue` is what
*simulating* it would need, and this ADR declines to make that change blind —
`M10.13` is where the remaining cost becomes visible, if any.

⚠️ **Does not close doc 10 `#32`.** Its *cost* question — the largest single
testing investment — is untouched by anything here; only the mechanism and the
placement are settled. `#32` stays on the open list for that reason, as it did
after `M1.22`.
