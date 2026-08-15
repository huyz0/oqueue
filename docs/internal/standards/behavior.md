---
title: "Behavior"
description: >
  Read when a change is visible to a Kafka client, an operator, or another tenant — protocol responses, defaults, degradation under a dependency failure, logs, or the shape of anything persisted to object storage.
tags: [code, compatibility, degradation, observability, defaults]
applies_to: ["*oqueue-broker/*", "*oqueue-coordinator/*", "*oqueue-codec/*", "*oqueue-crypto/*", "*oqueue-store/*", "*oqueue-compact/*", "bin/oqueue/*", "*/bin/oqueue/*", "docs/internal/product/requirements.md"]
---

# Behavior

The other four code standards govern how the Rust is written —
[rust-style.md](rust-style.md), [error-handling.md](error-handling.md),
[async-concurrency.md](async-concurrency.md), [contracts.md](contracts.md).
This one governs what the running broker *promises*, to a client speaking
Kafka's wire protocol, to an operator reading a log or a metric, and to a
tenant relying on isolation from another tenant — independent of which code
path implements the promise, and stable across a refactor that changes
every one of them.

## Protocol compatibility

1. **Advertise only what is actually implemented.** `ApiVersions` names the
   supported range for every API this broker really answers; nothing is
   advertised speculatively. → FR-2, and a conformance test asserting no
   advertised API returns `UNSUPPORTED_VERSION`
2. **A protocol version, once shipped, is never silently withdrawn.**
   Removing support for a version a real client library still sends is a
   compatibility break for whoever is still on it — see rule 19 for what
   "silently" excludes.
3. **An unmodified Kafka client must interoperate**, full stop — not "an
   unmodified client configured a specific way." FR-1's conformance target
   is `librdkafka`, `kafka-python`, and the Java client as they actually
   ship, not as a spec reader would idealize them.

## What a client may rely on

4. **A retried idempotent produce is deduplicated by producer ID and
   sequence number, not by content.** Two records that happen to be
   byte-identical are not the same record; the sequence number is the
   identity. → FR-14
5. **Within one partition, offsets are monotonic and contain no gap** a
   client can observe as a hole in an otherwise-successful read — a
   compaction or retention deletion is a documented absence, not a silent
   one indistinguishable from data loss.
6. **A client-visible error carries a real Kafka error code**, mapped from
   whatever internal error produced it via the classification in
   [error-handling.md](error-handling.md) rule 7 — never a generic
   catch-all code that forces the client to guess whether retrying will
   help.

## Defaults and configuration

7. **Changing a default is a compatibility break**, treated with the same
   weight as removing a protocol version (rule 2). It gets called out in the
   commit message and, once release notes exist, in them — not folded
   silently into an unrelated change, which is the same discipline
   [git.md](git.md)'s "never mix a refactor with a behaviour change" rule
   states for code.
8. **A config value out of range fails loudly at startup, not silently
   clamped to the nearest valid value.** A clamp is a behavior the operator
   asked to not have, applied without telling them. See
   [milestones/M12.md](../product/milestones/M12.md) task 11 for the
   worked case — a retention value that would violate the GC safety
   inequality is refused, not rounded.
9. **A single binary's role is chosen by configuration, and every role
   answers a start-up health probe honestly before it accepts traffic.**
   → FR-50

## Degradation under a dependency failure

10. **"Degraded, keep routing" and "down, stop routing" are two distinct,
    named states, and a component reports which one it is in rather than a
    single boolean "healthy."** This is the distinction
    [milestones/M6.md](../product/milestones/M6.md) makes load-bearing for
    graceful degradation, and it is a behavior contract, not an
    implementation detail — an operator's routing decision depends on
    telling the two apart.
11. **A dependency outage degrades a stated, bounded subset of behavior —
    it never silently produces a wrong answer in place of an error.** A
    KMS outage stalls BYOK reads for the topics it affects; it must not
    cause any topic's reads to return stale or incorrect data while
    reporting success. See [milestones/M8.md](../product/milestones/M8.md)
    and [milestones/M15.md](../product/milestones/M15.md) task 9.
12. **Every operation that can degrade names, in its own documentation, what
    it does when each of its dependencies is unavailable.** "Undefined" is
    not an acceptable answer for a dependency this component is known to
    have.

## Observability is a promise, not an aspiration

13. **Every log line has a fixed, structured schema** — not free-form
    interpolated text — reusing [error-handling.md](error-handling.md)'s
    redaction rules so a log line cannot leak key material by construction.
    → [milestones/M12.md](../product/milestones/M12.md) task 6
14. **FR-52's "diagnosable from metrics, logs, or traces alone" is verified
    against a written list of scenarios, and the list is part of this
    standard's scope, not an afterthought of the milestone that happens to
    ship it.** See [milestones/M12.md](../product/milestones/M12.md) tasks
    9–10 for the scenario list and the review against it.
15. **A metric, once published, is not silently renamed or re-scoped.**
    Dashboards and alerts key on the name; treat a metric's name and label
    set as part of the external contract, same weight as rule 2.

## Format and cross-version compatibility

16. **Anything persisted to object storage names its own format.** The
    region header's algorithm field (`alg`, value `none` on the default
    path — see [milestones/M1.md](../product/milestones/M1.md) task 18)
    exists so a future reader does not have to assume; every persisted
    format this project adds follows the same rule.
17. **A FIPS build reads what a non-FIPS build wrote, and vice versa where
    the algorithm allows it.** → FR-43, [security.md](security.md) rule 16
18. **Nothing this project persists is read only by the version of the code
    that wrote it.** A format change ships a reader for the old format
    alongside a writer for the new one; the migration window is stated, not
    assumed instantaneous.

## No silent behavior change

19. ⚠️ **A behavior change visible outside the process is never a side
    effect of a change described as "just a refactor."** If a commit's
    message doesn't mention a visible behavior change, a reviewer finding
    one is a defect in the commit, not a bonus improvement — the same
    "never mix a refactor with a behaviour change" rule [git.md](git.md)
    already names, restated here because it is this standard's failure
    mode specifically.

## What has no gate

**Whether a degradation is "a bounded, stated subset" or has quietly grown
past what was documented.** Nothing currently diffs the documented
degradation behavior against the code that implements it.

**Whether a log message is genuinely useful to the scenario it claims to
diagnose**, as opposed to merely present. [Milestones/M12.md](../product/milestones/M12.md)'s
review-against-scenarios step is the closest thing to a gate this has, and
it is a review, not a script.

## See also

- Protocol conformance targets: [requirements.md](../product/requirements.md) FR-1, FR-2, FR-14
- Graceful degradation as a milestone concern: [milestones/M6.md](../product/milestones/M6.md)
- The admin/operability surface this standard partly specifies:
  [milestones/M12.md](../product/milestones/M12.md)
- Error classification a client-visible error code is built from:
  [error-handling.md](error-handling.md)
