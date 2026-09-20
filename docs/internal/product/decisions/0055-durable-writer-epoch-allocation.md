# 0055. Allocate writer nonce epochs with durable create-only terms

Status: accepted
Date: 2026-09-20
Requirements: FR-42

## Context

`M8.10` derived `WriterEpoch` from `CoordinatorEpoch`. That fence identifies a
coordinator incarnation, but several writer processes can share one coordinator
and each process can restart its object sequence at zero. Reusing the same
epoch under one DEK repeats the first AES-GCM nonce.

## Decision

`oqueue-coordinator::DurableWriterEpochAllocator` stores one immutable object
per writer incarnation below its configured prefix. Terms are zero-padded
decimal values, and allocation reads the newest term then attempts to create
the next term with `Precondition::IfAbsent`. A concurrent loser retries after
the winner has advanced the durable counter. A successful term is the only
source from which the composer constructs `oqueue_core::WriterEpoch`.

The allocator rejects values beyond the nonce layout's 40-bit field and treats
malformed term payloads as metadata corruption. An immutable initialization
marker survives term objects and makes a missing term zero a refusal rather
than a reset to epoch zero. It does not use coordinator epochs, process
identity, wall-clock time, or a local counter.

## Alternatives considered

- **Reuse `CoordinatorEpoch`.** Rejected: one coordinator fence can cover
  multiple writer processes, so their first object sequence can collide.
- **Use process IDs, timestamps, or `WriterId`.** Rejected: each can repeat or
  be observed inconsistently across a restart.
- **Overwrite one shared counter object.** Rejected: it would need a backend
  compare-and-swap token and recovery logic for ambiguous acknowledgements;
  immutable create-only terms make the allocation race explicit and recoverable.

## Consequences

Writer epoch allocation now has object-store I/O and must happen before a
writer constructs a nonce minter. Terms and the initialization marker are
retained as the durable history of writer incarnations; cleanup must never
delete either while any DEK could still be used. The coordinator fence remains
the authority for coordinator state, but it no longer doubles as a per-writer
nonce epoch.
