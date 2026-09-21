# ADR-0067: Connected operational traces

Status: accepted
Date: 2026-09-22

## Context

M12's logs and bounded metrics identify an operation and its aggregate outcome,
but an operator still needs to locate time spent in a produce or fetch. The
object-store and KMS calls are asynchronous seam calls, so independent events
would lose the relationship between a client operation and the dependency that
delayed or refused it. Trace attributes must also remain safe when a request
names a topic, object key, or customer key.

## Decision

Use `tracing` spans in the broker for `produce` and `fetch`, with child spans
named `dependency` for object-store PUT/GET calls. The encryption caches add
child `kms` spans for wrap and unwrap calls. Each span carries only the fixed
operation/dependency, outcome, and bounded scope fields; object keys, topic
names, principals, credentials, wrapped keys, and provider error text are not
attributes. The span records `success` or `failure` before it closes.

Instrument futures rather than holding a span guard across an await, so the
parent relationship remains correct when the runtime moves work between
threads. The binary remains responsible for subscriber/exporter setup; library
crates emit spans but do not choose an exporter.

## Consequences

Successful and failed store/KMS calls can be located under the produce/fetch
operation that caused them, while the fixed field set prevents unbounded
cardinality and secret leakage. A trace backend is optional at runtime; without
a subscriber the instrumentation is inert. More detailed per-topic diagnosis
continues to use the bounded metrics policy from ADR-0066 rather than adding
topic names to every trace.
