# 0064. Live principal quota administration

Status: accepted
Date: 2026-09-22
Requirements: FR-40, FR-45, FR-53

## Context

`PrincipalQuota` already refuses concurrent requests after one authenticated
principal reaches its shared in-flight bound. M12 must expose that supported
bound through Kafka's quota administration path while preserving the
principal isolation guarantee. The existing environment value is a useful
default, but an update must reach every connection sharing the broker's quota
object immediately.

## Decision

1. Serve Kafka `DescribeClientQuotas` and `AlterClientQuotas` versions 0 and 1.
2. Support only the named `user` entity and the `in_flight_requests` key. A
   user override is an in-memory live policy entry; removing it restores the
   startup default. The API reports explicit overrides, never credentials or
   current request contents.
3. Protect both APIs with the existing `AdminOperation::AlterQuotas` grant.
   Once credentials are configured, missing authority returns
   `CLUSTER_AUTHORIZATION_FAILED` and does not inspect or mutate quota state.
4. Apply valid alterations to the shared `Arc<PrincipalQuota>` after
   validation. `validate_only` performs the same validation without changing
   live admission. Quota updates are process-local in v1; durable policy
   storage is a separate requirement from the request-concurrency primitive.

## Alternatives considered

- **Keep the wire APIs deferred.** Rejected because M12's acceptance requires
  an admin inspection/update path and clients already standardize on keys 48
  and 49.
- **Add a separate mutable quota service beside `PrincipalQuota`.** Rejected
  because it would create two policy owners and could let administration and
  admission observe different limits.
- **Expose Kafka's byte-rate quota keys.** Rejected because the broker only
  enforces concurrent request admission; reporting a rate that is not enforced
  would make a successful admin response misleading.
- **Persist quota overrides in the topic catalog.** Rejected because quotas
  are broker-wide principal policy, not topic metadata, and the current
  durability seam has no principal-policy owner.

## Consequences

Kafka admin clients can inspect and change the one quota the broker actually
enforces, and the change is immediately shared by all connections. Unsupported
quota dimensions are explicit errors rather than silent no-ops. Overrides are
lost on restart until a later durable policy store is selected; the startup
default remains available for every principal without an override.
