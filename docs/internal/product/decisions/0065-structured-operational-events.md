# 0065. Structured operational events

Status: accepted
Date: 2026-09-22
Requirements: FR-44, FR-52

## Context

M12 needs operational evidence for admin decisions and dependency failures.
The fields must be machine-queryable, bounded, and safe when a broker handles
credentials, customer key domains, and arbitrary client input. Library crates
must emit events without choosing a process-wide output policy, while the
composition root must make serving a real broker observable.

## Decision

Use `tracing` as the library-side event seam and initialize a JSON
`tracing-subscriber` formatter only in the serving binary. Define two fixed
schemas:

- `admin_operation`: `event`, `operation`, `outcome`, `correlation_id`,
  `principal`, and `scope`.
- `dependency_failure`: the same correlation fields plus `dependency` and
  `operation`, with the fixed outcome `failure`.

Principal names, correlation IDs, and bounded scope labels are the only dynamic
context. Credentials, error display strings, key IDs, and key material are not
event fields. The no-argument banner remains plain stdout because it exits
before subscriber initialization.

## Alternatives considered

- `log` plus an application-specific formatter: rejected because it does not
  provide structured event fields without rebuilding the context and loses the
  field-level subscriber seam needed by tests.
- `tracing-subscriber` in every library crate: rejected because a library must
  not install a global process policy, and consumers may already have a
  subscriber.
- Emitting JSON with `println!`/`eprintln!`: rejected because it couples event
  production to a stream, is difficult to capture without scraping text, and
  makes redaction a formatting convention rather than a field contract.

## Consequences

The broker emits safe, stable fields and the binary produces newline-delimited
JSON suitable for a log collector. Tests can capture event fields without
depending on timestamps, thread IDs, or OS-specific formatting. The schema is
deliberately small: detailed dependency error strings are omitted, so a future
diagnostic field requires an explicit security review and schema change.
