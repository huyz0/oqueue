# 0029. NFR-56 measures the suite, not the container that runs it

Status: accepted
Date: 2026-09-01
Requirements: NFR-56

## Context

`M10.24` wired the local `git commit` hook to run the whole `pre-commit`
stage inside `scripts/docker-test.sh`'s container — before that commit, the
three heaviest hooks (`check-crate`, `check-coverage`, `check-mutants`) ran
on the host uncontained, which was itself the gap `M10.24` closed. `M10.25`
measured what that did to NFR-56 ("Pre-commit suite completes within 10 s",
`check-budget.sh` against a literal, summing each gate's own recorded wall
clock, `BUDGET_MS = 10000`):

- **Containment roughly doubled the measured suite.** Min/max of every
  17-gate `ok` row in `target/timings/suite.tsv`, not a pair of
  representative runs. Host, native, n=74: **3510–6305 ms**. Container,
  n=12: **7658–9456 ms**. The worst contained run is at **94.6%** of
  `BUDGET_MS`; the native distribution never exceeded 63%.
- **~2 s of real, developer-felt latency is invisible to `check-budget.sh`.**
  The `docker images` stale-image scan, two `docker volume create` calls, and
  — the dominant piece, measured directly against this repo's own image at
  ~380–390 ms each — two separate `docker run --rm ... test -w` permission
  probes (one per named volume) all run **before** `pre-commit` starts, so
  none of it is inside any gate's own timed span. One measured run: 11.3 s
  wall clock for a `pre-commit` invocation `check-budget.sh` counted at
  9350 ms.
- **Non-negotiable 2 forbids raising `BUDGET_MS`.** It is one of the seven
  constants `m0-complete.sh`'s `NFR_CONSTANTS` table names, and 10000 is the
  floor's own ceiling, not a knob.

The question this row asks: does NFR-56 bound the suite `check-budget.sh`
sums, or the wall clock a developer actually experiences from `git commit` to
the hook returning? The two readings now disagree by roughly two seconds, and
picking wrong either lets a real regression hide behind a number that is not
what the requirement means, or spends the rest of this milestone chasing a
budget the requirement was never written to include.

## Decision

**NFR-56's number is, and has always been, about the suite `check-budget.sh`
sums — not about the process that invokes it.** The requirement's own
measurement column says so: "`check-budget.sh` against a literal, summing
each gate's own recorded wall clock." That method predates containment by
ten milestones and never counted the overhead *outside* a gate's own span
either — a hand-written hook's shell startup, `pre-commit`'s own Python
bootstrap, git's hook dispatch. Containment adds a larger fixed cost in the
same category, not a new category. Reading NFR-56 as "everything between
typing `git commit` and getting a prompt back" was never true even in the
native era; it would also make the requirement's compliance depend on
factors `check-budget.sh` cannot see or attribute to a specific gate, which
is what non-negotiable 2's "never move a threshold" is protecting against —
a budget nobody can point at is a budget that erodes silently.

**This does not make the ~2 s free.** It is real latency a commit pays every
time, and it is reduced where reduction is cheap and does not compromise the
correctness the probe exists for: the two `test -w` permission-probe
containers merge into one, checking both named volumes in a single `docker
run` rather than two — measured at ~380 ms saved per invocation, the
dominant piece of the invisible cost that was actually addressable without
restructuring how containment works. The `docker images` stale-image scan
and the two `docker volume create` calls are left alone; both measured under
50 ms combined, not worth the risk of touching for the return.

**The 94.6% margin inside `check-budget.sh`'s own sum is a separate, live
risk this decision does not resolve.** A busier host, `CPUS` lowered further,
or one more dependency added to the hot gates can still blow `BUDGET_MS`
outright, and non-negotiable 2 means the only two honest responses when that
happens are: make the slow gates (`check-mutants.sh`, `check-coverage.sh`)
genuinely faster, or fall back to `OQUEUE_NO_CONTAINER=1` — documented,
visible, and understood as giving up the memory-containment guarantee
`M10.23` built, not a quiet escape hatch. This ADR does not pick between
those two for a future overrun; it only says raising the constant is not a
third option.

## Alternatives considered

**A. Give the contained path its own, larger stated budget.** Rejected. It
solves the symptom by relabeling it: `BUDGET_MS` stays a single named
constant `m0-complete.sh` checks against a table, and a second budget next to
it for "the same suite, but slower" is two numbers answering the question
"is the suite fast enough" depending on which one a reader happens to read —
exactly the ambiguity non-negotiable 2 exists to prevent. It also does
nothing about the ~2 s of genuinely invisible overhead, which would still sit
outside *either* number.

**B. Redefine `check-budget.sh` to measure the full wall clock, container
startup included.** Rejected as the *default* reading, though it is the more
conservative one. Doing this literally would have `check-budget.sh` report a
violation for a run whose own gates finished on time — the requirement would
start failing for reasons no gate's own timing row can explain, and the
"which gate got slow" diagnosis `check-budget.sh`'s per-gate breakdown exists
to provide would stop working the moment the overrun comes from Docker
startup rather than any gate. If a future measurement shows the invisible
overhead growing rather than shrinking, this is the fallback to revisit —
recorded here so the option is not lost, not chosen now.

**C. Remove containment from the commit path, restoring the native margin.**
Rejected outright. This undoes `M10.23`'s and `M10.24`'s own point: a
runaway `cargo test`/`cargo-mutants` process with no ceiling can exhaust the
host, and `M10.24`'s whole row exists because that path was found still
uncontained. Trading a timing risk for a memory-exhaustion risk is not a
trade this project makes.

## Consequences

- NFR-56 stays satisfied today (worst measured contained sum 9456 ms of
  10000) and the requirement's own text needs no edit — only this ADR, which
  the requirement's row does not yet link, records what "the suite" was
  always understood to mean.
- The merged permission probe is a small, real win (~380 ms) but does not
  close the gap between `check-budget.sh`'s sum and the wall clock a
  developer sees; that gap is now a named, accepted cost rather than an
  unexplained one.
- The 94.6% margin remains thin, and the next dependency, gate, or host that
  pushes the sum itself past 10000 ms is a genuine NFR-56 violation under
  this reading, not a false alarm from counting container startup — which is
  the point: this decision is what lets that future overrun be diagnosed
  correctly instead of relitigating what NFR-56 was about.
- `docs/internal/product/requirements.md`'s NFR-56 row does not name this ADR
  today — no requirement row in that file links to one yet, so this is not a
  convention broken, only a gap this decision could be the first to close the
  next time that row is touched.
