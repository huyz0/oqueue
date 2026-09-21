---
title: "M12 diagnostic runbooks"
description: >
  Operator queries for distinguishing the five M12 failure scenarios from
  bounded metrics, structured events, traces, health, and Kafka inspection.
tags: [operations, diagnostics, m12, fr-52]
---

# M12 diagnostic runbooks

This is the FR-52 scenario review. It is deliberately an operator document:
an independent reviewer can follow the injection, collect the named signals,
run the queries, and identify the distinguishing observation without
consulting implementation source.

The runbooks use three evidence streams:

1. the broker's bounded `OperationalMetricsSnapshot` and `HealthSnapshot`,
   exported by the deployment's diagnostic collector as `snapshot.json`;
2. structured tracing events/spans, one JSON object per line in `events.jsonl`;
3. Kafka inspection from the affected listener, using `kafka-consumer-groups`
   and the AdminClient where the scenario is group- or coordinator-scoped.

The snapshot is observational, not a correctness authority. Its atomic fields
may come from adjacent instants. Capture two snapshots at least one sampling
interval apart and compare deltas; never infer health from a missing series.
Routine partition samples are bounded by `MAX_SCOPED_PARTITIONS` (256), and a
non-zero `dropped_scoped_samples` means a named partition may not be present.

## Common collection and query steps

Record the node `role`, `state`, `replay_in_progress`, and
`coordinator_task_alive` from `snapshot.json`. Record these metric fields:
`writes.count`, `writes.failures`, `writes.total_latency_micros`,
`coordinator_ready`, `coordinator_failures`, `compaction_backlog`,
`storage_failures`, `key_domain_failures`, `encryption_cache_hits`,
`encryption_cache_misses`, `index_entries`, `dropped_scoped_samples`, and
`partitions[]`.

```sh
jq '{health, metrics: {writes, coordinator_ready, coordinator_failures,
  compaction_backlog, storage_failures, key_domain_failures,
  encryption_cache_hits, encryption_cache_misses, index_entries,
  dropped_scoped_samples}}' snapshot.json
jq '.metrics.partitions[] | select(.topic == "orders" and .partition == 3)' snapshot.json
jq 'select(.event == "dependency_failure" or .span == "dependency" or .span == "kms")' events.jsonl
```

The fixed event fields are `event`, `operation`, `outcome`, `scope`,
`correlation_id`, and `principal` for admin events; dependency events add
`dependency`. The `produce`, `fetch`, `dependency`, and `kms` spans carry the
same bounded `operation`, `outcome`, `scope`, and dependency fields. Do not
paste credentials, key identifiers, raw backend errors, or topic data into an
incident record.

## Scenario: stalled partition

**Injection.** For a test topic with one named partition, pause its producer
or make the object-store `put` dependency return a transient failure. Keep a
second partition healthy and continue producing to it. Restore the dependency
after two samples.

**Signals.** Inspect `metrics.partitions[]` for the affected topic/partition,
`metrics.writes.failures`, `metrics.writes.total_latency_micros`, and
`metrics.storage_failures`. Correlate `produce` spans with `dependency` spans
whose dependency is `object_store` and operation is `put`; compare with
`fetch` spans. A `dependency_failure` event has `scope=produce` or
`scope=fetch`.

**Query steps.** Compare the affected and healthy partition rows in two
snapshots. Then run the event query below and group by `scope`, `operation`,
and `outcome`:

```sh
jq 'select(.span == "produce" or .span == "fetch" or
  (.event == "dependency_failure" and .dependency == "object_store")) |
  {span, event, scope, operation, outcome, dependency}' events.jsonl
```

**Distinguishing observation.** Rising lag plus failed/slow `produce` and
`object_store.put` evidence identifies storage blockage. Rising lag with no
storage failures but a `coordinator.commit` failure identifies sequencing or
coordinator blockage. No produce observations identifies ingest/client
silence; fetch-only failures identify serving/storage reads. If the healthy
partition also stalls, investigate a shared dependency or node failure rather
than a partition-local fault.

## Scenario: lagging consumer group

**Injection.** Join a named group on two partitions, then stop polling from
one consumer while leaving its membership alive. Keep another group consuming
normally. Resume polling after two samples.

**Signals.** Use `partitions[]` lag and its sample-drop bound, then inspect
group membership and assignment with `DescribeGroups`/`ListGroups` from the
AdminClient. Compare committed offsets and log end offsets with
`kafka-consumer-groups --describe`. The broker snapshot is partition-scoped,
so the group identity comes from Kafka inspection, not from a fabricated
per-group metric.

**Query steps.**

```sh
kafka-consumer-groups.sh --bootstrap-server "$BOOTSTRAP" \
  --describe --group "$GROUP"
jq '.metrics.partitions[] | {topic, partition, lag}' snapshot.json
jq 'select(.span == "fetch") | {operation, outcome, scope}' events.jsonl
```

