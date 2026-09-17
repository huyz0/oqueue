# 0039. Compaction copies through the process, and measures what that cost

Status: accepted
Date: 2026-09-18
Requirements: FR-34 (compaction); NFR-11's cost model is what it bears on

## Context

`M1.18` deferred server-side range copy into `M5` (`ADR-0015`), on the finding
that `object_store` exposes it for neither backend. `M5.7` inherits that
decision and, since `M5.3` and `M5.6`, a second one with it: what unit the
compaction cost estimate is denominated in.

⚠️ **The upstream gap is unchanged, checked against the pinned 0.14.1 source
rather than against the issue tracker.** `src/aws/` contains no
`UploadPartCopy`, no `x-amz-copy-source-range` and no `copy_part`; `src/gcp/`
contains no `compose`. The public surface is `copy`, `copy_opts` and
`copy_if_not_exists`, all whole-object.
`apache/arrow-rs-object-store#121` has not closed.

⚠️ **And a server-side range copy could not express this compaction anyway**,
which is the part `ADR-0015` could not know because the executor did not exist.
A merge does three things a copy cannot: it **reorders** regions by topic,
partition and offset so a fetch of one partition stays one ranged GET
(`M5.5`); it **rewrites the footer**, which is computed from the new layout and
is not any input's bytes; and it produces parts that are unions of regions from
several inputs. `UploadPartCopy` copies a contiguous range of one source into
one part, with a 5 MiB floor on every part but the last — so even the regions
that are contiguous in an input are mostly too small to be a part. What
server-side copy could serve is a whole-object move, which is the one case
compaction has no reason to perform.

Separately, `M5.3` shipped a records budget and no byte one, because the
history tier of the index holds no byte range: a byte-denominated estimate
needs a GET per candidate, which is the cost the estimate exists to bound.
`M5.6` then made a run's *request* count a function of the part size and the
output's length, so `CostEstimate::puts`, which counts output objects, stopped
tracking requests.

## Decision

**Compaction reads and writes through the process. There is no server-side
copy path, and `copy_range` is not built.**

**The estimate is denominated in records and says so; bytes and requests are
measured after the run, not modelled before it.**

1. `CostEstimate` carries `gets`, `puts` and `records_rewritten` and no byte
   length. Its doc says `puts` counts output *objects* and that a round's
   request count is not derivable from it.
2. `MergeOutcome::written` carries what the write actually cost: record bytes
   moved, footer excluded, and **parts** written, footer part included. Both
   come from `BundleStream`, which is the one place that sees them.
   ⚠️ **Parts are not requests**, and this decision does not claim they are: a
   backend's `CreateMultipartUpload` and `CompleteMultipartUpload` are issued
   inside the writer and invisible at this seam, so a real S3 write costs
   `parts + 2`. `roadmap.md`'s "Multipart's true per-request cost" row (M14,
   from M1) already owns making that observable, and it is still open.
3. ⚠️ **Nothing multiplies records by a modelled record size.** `M5.7`'s row
   offered that as a third answer and it is rejected: **there is no modelled
   record size in this repository.** `read_amp.rs` called 1 KiB "this
   project's modelled" record and no requirement, NFR or research document
   states one — the phrase was an assumption that had acquired a citation.
   Budgeting egress against it would make the estimate an estimate of an
   estimate whose inner term is invented.

## Alternatives considered

- **Hand-rolled signed requests** for `UploadPartCopy` and `compose`. Rejected
  for `ADR-0013`'s reasons, which have not changed — SigV4, the credential
  chain, the retry semantics and the error taxonomy, maintained by us — and now
  for a second reason that is specific to this caller: by the argument above,
  the operation it would buy is not the operation compaction performs.
- **Wait for upstream #121.** Rejected as a plan: it has been open since
  2023-10-20 and compaction is this milestone's subject. If it closes, this
  decision is cheap to revisit — nothing here depends on the absence of a copy
  path, only on not having built one.
- **A byte length from the object's footer.** That is a GET per candidate, at
  sweep cadence, over every candidate partition — the cost `ADR-0036` decision
  1 exists to avoid paying.
- **A byte length in the index.** An `ObjectRef` deliberately carries none:
  ~16 bytes against the ~40-byte inline budget doc 14's arithmetic rests on,
  paid by every entry to serve the planner alone. This is `M5.9`'s territory
  and if the re-keying changes what an entry holds, this decision is worth
  re-reading.

## Consequences

- Compaction pays egress. That is the cost FR-34 buys read amplification down
  with, and it is now a number an operator can read rather than infer.
- `M14` can calibrate a record-size model against `MergeOutcome::written`,
  which is the first place in this repository where records and bytes are both
  measured over the same data — recorded in `roadmap.md`'s deferral table
  rather than only here, because a milestone named in prose alone owns
  nothing.
- ⚠️ **NFR-11's cost model cannot be checked against the estimate alone, and
  is not fully answered by the outcome either.** The outcome gives bytes
  exactly and parts exactly; the two requests per object that bracket a
  multipart write are visible at neither, and `roadmap.md`'s M14 row is where
  that closes. A reviewer asking "what does a round cost in requests" is
  answered "parts, plus two per object, and nothing here counts the two".
- Forecloses: nothing. A server-side copy path remains available to a later
  milestone, and would arrive as a new ADR superseding this one.
