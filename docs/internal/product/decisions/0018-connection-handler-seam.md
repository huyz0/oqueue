# 0018. The connection `Handler` seam is bytes in, bytes out

Status: accepted; 2026-08-24 (`M2.21`): the response became
`Option<Vec<u8>>` — `None` closes the connection, the dispatcher's only
safe answer to a request no response schema fits (an unknown api key, an
unadvertised version outside `ApiVersions`' fallback)
Date: 2026-08-24
Requirements: FR-1

## Context

`M2.17`'s connection task needs a boundary to hand a decoded frame across:
the dispatcher (`M2.21`) is protocol-aware, the connection is not, and
whatever sits between them is the seam every API handler builds against —
`check-core-contract.sh` holds any `pub trait`'s method set to an ADR, and
this is the first one outside `oqueue-core`.

## Decision

`Handler` is one method: a request frame's bytes in, the complete response
body's bytes out (header included; framing stays the connection's). RPITIT
(`impl Future` in the trait), with a blanket impl for async closures so
tests need no named type. The connection knows nothing of headers,
versions, or `kafka-protocol` types.

## Alternatives considered

- **Typed messages across the seam** (decoded header + body structs) —
  rejected: it drags `kafka-protocol` types into the connection layer,
  couples the shell to the dependency `ADR-0017` confines to `oqueue-codec`,
  and buys nothing the dispatcher cannot do one call deeper.
- **`async_trait`** — rejected: a proc-macro dependency and a boxed future
  per request where RPITIT is native on the pinned toolchain.
- **A channel-based seam** (requests in one mpsc, responses out another) —
  rejected: it dissolves the per-request backpressure permit and reinvents
  the sequencing the writer already owns.

## Consequences

Easy: dispatchers and tests are closures; the connection layer never
changes when an API is added. Hard: a handler cannot stream a response
body — `Vec<u8>` whole-response is the contract until a real need (fetch
of a huge batch) forces a streaming seam, which would supersede this.
