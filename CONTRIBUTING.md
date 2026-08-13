# Contributing

Thanks for looking. Being honest about the current state saves your time.

## Right now

**Code contributions are not being accepted yet.** Not because they are
unwelcome in principle, but because the system that would review them is itself
still being built (milestone M-1), and merging code into a project whose gates
do not exist yet would defeat the point of having them.

**Issues and design discussion are very welcome.** Particularly:

- **Corrections.** The research corpus in [`docs/researches/`](docs/researches/README.md)
  makes many specific, cited claims about S3/GCS behaviour, competing systems,
  and Rust tooling. If one is wrong, saying so is the most valuable thing you
  can do — one document already exists solely because reading AutoMQ's source
  invalidated several claims taken from its marketing material.
- **Experience reports.** If you have run an object-storage-native broker in
  production, what broke is more useful than any benchmark.
- **The open questions.** [`docs/researches/10-open-questions.md`](docs/researches/10-open-questions.md)
  lists what is undecided and why. Several are blocked on information rather
  than on effort.

## When code opens up

The working agreement is in [AGENTS.md](AGENTS.md) and will apply to everyone,
human or agent. The parts most likely to surprise:

- **One task equals one commit equals one change that leaves the tree green.**
  Commits go directly to `main`; there are no feature branches. This only works
  because every commit is small and green, and the hooks are what make that true
  rather than aspirational.
- **Never lower a threshold or delete a test to make a check pass.** Thresholds
  are constants that no environment can move.
- **Business logic is sans-I/O.** If it needs a socket, a clock, or an object
  store to test, it is in the wrong layer.
- **`unsafe` lives in three crates only**, and never in the async or concurrency
  layer. Everything else is `#![forbid(unsafe_code)]`.

## A note on how this is built

oqueue is written entirely by AI agents, spec-first, with no manually written
code. This is stated openly because it changes what you should expect: the
review discipline, the mutation-testing gate, and the author/reviewer separation
described in [docs/researches/21](docs/researches/21-ai-development-loop.md)
exist specifically because no human reads every line.

If that makes you want to scrutinise the output more carefully, good — that is
the correct response, and bug reports are the useful form of it.

## Code of conduct

Be decent. Disagree about the engineering, not about the person. Reports of
behaviour that makes participation unpleasant go to the address in
[SECURITY.md](SECURITY.md).
