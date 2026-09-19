# 0048. Recovery times are measured and published, not targeted

Status: accepted
Date: 2026-09-19
Requirements: NFR-22 (states no number)

## Context

Doc 10 #15 asks for target RTOs. Doc 13 §2 argues hot-standby failover and
cold rebuild are different scenarios with different costs, and §11 records
that no peer system publishes a cold-rebuild figure to anchor a target against.

## Decision

**No target is set.** M6's gate times hot-standby failover and cold rebuild as
two separate numbers and records them; it asserts that each was measured, not
that either is under a bound. A target written now would be an invented number
every later capacity argument rested on. M14, which measures against real
object storage, is where a target can be derived; until then the recorded
numbers are what an operator is told.
