# 0047. No recovery scanner: commits go through the coordinator before the ack

Status: accepted
Date: 2026-09-19
Requirements: FR-51, NFR-20; mission: never require LIST on the read path

## Context

Doc 10 #14 is a fork (doc 13 §7): a WarpStream-style prefix-bounded recovery
scanner, which survives total coordinator loss but enumerates object storage,
or AutoMQ-style controller state, which never enumerates but keeps the
coordinator on the write path. Doc 10 notes the coupling: ack-before-sequencing
*requires* the scanner.

## Decision

**No scanner.** `ADR-0020` already put the coordinator on the write path — a
produce is acknowledged only after its commit is journaled — so no acknowledged
record exists that the log does not name, and recovery never has to discover
data by enumerating. An object PUT whose commit never landed is an orphan the
producer was never acknowledged for; reclaiming it is garbage collection
(`M5.23`, orphan reconciliation from a storage inventory), not recovery.

## Consequences

Recovery reads the log and nothing else. The cost is the one `ADR-0020`
already accepted: while a shard's coordinator is down, that shard cannot
acknowledge writes (reads continue from cached indexes, M6 task 12). A
Ripcord-style total-coordinator-loss mode is foreclosed.
