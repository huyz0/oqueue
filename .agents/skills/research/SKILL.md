---
name: research
description: Find whether a question is already answered in the research corpus before investigating it fresh. Use before any web search about object storage, Kafka protocol, Rust performance, competing systems, or cost models — the answer is often already here, cited.
---

# Research corpus

22 documents, ~110,000 words, compiled before any code existed. ⚠️ **Do not read
it wholesale, and do not re-research what it already answers with citations.**

## How to navigate

1. **Start at `docs/researches/README.md`.** It has a document map and a tag
   index. Use them.
2. **Match the question to a tag**, then read that one document's relevant
   section. One section usually suffices.
3. **Check `10-open-questions.md`** before assuming a decision is made. It is
   the decision log; several things that look settled are not, and several that
   look open were resolved.

## What is where, roughly

| Question about | Document |
|---|---|
| The reference architecture | 01 |
| Kafka wire protocol, KIPs | 02 |
| Competing systems | 03, 16 |
| S3/GCS behaviour, pricing, consistency | 04 |
| Rust crates for a layer | 05 |
| A hard design problem | 06 |
| Finding data in objects, API cost | 12 |
| Coordinator recovery | 13 |
| Scale and metadata | 14, 15 |
| How fast can it be | 17 |
| Benchmarking, build config, SIMD, unsafe | 18 |
| Workspace, testing, mutation | 19 |
| Cross-compilation, glibc, artifacts | 20 |
| How this project is built | 21 |
| Encryption, BYOK, FIPS | 22 |

## Reading the markings

Every claim carries provenance, and the distinction matters:

- **[Documented]** — externally cited, verifiable
- **[Measured]** — a published benchmark with methodology
- **[Practice]** — established practice, stated as our standard
- **[Assessment]** / **[Design]** — this project's own reasoning, not a fact
- **[Vendor]** — vendor-reported and not independently verified

⚠️ **Watch for correction banners.** Several documents carry ⚠️ notes where
later work invalidated an earlier claim — a source-code study overturned things
taken from marketing material, and a clarified requirement overturned a design
conclusion. **When code and marketing disagree, the code wins.**

## If the corpus does not answer it

Then research it — and **write the answer back**. A finding that lives only in a
conversation is a finding that will be researched again. If a design thread runs
more than a couple of exchanges, it has earned a home in the corpus.
