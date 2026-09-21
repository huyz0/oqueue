# 0061. Bounded DescribeConfigs surface

Status: accepted
Date: 2026-09-21
Requirements: FR-40, FR-44, FR-53

## Context

Kafka's `DescribeConfigs` API can ask for broker or topic settings, including
settings that may contain passwords or key material. Oqueue currently has one
authoritative retention owner — the compaction/retention subsystem's
`DEFAULT_RETENTION_MS` — and no durable per-topic configuration state. A
successful response must describe what the broker actually enforces, not a
Kafka-shaped setting that is silently ignored.

## Decision

M12.5 serves only topic resources (`resource_type = 2`) that already exist and
are visible to the caller. The supported key is `retention.ms`, reported as
the authoritative `DEFAULT_RETENTION_MS` effective default, read-only, and
non-sensitive. A null key filter returns that one entry; an empty filter
returns no entries. Unknown keys return `INVALID_CONFIG` without echoing the
requested key. Broker resources, missing topics, and unauthorized topics use
their respective resource errors. Error messages are generic and never carry
configuration values, secret names, credentials, or key references.

No per-topic override is implied by this read surface. `AlterConfigs` remains
responsible for adding a durable setting only when it has a live owner,
validation rule, persistence boundary, and update semantics of its own.

## Alternatives considered

- **Report Kafka's broad default configuration catalogue.** Rejected because
  most entries have no live owner in this broker; reporting them would make a
  client believe a setting is enforced when it is not.
- **Accept and omit unknown keys.** Rejected because a successful response for
  an ineffective request violates the explicit M12 policy and hides operator
  mistakes.
- **Persist per-topic settings in DescribeConfigs.** Rejected because reads do
  not establish the write, recovery, validation, or live-update contract that
  M12.6 must provide.

## Consequences

Admin clients can inspect the one retention policy that currently governs
expiry, with authorization and secret-redaction guarantees. The surface is
deliberately smaller than Kafka's, so unsupported settings fail loudly and
future settings need an owner-backed ADR or an amendment to the configuration
write task before they are advertised.
