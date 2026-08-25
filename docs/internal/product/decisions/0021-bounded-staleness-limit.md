# 0021. Bounded staleness limit

Status: accepted
Date: 2026-08-23
Requirements: FR-12, FR-13, NFR-2, NFR-3, NFR-21

## Context

Doc 12 §4.6 (hazard H4) gives the safety condition M5's deletion delay must
satisfy, but supplies a value for none of its terms. This ADR sets one:

```
deletion_delay  >  max_metadata_staleness  +  max_in_flight_fetch_duration  +  clock_skew
```

`max_metadata_staleness` is the bound doc 12 §4.6 point 2 requires an agent to
enforce as a circuit breaker: *"If `now - last_delta_received >
staleness_limit`, the agent stops serving from cache and either errors out or
forces a coordinator round-trip. This converts unbounded staleness into a hard
bound you can put in the inequality above."* Without a concrete value this
milestone cannot build that circuit breaker, and M5 cannot set a deletion
delay it can prove safe.

Doc 10 #11 leaves the number open. The only figure the corpus cites nearby is
turbopuffer's own bound — *"turbopuffer does exactly this with its ~1 h
eventual-consistency bound"* (doc 12 §4.6 point 2) — offered as precedent for
the *mechanism* (bound it explicitly, don't leave it unbounded), not as a
number to adopt. turbopuffer's ~1 h figure is sized for a structurally
different system: CAS-on-object-storage sequencing at ~1 write/sec/namespace
(doc 12 §4.5) — which, per doc 14 §2's correction that `ADR-0020` quotes
verbatim, is a **commit-rate ceiling bounding *latency* to a 200 ms–1 s floor
per write, not a throughput ceiling**. ⚠️ That floor is three orders of
magnitude short of an hour, so the ~1 h bound is **not** a consequence of
write-path granularity. Doc 08 §2 records what it actually is: an **opt-in**
eventual-consistency mode over a **strongly-consistent default**, where a
query otherwise linearly scans the un-indexed WAL tail (p50 ~10 ms) and where
*"over 99.8% of queries return consistent data even with eventual
consistency"*. It is a worst-case ceiling on a path the caller selects, not a
staleness that system runs at. Doc 12 §4.6 offers it as precedent for the
*mechanism* — bound staleness explicitly rather than leaving it unbounded —
and not as a number to adopt.

The corpus does supply the numbers this project's propagation is **modelled**
at — ⚠️ doc 12 §4.5's latency table is marked **[Synthesis]**, which that
document's reading guide defines as "parameterized estimates, not published
figures", and the ~1 ms row's own Source column reads *"one network RTT"*:

| Latency source | Magnitude |
|---|---|
| Write batching window | ~250 ms |
| Object-storage PUT | ~50–200 ms |
| Metadata push, coordinator → agent (in-AZ) | **~1 ms** |
| Coordinator round trip, if needed | ~10 ms p99 |

*"Metadata staleness sits two orders of magnitude below the batching latency
already inherent in the architecture."* Normal-case propagation is ~1 ms. The
staleness limit is not sized to catch normal propagation — it is sized to
catch the abnormal case (an agent partitioned from the coordinator's push
stream, a GC pause, a coordinator restart) before an agent serves arbitrarily
stale data indefinitely.

## Decision

**`max_metadata_staleness = 5 seconds` — a constant, not a per-deployment
knob — enforced per the doc 12 §4.6 mechanism**: an agent tracks `last_delta_received` per subscription; when
`now - last_delta_received` exceeds the limit, the agent stops serving Fetch
from cache and forces a coordinator round trip (`AtLeast` semantics) rather
than continuing to answer from a cache that might be arbitrarily behind.

Five seconds is roughly 5,000× the ~1 ms normal-case push latency doc 12 §4.5
**models** — ⚠️ an assumption about `M3.9`'s push subscription, which is not
built yet, and not a measurement of it — generous headroom against transient jitter (a GC pause, a brief
network blip, a coordinator restart mid-lease-handoff) so the circuit breaker
does not flap under ordinary conditions — while staying two to three orders of
magnitude below turbopuffer's ~1 h figure, which this project's own
~1 ms modelled propagation gives no reason to need. It is explicitly
**provisional** — but ⚠️ **provisional is not configurable**. `AGENTS.md`
non-negotiable 2 governs it, and rule 2's text is *"thresholds are constants no
environment can move"*; `check-drift.sh` is that rule's gate — ⚠️ **but it does
not cover this value yet, and saying so is the point of writing it down.** Its
`THRESHOLD_RE` matches `staleness_limit` and `max_metadata_staleness_ms`; it
does **not** match `max_metadata_staleness`, the spelling this ADR, doc 10
#11, `M5.md` and the H4 inequality all use. That script's own header
adds that a new threshold needs both regex visibility and an
`m0-complete.sh` `NFR_CONSTANTS` entry, "and nothing will tell you that you
did neither". So the rule binds here and the gate does not: **`M3.10`, which
builds the circuit breaker, owns landing the constant under a spelling
`THRESHOLD_RE` actually matches** — `max_metadata_staleness_ms` and
`staleness_limit` both do — which is what lets `check-drift.sh` refuse an
environment read of it — ⚠️ a same-line grep, which its own header notes does
not catch a value reached through a config struct populated elsewhere. ⚠️ `m0-complete.sh`'s `NFR_CONSTANTS` is **not** the
other half here: it resolves entries with a shell-assignment grep and every
entry keys a `scripts/*.sh`, so a Rust `pub const` can never match it. It
becomes relevant only if this value is ever also spelled in a gate script.
Until the constant lands, non-negotiable 2 binds this number by rule and by
nothing executable, in exactly the sense `AGENTS.md` warns about. Revising
the number is an **amendment to this ADR**, never a deployment's config. ⚠️ And a
*downward* revision takes more than a propagation measurement, however real.
The 5 s is not sized against the healthy path at all — it is sized against the
abnormal cases the circuit breaker exists to catch (a GC pause, a partition, a
coordinator restart mid-lease-handoff), and no measurement of the healthy path
bounds those. Shrinking the constant is a *tightening*, so no gate objects,
and it lowers M5's `deletion_delay` floor toward the H4 404 this ADR exists to
prevent. The weakening direction
is *raising* it: a longer window before the circuit breaker trips means longer
exposure to stale reads. ⚠️ What a raise actually puts at risk is doc 12
§4.6's H4 inequality, which `deletion_delay` is sized against — **not**
FR-12 or FR-13, neither of which states a freshness guarantee at all. ⚠️ FR-12
is specifically about avoiding an *object-storage* round trip, and the circuit
breaker's forced round trip is to the coordinator, which doc 12 §4.5 counts as
a separate ~10 ms p99 operation — so FR-12 is untouched by this value in
either direction, and must not be cited in a later argument for raising it.

This closes doc 10 #11's *value* question for M3's purposes. It sets the
floor for M5's own decision (`deletion_delay > max_metadata_staleness +
max_in_flight_fetch_duration + clock_skew`) without deciding `deletion_delay`
itself — that composition, and the other two terms, are M5's to set when it
exists to reason about GC.

## Alternatives considered

**Adopt turbopuffer's ~1 h bound directly.** Rejected — ⚠️ **not** because
that system's writes are coarse, which is the misreading Context corrects
above: its ~1 commit/s/namespace is a *latency* floor of 200 ms–1 s per write,
and one commit carries up to ~10k documents, so ~10k writes/s per namespace
(doc 14 §2). Nor is ~1 h a staleness that system runs at:
doc 08 §2 makes it an **opt-in** ceiling over a strongly-consistent default,
and doc 12 §4.6 cites it as precedent for the *mechanism*, not the number.
⚠️ The reason not to inherit it is on **this project's own cost side**, which
needs no claim about turbopuffer's architecture at all: `max_metadata_staleness`
is a term in H4's inequality, so a 1 h value forces `deletion_delay > 1 h` and
holds every reaped object an extra hour for no correctness benefit — while
modelled ~1 ms propagation leaves 5 s already 5,000× clear of the flap
threshold.

**Leave it UNDERIVED, like NFR-13.** Rejected — but ⚠️ **not on the grounds
that this number is measured and NFR-13's is not.** Neither is. The
difference is in kind: NFR-13 (aggregate throughput) is an *empirical*
quantity, whatever the built system turns out to achieve, and only a benchmark
can say. `max_metadata_staleness` is a *chosen* design parameter, and the
choice is bounded below by the mechanism it drives (a circuit breaker that
must not flap under ordinary propagation jitter) and above by the H4
inequality it feeds. A value is defensible inside that band today; a later
measurement would *narrow* the band, not unblock the choice. Refusing to pick
would block M3's read path and M5's deletion delay on a benchmark that is not
what is actually missing.

**Size it off `fetch.max.wait.ms` (Kafka's own default, 500 ms) instead of the
push-latency table.** Considered: it has the appeal of being a number a Kafka
operator already recognizes. Rejected because it conflates two different
things — `fetch.max.wait.ms` bounds how long one Fetch blocks waiting for new
data (doc 12 §4.4's `handle_fetch` sketch), while `max_metadata_staleness`
bounds how long a cache may go **without confirmation it is still current**
before refusing to serve from it at all. The two are allowed to differ, and
tying them together would make the staleness circuit breaker move whenever a
client's per-request wait tuning changes, which is not what H4's safety
argument needs.

## Consequences

**Makes easy:** M3's read path has a concrete, testable threshold — a fault
injection that stalls the coordinator's push stream for longer than 5 s has an
unambiguous, assertable required response (stop serving cache, force a round
trip) rather than an open-ended "should probably do something eventually."

**Makes hard:** nothing new; this is a circuit breaker on an already-required
mechanism, not new machinery a reader has to reconcile with anything else.

**Forecloses:** treating `max_metadata_staleness` as a **wire-visible**
protocol constant. Nothing a client negotiates or observes depends on the
number, so revising it needs no migration and no version bump. ⚠️ It remains a
constant in this codebase's sense (non-negotiable 2): what a revision is cheap
in is *migration cost*, not mutability — it is an amendment to this ADR. ⚠️ A
measurement-informed replacement is *not* automatically the expected one — see
the Decision's note on why a healthy-path measurement cannot justify shrinking
this constant.

## Related

`docs/internal/product/decisions/0020-offset-sequencing-and-coordinator-shape.md`
establishes the push/coordinator shape this staleness bound is enforced
against.
