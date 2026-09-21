# 0063. Consumer-group administration snapshots

Status: accepted
Date: 2026-09-22
Requirements: FR-40, FR-53

## Context

`DescribeGroups` and `ListGroups` need a consistent view of every group the
single broker coordinates. `GroupCoordinator::record` answers a named group's
state, but it cannot enumerate groups, so `ListGroups` would otherwise have to
maintain a second group index in the broker. The join-round bookkeeping owns
the live member roster, while its protocol metadata is opaque client data and
must not be copied into an administrative response.

## Decision

Add a read-only `records` snapshot method to the `GroupCoordinator` seam. It
returns owned `(GroupId, GroupRecord)` pairs, is sans-I/O, and is the sole
source for group enumeration and lifecycle state. The fake implements the
same snapshot semantics as its map.

The group metadata log keeps a small summary of the most recently closed join
round: member ids, protocol type, and negotiated protocol name. Replay restores
that summary beside the lifecycle state. It never stores or returns the
members' opaque subscription metadata or assignments in the admin path. A
group outside `Stable` state is described with no members or protocol data; a
named group absent from the coordinator returns Kafka's group-not-found
response shape. `ListGroups` filters this coordinator snapshot by the
requesting principal's `GroupGrants` before returning any group id.

## Alternatives considered

- **Enumerate the broker's join-round map.** Rejected because it is
  process-local bookkeeping and can disagree with the durable coordinator
  state; the durable summary is now written through the group metadata log.
- **Return the opaque join metadata in `DescribeGroups`.** Rejected because
  subscription metadata can contain tenant- or topic-sensitive client data;
  the admin API needs membership visibility without widening that exposure.
- **Add a second mutable group registry to the broker.** Rejected because it
  creates two enumeration authorities and another race with coordinator
  transitions.

## Consequences

The trait change is atomic with its fake and this ADR. Enumeration is a
bounded copy of the coordinator's current in-memory state, and authorization
is applied before group ids or member summaries enter a response. Roster
records contain no subscription or assignment bytes and are replayed before
serving admin requests.