**Distinguishing observation.** Growing committed-to-end offset difference
with stable membership and assignment identifies consumption/polling lag.
Missing members or a changing assignment identifies membership/rebalance
lag. A group whose assignment is healthy but whose partition rows do not
advance points back to the stalled-partition runbook. A non-zero
`dropped_scoped_samples` prevents a negative conclusion from an absent row.

## Scenario: coordinator failover in progress

**Injection.** Stop the coordinator task or restart the coordinator role after
capturing a healthy baseline. Do not inject a storage or KMS failure. Restore
the coordinator and wait for replay and group transitions to settle.

**Signals.** Inspect `health.state`, `health.replay_in_progress`,
`health.coordinator_task_alive`, `metrics.coordinator_ready`, and the delta of
`metrics.coordinator_failures`. Correlate `dependency_failure` events whose
dependency is `coordinator` or `group_log`, and check whether group APIs
return coordinator-unavailable responses during the window.

**Query steps.**

```sh
jq '{role: .health.role, state: .health.state,
  replay: .health.replay_in_progress,
  task: .health.coordinator_task_alive,
  ready: .metrics.coordinator_ready,
  failures: .metrics.coordinator_failures}' snapshot.json
jq 'select(.event == "dependency_failure" and
  (.dependency == "coordinator" or .dependency == "group_log"))' events.jsonl
```

**Distinguishing observation.** `replay_in_progress=true`,
`coordinator_ready=false`, or a dead coordinator task with coordinator/group
log failures identifies failover. The recovery is progressing when replay
ends, the task is alive, readiness returns, and the failure counter stops
increasing. A simultaneous `storage_failures` increase means storage is an
additional cause; a KMS-only increase belongs to the KMS runbook and must not
be used to explain coordinator unavailability.

## Scenario: compaction backlog

**Injection.** Generate enough eligible history to create compaction work,
then pause the compaction worker or make its deletion operation fail. Keep
produce and fetch traffic running, and restore the worker after two samples.

**Signals.** Compare `metrics.compaction_backlog` and `metrics.index_entries`
over time. Inspect `writes` for unrelated traffic, `storage_failures`, and
`dependency` spans for compaction reads/writes/deletes. A backlog is a count,
not proof of a stuck worker; its slope and the accompanying failure events
are required.

**Query steps.**

```sh
jq '{backlog: .metrics.compaction_backlog,
  index_entries: .metrics.index_entries,
  storage_failures: .metrics.storage_failures}' snapshot.json
jq 'select(.span == "dependency" and .scope == "compaction") |
  {dependency, operation, outcome, scope}' events.jsonl
```

**Distinguishing observation.** A rising backlog with successful dependency
spans and stable storage failures indicates insufficient compaction capacity.
A rising backlog with failed object-store operations identifies repeated
storage failure. A rising `key_domain_failures` with KMS spans identifies a
blocked key domain instead. If backlog falls while index entries remain
bounded and produce/fetch continue, compaction is making progress.

## Scenario: KMS outage or key revocation

**Injection.** In a BYOK test domain, make the fake/provider reject unwraps
for one customer-key scope (or revoke that key), while leaving a second key
domain available. Do not change object storage. Restore the key after two
samples.

**Signals.** Inspect `metrics.key_domain_failures`,
`metrics.encryption_cache_hits`, `metrics.encryption_cache_misses`, and
`health.state`. Correlate `kms` spans with `operation=unwrap` or `wrap`,
`outcome=failure`, and `scope=dek`. Verify that unrelated-domain reads still
produce successful fetch spans.

**Query steps.**

```sh
jq '{state: .health.state, key_failures: .metrics.key_domain_failures,
  cache_hits: .metrics.encryption_cache_hits,
  cache_misses: .metrics.encryption_cache_misses}' snapshot.json
jq 'select(.span == "kms" or (.event == "dependency_failure" and
  .dependency == "kms")) | {span, event, operation, outcome, scope}' events.jsonl
```

**Distinguishing observation.** KMS failure is established by failed `kms`
unwrap/wrap spans and an increasing `key_domain_failures` counter, with
`health.state=degraded` rather than a false whole-node outage. A warm-cache
read may continue briefly, so cache hits do not disprove an outage; increasing
misses followed by failed unwraps do. If unrelated key-domain reads continue,
the blast radius is scoped correctly. Object-store failures without KMS spans
belong to the stalled-partition or compaction runbook.

## Review record

The review is complete when an independent operator can identify each
injected condition from the named signals and distinguishing observation,
without reading source. The checker paired with this document verifies that
all five scenario sections retain their injection, signals, query steps, and
distinguishing-observation fields, and that each names the telemetry required
to make the observation.
