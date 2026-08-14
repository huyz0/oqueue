---
title: "Backlog"
description: >
  Read when picking up work. Only the current milestone is decomposed.
tags: [product, tasks, planning]
---

# Backlog

One task equals one commit equals one change that leaves the tree green. If a
task cannot be finished in one green commit, split it before writing code.

Task IDs are stable. Completed tasks stay here with their commit reference so
the history of why something was done survives.

Decomposition rule: only the current milestone is decomposed in detail. Future
milestones stay as [roadmap](roadmap.md) entries with a
[plan](milestones/README.md) until their turn. ⚠️ **A plan is not a
decomposition** — it is a hypothesis, it carries no task IDs, and its items are
re-derived rather than copied when a milestone opens. Read the plan's
"Decisions required first" before writing any of that milestone's code; see
[`sdd.md`](../standards/sdd.md) §Decomposition.

## M-1: AI development system

Build the system that builds everything else. Ordered by dependency — the
product docs come first because everything else references them, and `lib.sh`
comes before any gate because every gate sources it.

| ID | Task | Acceptance | State |
| --- | --- | --- | --- |
| M-1.0 | Repo foundation: `git init`, Apache-2.0 `LICENSE`, `.gitignore`, `rust-toolchain.toml`, and the research corpus placed under version control | Files exist; toolchain pins 1.97.1 and both Linux targets | done |
| M-1.1 | `AGENTS.md` + `CLAUDE.md` adapter, non-negotiables with honest enforcement marking | Every rule names its gate task or is marked unenforceable; no rule claims a gate that does not exist, and no link points at a file that is not there | done |
| M-1.2 | `mission.md` — what this is, what it costs, what it must never do | States the latency position and the scale target with citations into the corpus | done |
| M-1.3 | `architecture.md` — crate map, dependency rule, the seams | Names all eleven crates, the star rule with its two exceptions, and the `unsafe` budget | done |
| M-1.4 | `roadmap.md` + `backlog.md` — milestones with executable completion conditions | Every milestone's "done" is a command; only M-1 is decomposed | done |
| M-1.5 | `standards/` — remaining code standards: `behavior`, `rust-style`, `error-handling`, `async-concurrency`, `contracts` | ⚠️ Every carried-over threshold is re-derived against oqueue or explicitly marked underived | todo |
| M-1.6 | `scripts/lib.sh` + `check-commit-msg.sh` | Commit subject must name a task this file lists; `require_tool` skips with a named remedy rather than failing | done |
| M-1.7 | `check-drift.sh` + `check-tests-kept.sh` | A threshold made settable fails; a deleted test without `Removes-test:` fails | todo |
| M-1.8 | `check-layering.sh` + `check-sans-io.sh` | Sideways dependency fails; a concrete socket type, real clock read, or object-store call in a library crate fails | todo |
| M-1.9 | `review.sh` + `check-reviewed.sh` — the isolated reviewer and its gate | Review artifact keyed by staged-diff hash; amending one byte after review fails the commit | done |
| M-1.10 | `check-core-contract.sh` | A `pub trait` method-set change without every implementor and an ADR in the same commit fails | todo |
| M-1.11 | `check-unsafe.sh` | `unsafe` outside the three named crates fails; every `SAFETY:` block has a baseline entry | todo |
| M-1.12 | `check-budget.sh` — the pre-commit time budget as an enforced constant | Suite over budget fails; timings written as an artifact so erosion shows as a trend | todo |
| M-1.13 | `.agents/skills/` — `milestone`, `next-task`, `spec`, `tdd`, `review`, `adr`, `research`; `.claude/` adapters and the isolated reviewer subagent | Each parses as the Agent Skills spec; each calls `scripts/`, never a tool built-in; no vendor syntax outside `CLAUDE.md`; adapters contain pointers, not procedures | done |
| M-1.14 | `.pre-commit-config.yaml` (direct-to-main) + push-triggered CI | ⚠️ No gate keyed to `origin/main...`; PR-triggered gates rebased onto the previous commit | todo |
| M-1.15 | `tests/gates/negative.sh` — prove every gate can fail | Each gate invoked against a broken artefact and observed to fail | todo |
| M-1.16 | `scripts/gates/m-1-complete.sh` — the milestone's own completion condition | Asserts every non-negotiable names a passing script, except rule 3; calls `check-milestone-review.sh`, so the milestone cannot be completed while any of its commits has gone unread as a whole | todo |
| M-1.17 | Make the corpus and product docs self-contained before the repo goes public | No reference to any other repository, no absolute local path, no verbatim quotation of an external private source; every practice stated as this project's own standard | done |
| M-1.18 | Public-facing files: `README.md`, `CONTRIBUTING.md`, `SECURITY.md` | README states plainly that no implementation exists; contributing says code is not yet accepted and why; security gives a private reporting route | done |
| M-1.19 | Record the BYOK and FIPS requirements across mission, architecture, roadmap, and the corpus | New milestone M8; `KeyProvider` seam and `oqueue-crypto` crate in the architecture; the AEAD-algorithm-in-region-header constraint recorded against M1 | done |
| M-1.20 | Revise the encryption design for the clarified BYOK volume (~10K topics, not catalog-wide) | Segregation replaces universal per-region sealing; the KMS key-count conclusion corrected and marked as corrected | done |
| M-1.21 | `requirements.md` — functional and non-functional, with stable IDs | Every entry names a verification; every NFR carries a number or is marked UNDERIVED with what blocks it; nothing invented | done |
| M-1.22 | `standards/sdd.md` — the process standard | Defines the requirement→spec→task→commit chain, what a spec must contain, acceptance-criteria rules, definition of done, and what to do when a spec proves wrong | done |
| M-1.23 | `standards/review.md` — the operational review standard | Turns [docs/researches/21](../../researches/21-ai-development-loop.md) §3–5 into a standard: reviewer context isolation, the deterministic/semantic split, fixed-or-argued resolution | todo |
| M-1.24 | Trace every milestone to the requirements it serves | Every roadmap entry names FR/NFR IDs; a gate fails on a milestone that names none | todo |
| M-1.25 | `standards/security.md`, `performance.md`, `build.md`, `portability.md` | Each rule names its gate or is explicitly marked as having none; rationale delegated to the corpus rather than restated | done |
| M-1.26 | `standards/code-structure.md` + `standards/testing.md` + `clippy.toml` | File ≤500 lines with a reasoned allowlist; function ≤50 lines, cognitive complexity ≤20, ≤5 arguments, all via `clippy.toml`; per-crate `README.md` and `AGENTS.md` required; fakes over mocks; the no-flake rules | done |
| M-1.27 | `check-file-size.sh` + `check-readmes.sh` | File-size limit with an allowlist whose entries carry reasons; every crate has both documents, and the README's stated dependencies match `Cargo.toml` | todo |
| M-1.28 | `scripts/profile.sh` + `scripts/bench.sh` | Every profiling mode is one command: instructions, flamegraph, heap, massif, cache, allocation counts. **Never gated** — available on demand | todo |
| M-1.29 | `check-hot-path-bench.sh` | Every hot path named in `performance.md` rule 18 has a benchmark, so the list cannot silently rot | todo |
| M-1.30 | `check-portability.sh` — tool portability of the agent system | `AGENTS.md` and every `SKILL.md` parse without vendor syntax; every skill has `name` and `description`; every `.claude/commands/*.md` is a pointer rather than a procedure | todo |
| M-1.31 | `contract-change` skill | The atomic `oqueue-core` trait change: trait, every fake, every implementation, call sites, and the ADR in one commit | todo |
| M-1.32 | `standards/git.md` — commit atomicity and structure | States why atomicity matters (bisect is the substitute for a reviewer), the subject and body rules, amend-before-push / follow-up-after, and the no-branching workflow | done |
| M-1.33 | Frontmatter on standards and product docs + `scripts/build-index.sh` | Every standard and product doc carries a `description` saying *when to read it*, so layer-1 disclosure works for them as it does for skills; generated index regions rebuild from frontmatter and `--check` fails a stale one | done |
| M-1.34 | `applies_to:` frontmatter + `scripts/which-standards.sh` — route a change to the standards it is judged against | Every standard declares the paths it claims, and one without `applies_to` fails; a staged diff resolves to standards without anyone choosing | done |
| M-1.35 | `check-commit-msg.sh` dies silently on an all-comments message | `grep -v '^#' \| head -1` under `pipefail` exits 1 with no output; the gate must name itself and the reason | done |
| M-1.36 | Rename the `goal` skill to `milestone` | No skill, adapter, or index entry is named `goal`; every reference resolves and `build-index.sh --check` passes | done |
| M-1.37 | The outer loop: `milestone-review` skill + `milestone-review.sh` + `check-milestone-review.sh` | Every commit in a milestone is covered by a review artifact naming the commits it read; a blocking or major finding must name a backlog task that exists, or be argued; an uncovered commit fails the gate | done |
| M-1.39 | `known_task_ids` reads the backlog from the working tree | Every other input to `check-reviewed.sh` and `check-milestone-review.sh` is read from the index; an unstaged backlog row satisfies a gate locally and fails the same gate on CI. Shared by three gates, so it is not M-1.37's to change | todo |
| M-1.38 | `check-reviewed.sh` matches a task id as a regex | `grep -qx "$task_id"` against the backlog's ids: an artifact whose `task_id` is `.*` matches every row. `grep -qxF`. Found by M-1.37's review at the sibling site | todo |
| M-1.40 | The plan layer: `milestones/` + the plan-vs-backlog distinction | `sdd.md` states the difference between a *plan* (forward-looking, expected to be re-derived) and the *backlog* (authoritative, current milestone only); `roadmap.md` carries every milestone to v1 with a kind, the requirements it serves, its dependencies, and an execution order that is not numeric order; `milestones/README.md` says how a plan is consumed | done |
| M-1.41 | Milestone plans: M0, M1, M2, M3, M10 | The build-out sequence — workspace, object store, protocol, coordinator, deterministic simulation. Each names its goal, kind, requirements, the ADRs that must be written before its code, ≤20 provisional tasks, and a completion condition that is a command | done |
| M-1.42 | Milestone plans: M9, M4, M11, M5, M6 | Security, consumer groups, idempotence, compaction, recovery. Same shape as M-1.41 | done |
| M-1.43 | Milestone plans: M7, M8, M12, M13, M14, M15 | Scale, encryption, admin, release engineering, performance validation, hardening. Same shape as M-1.41 | done |

### Notes on specific tasks

**M-1.5** carries the hazard the hybrid decision named: a ported rule justified
by the other project's wire protocol, or a threshold derived from a test suite
that does not exist here, is **stale but authoritative** — worse than absent.

**M-1.9** is new to oqueue and has no precedent to port. The mechanism is in
[docs/researches/21](../../researches/21-ai-development-loop.md) §5. The
reviewer must not receive the author's reasoning; separation of invocation is
not separation of information.

It converts the review from a claim into an artifact, which is the one thing
non-negotiable 3 cannot do for test claims. The verdict lives at
`target/review/<sha256-of-staged-diff>.json` and the gate recomputes the hash
rather than reading a filename, so amending a byte after review invalidates it.

⚠️ **What the gate does not prove: that the reviewer was not the author.**
Nothing in the artifact carries provenance, and a determined author can write
the JSON. What it buys is that the verdict is *about these exact bytes*, which
is the property that was missing; isolation is bought by the harness
(`.claude/agents/reviewer.md`), not by the script. Saying so plainly is the same
discipline as rule 3.

⚠️ **`review.sh` deliberately does not spawn the reviewer.** No script can start
an agent in a way that works across tools, and hard-coding one vendor's CLI
would put a procedure in the enforcement path — the fork the skills exist to
avoid. The script owns the hash, the packet, the schema, and the artifact; the
agent owns the judgement.

The gates the packet reports as already passed are **discovered** — every
`scripts/check-*.sh` minus a short exclusion list carrying a reason per entry —
so a gate added by a later task appears without anyone remembering to add it.

Nine negative cases ran before this landed: no artifact; a verdict claiming a
different diff; a finding with no failure scenario; blocking findings with a
`pass` verdict; an invalid severity; prose instead of JSON; **one byte amended
after review**, which is the acceptance criterion; an artifact hand-edited and
filed under the wrong hash; and a blocking finding resolved by arguing it in the
baseline. All nine behaved.

⚠️ They also found a defect no reviewer would have: `python3 …; rc=$?` under
`set -e` kills the script at the failing command, so the `fail` line never
prints and the exit code is python's rather than the `1` every gate here
promises. `build-index.sh` had the identical bug and **survived its own negative
test**, because the `PROBLEM` lines still printed and only the summary was
missing — a gate failing for the right reason with the wrong message and the
wrong code. Corrected in a follow-up commit naming M-1.33, not by amending it,
because that commit is pushed.

⚠️ **A fresh clone is not yet enforced.** `.git/hooks` is not tracked, so the
wiring that makes "amending one byte fails the commit" true lives on one
machine. The script is correct and the hook is installed here; **M-1.14** is
what makes a clone inherit it, and until then a new checkout gets the gate only
by installing the hook by hand. M-1.6 left the same hole for
`check-commit-msg.sh`.

Review found eight things in one round, and two of them were the kind no test
would have: the packet ran the deterministic gates **against the working tree
while hashing the index**, so staging a broken change and restoring the file
made it assert a gate passed for bytes that were not being committed — with the
reviewer explicitly told not to re-check them. And `sha256sum` was called
unguarded, which on macOS (a first-class platform under portability.md rule 2)
meant `command not found`, exit 127, or worse: `context` printed a one-line
packet and **exited 0**, so a wrapper checking `$?` would proceed to review
nothing. Gates now run against a materialised copy of the staged tree, and
`sha256_stdin` falls back to `shasum`.

Three of the rest were claims that were not true of the code — the same class as
M-1.34's false comment. The baseline's header told the reader that
`review.sh record` prints the finding id; it did not, so arguing a finding meant
opening the JSON by hand. `record` now prints them. The baseline also accepted
an id with no reason at all, suppressing a blocking finding while recording
nothing about why. And a `major` finding could not be recorded as
`changes-requested`, so a reviewer who found two genuine major defects had to
file them as a `pass`, after which they lived only in a gitignored file.

⚠️ A second round found the sharpest defect of the milestone so far, and it was
an **incentive**, not a bug. The argued-findings baseline was read from the
working tree, so appending a line *without staging it* suppressed a blocking
finding: the staged diff was unchanged, the verdict stayed valid, the gate
passed — and the commit recorded the pristine template, so the milestone-
boundary reader the file's own header demands would see an empty argued-list.
Staging the entry properly moved the hash and invalidated the review, so **the
honest path was punished and the trace-free one rewarded.** The baseline is now
read from the index. Arguing a finding is part of the commit, which forces one
more review round. ⚠️ **Superseded below:** that round does not reliably
terminate, because the id is derived from the reviewer's prose.

The same round found three more holes of the shape this milestone keeps
producing — a gate that reports success while enforcing nothing. The
materialised staged tree sat under `target/`, hence still inside the work tree,
so `git rev-parse` there resolved to the real `.git`: a gate written as
`git ls-files … | xargs grep` would iterate zero files, exit 0, and be reported
as passed. ⚠️ **Superseded below:** the first fix moved it out of the
repository, which broke a different standard. `which-standards.sh` failing killed the packet mid-write with
its explanation sent to `/dev/null`, leaving plausible-looking Markdown with no
diff in it. And `check-reviewed.sh` read the verdict, printed it, and never
acted on it, so `changes-requested` passed with an `ok`.

A third round found that the *fix* for the materialised tree had traded one
standard for another: moving it to the system temp directory satisfied the
git-discovery problem and broke build.md rule 19, which keeps scratch out of a
RAM-backed tmpfs — and left a full copy of the index there on an interrupted
run with nothing to reap it. Both constraints are kept now: the tree is under
`target/tmp` with a deliberately invalid `.git` planted at its root. Measured:
without it a `git ls-files` gate reports **0 files and exit 0**; with it, 128.

⚠️ It also found the termination argument for arguing a finding was **unsound**,
and that one is recorded rather than fixed. The id is `sha256(file + summary)`,
so it survives unrelated edits — but the extra round that arguing forces is a
different invocation looking at a different diff, and a reviewer wording the
same defect differently produces a different id, leaving the staged argument
dead. Not a livelock, since fixing is always available; the cost is that the
baseline can accumulate several ids for one defect, which blunts the one signal
it exists to carry. If that starts happening, the id needs to derive from
something stabler than prose.

⚠️ A fourth round found the **third instance of the `set -e` bug class in this
milestone**, this time introduced by the refactor that fixed the second round's
findings. Folding the two baseline loops into one function produced
`argued "$id"; a=$?` — a bare call in an untested context — so the moment a
blocking finding was *not* argued, the shell died before the `fail`, the
remedy, and the summary. The gate refused the commit with **no output at all**,
and with exit 2 for an unusable id rather than the 1 every gate here promises.
The failure path of the check was the one path that could not report, and it
broke lib.sh's contract that a run reports every violation rather than only the
first.

⚠️ **The tell is a bare call whose non-zero return is meaningful**, and it is
worth naming because the milestone has now produced it three times in three
different scripts. It is also a lesson about how the refactor was tested: the
new paths were exercised and the path that used to work was not.

⚠️ Naming the tell was not enough: a fifth round found **two more instances in
the same diff that named it**. `ws_err="$(mktemp …)"` sat one line above the
guard written to catch precisely this, so an unusable `TMPDIR` killed `context`
and emitted the truncated packet all over again — and it contradicted this
file's own argument for keeping scratch out of the system temp directory.
`known="$(known_task_ids)"` was the same shape through a pipeline: a backlog
whose table had no rows returned 1 and killed the gate with no output, while a
*missing* backlog was handled gracefully — the two empty states behaved
oppositely, and the graceful one was the anticipated one.

So the fix went into `lib.sh` rather than the call site, and the whole script
set was swept for the pattern rather than patching the two that were reported.
⚠️ One instance is knowingly left: `check-commit-msg.sh`'s
`grep -v '^#' "$MSG_FILE" | head -1` dies the same way when a commit message is
entirely comments. It belongs to M-1.6, so it is tracked as **M-1.35** and
corrected in its own commit rather than folded in here — prose inside a done
task's retrospective is not a record the `next-task` skill can see, and
AGENTS.md is explicit that out-of-scope work becomes a backlog task.

⚠️ Later rounds found the pattern twice more, in places the sweep had not
reached because they did not look like calls: `|| true` on `known_task_ids`
fixed the death but produced a **silent skip** — the membership check stopped
running and the gate still printed `ok`, which lib.sh's own header names as the
failure to avoid — and `shift 2` in the option loop killed the script with zero
bytes on both streams whenever a flag arrived without its value, which is what
`--task $UNSET_VAR` interpolates to.

Counting the two follow-up commits, this class accounts for **six of the
defects found in M-1.9**. Every one of them made a gate report something other
than what it checked, and none of them was visible from reading the diff — they
were all found by running the failure path.

**M-1.12** has no constant yet. Two minutes is a common figure for a pre-commit
budget, but it is usually written as a comment and never measured, and oqueue's
dependency graph is heavy. Derive it from measurement once M0's workspace
exists; until then the task is blocked rather than guessed.

**M-1.14** ⚠️ the CI adaptation that is easy to miss: with no pull requests, any
gate triggered by one silently never runs.

**M-1.33** closed a hole in progressive disclosure: skills had layer-1
descriptions and standards did not, so an agent could see *when* to load a skill
but had to open a whole standard or guess from its filename.

⚠️ It also produced a lesson about generated indexes. The first version replaced
the curated 39-entry tag index with 205 alphabetical raw tags — mechanically
correct and strictly worse, because the curated one is organised by *question*,
which is how people look things up. The split now is: **mechanical things are
generated (counts, tables, coverage), authored things stay authored**, and the
generated tag list is filtered to tags that group three or more documents.

**M-1.37** builds the outer loop. Doc 21 §8 designed it and then recorded why
it does not happen: it is "initiated by inspiration rather than by schedule" —
someone notices something and writes a milestone about it. ⚠️ **A phase that
runs when somebody thinks of it is not a phase**, which is the same argument
this project makes about every rule with no gate.

The inner loop reads one delta against one task, so it cannot see its own
drift: a convention quietly abandoned, two commits that each passed review and
contradict each other, or a spec that should not have been written that way all
pass it, because every individual step was faithful.

The artifact follows doc 21 §5's pattern but keys on **a set of commits** rather
than a diff, which is what makes reviews incremental and therefore
checkpointable: each names the commits it read, the gate unions them, and a new
commit is simply uncovered until some review covers it.

⚠️ **The step with teeth is `task_id`.** Every blocking or major finding must
name a backlog row that exists, or be argued. A cross-cutting finding recorded
only in `target/review/` is one nobody will act on — the directory is gitignored
and `cargo clean` reclaims it. That is what "findings become backlog tasks"
means operationally, and it is checkable.

⚠️ **What has no gate: the cadence.** Nothing bounds how many commits may
accumulate before a review, and no honest constant is available — it depends on
how much a reviewer can hold at once. Reviewing thirty commits in one pass
satisfies this gate and wastes it. ⚠️ **M-1 already has 24 commits and none has
been read as a whole**, which is the backlog of outer-loop work this task
creates rather than discharges.

Writing it produced one defect of the family M-1.9 catalogued, in the argued
escape: `grep -q … || argued=$?` only assigns when grep *fails*, so the variable
never became 0 and a finding argued in the baseline was still reported
unresolved. Exit-status plumbing written so the interesting branch cannot run.

Review returned `changes-requested` on three majors, and the sharpest is one
the *inner* loop produced.

⚠️ **The packet reported `check-milestone-review.sh: passed` for a milestone
whose every commit was unread.** `review.sh` discovers `scripts/check-*.sh`
automatically, so the new gate was picked up on the commit that added it — and
ran inside the materialised staged tree, whose `.git` M-1.9 deliberately plants
as an invalid file. No history, no milestone commits, and the gate's own
`skip … (no commits yet)` exit 0. Two independent defects composing into a
false pass: a **fail-open** in the gate, and the wrong **scope**. Both fixed —
the gate now hard-`fail`s when it cannot read history, and the gate is in
`GATES_EXCLUDED` because it is a *milestone completion* gate and a commit is
not required to have had its whole milestone re-read. ⚠️ Note which way the
composition ran: M-1.9's `.git` plant is what makes a git-using gate fail
closed, and it turned a gate that could not run into one reporting success,
because the gate answered "cannot run" with `skip`. **A gate that cannot run
must not report success**, and `skip` is a claim about applicability, not about
availability.

⚠️ **The documented escape was unreachable.** The skill and the packet both
said a blocking or major finding may be argued in `baselines/review.txt`
instead of becoming a task — but `record` refused to store a finding with no
`task_id`, and the `id` a baseline entry needs is assigned *by* `record`. So
the only path to the escape ran through the check that forbade it. It bit
hardest on `kind: spec-wrong` at blocking severity, precisely where the skill
says the outcome is a decision and not a task. Fixed by moving the demand from
`record` to the gate: record validates shape and prints the id, the gate
demands a resolution — the split `check-reviewed.sh` already used. ⚠️ The
general shape is worth keeping: **a documented escape nobody has walked is a
claim, not a feature**, the same argument this file makes about a gate whose
failure path nobody has run.

Two more, both about a check that answers with something other than what it
checked. `git show … | grep -q` on the baseline is a pipe whose left side dies
of SIGPIPE when grep exits at the first match — measured at rc=141 on a
200,000-line baseline, reporting a genuinely argued finding as unargued.
`check-reviewed.sh` escapes it only because its read carries `|| true`. And
`grep -qx "$tid"` treats a task id as a regular expression, so a hand-written
artifact claiming `task_id: ".*"` satisfies the one rule this gate calls the
one with teeth. `-F` closes it here; **the identical site in
`check-reviewed.sh` is M-1.38**, tracked rather than fixed in passing, because
it belongs to M-1.9.

A second round found two more blocking, and both are the same mistake in
different clothes: **a check written against the example in front of it.**

⚠️ **The milestone derivation matched exactly one milestone in the project's
life.** `^## (M-[0-9]+):` requires the hyphen, and only *this* milestone has
one — the roadmap's next headings are `## M0 —`, `## M1 —`, `## M8 —`. From M0
onward the gate would print "no milestone is decomposed" and exit 0: a false
statement, and a pass for a milestone whose every commit was unread. Once
M-1.16 wires it into the completion condition, every remaining milestone in the
roadmap could be declared complete with no cross-cutting review at all. ⚠️ It
also fails **quietly and late** — nothing would have gone wrong until the first
commit after M-1, months from the code that caused it. `known_task_ids` and
`check-commit-msg.sh` both already use `M-?[0-9]+`; this was the only place
that assumed otherwise, which is the tell: a project-wide id shape re-derived
locally, from the one example the author could see.

⚠️ **Rule 2 wedged the gate shut on `git commit --amend`.** git.md rule 21
endorses amending freely before a push, and this project never pushes unasked,
so every commit is amendable. A message typo fixed after a review left an
artifact naming a SHA that history no longer reaches — and rule 2 called that
"a review claims a commit the milestone does not contain", forever. The argued
baseline resolves *findings*, not coverage, so the only escape was hand-deleting
a file in gitignored `target/review/` that no message named, which also
discarded the coverage of every other commit in the milestone: a typo fix
costing a full re-review. The gate also printed its `FAIL` and then `ok … all
commits covered`, two contradictory lines with the reassuring one last. Now a
commit HEAD no longer reaches is a superseded artifact (`warn`), a commit that
is reachable but belongs elsewhere is still a failure, and the failure path
`finish`es before it can contradict itself. ⚠️ The first attempt at this used
`git cat-file -e`, which succeeds for an amended-away commit because the object
survives in the reflog — **reachability, not existence, is the question**.

And the packet's `## What changed across them` used `${first}~1`, which the
**root commit does not have**. M-1.0 is this repository's root *and* its first
unreviewed commit, so the first intended use of this tool hit it: `2>/dev/null`
swallowed the fatal and the fallback showed the root commit's stat alone — 73
files of research corpus, none of the work under review — under a heading
claiming it was the shape of all 24. The empty tree is the correct base.

⚠️ Three of these five were **fail-open in a gate whose subject is fail-open**,
which is worth stating plainly rather than filing away: writing the check does
not confer the property, and the only thing that found them was running them.

A third round found the *same* fail-open a third time, one layer further in.
Fixing the hyphen made `grep -m1 -oE '^## M-?[0-9]+'` match M0 — but line 13 of
this file says **"Completed tasks stay here with their commit reference"**, so a
finished milestone keeps its section and `-m1` returns M-1 forever. From M0
onward both scripts would have kept checking a fully covered M-1 and reported
success while the milestone actually being built went unread. ⚠️ **Twice in a
row the milestone derivation was written against the only example available**,
which is the argument for deriving it from something that cannot be one example:
the current milestone is now the one **HEAD is working in**, read from the most
recent commit subject that names a task. This gate's subject is commits, so
deriving from commits needs no convention, and it flips to M0 exactly when M0's
first commit lands — not before, which is what leaves M-1's completion still
checkable after M-1's last commit.

Two honesty fixes came with it. The `ok` line said "all N commit(s) covered"
when what it counted was commits whose subject *begins* with a task id of this
milestone — so the shapes `check-commit-msg.sh` deliberately exempts, `Revert
"…"` and merges, are never required to be covered. It now says what it counted.
⚠️ **Closing that gap needs a decision about what a revert's coverage means**,
and is not this task. And the unresolved-finding failure named neither the
baseline nor the requirement that the argued line be *staged*, so an operator
taking the documented escape got an unchanged failure and repeated the edit;
`check-reviewed.sh` already had both aids.

A fourth round found the artifact in the wrong place. ⚠️ **Milestone coverage
was stored in gitignored `target/review/`**, so `cargo clean` erased the record
that a milestone had been reviewed — and M-1.16's completion condition calls
this gate while M-1.14 puts gates in CI, which means the condition would have
passed on exactly one machine and been permanently red on every clone. The
per-commit reviewer is right to write there: its verdict is keyed to a staged
diff about to become a commit, and it is consumed where it was made. A
milestone's coverage is the opposite kind of claim — durable, about history,
and something a second agent has to be able to check. It now lives in tracked
`reviews/`, read **from the index** for the reason `baselines/review.txt` is:
a file that counts while unstaged rewards the path that leaves no trace.

⚠️ That move exposed a regress the on-disk version had hidden: **committing a
milestone's verdict creates a commit that names a task in that milestone**, so
the milestone goes from fully covered to one-uncovered the instant it is
recorded, and covering that commit needs another review in another commit.
Measured, then fixed by excluding commits whose every changed path is under
`reviews/` — narrowly, so a commit carrying real work alongside a review file is
still subject matter.

The rest of the round was duplication and honesty. `current_milestone` and the
commit-enumeration grep were **byte-identical copies** in the driver and the
gate, and the two rounds above had each had to land in both; they are one
definition in `lib.sh` now, because if the gate ever enumerates a commit the
driver did not show the reviewer, `record` refuses the SHA the gate demands and
the gate cannot be passed at all. `record` called an abbreviated SHA "not a
commit in M-1" — false, and pointing at the wrong problem, while this tool's own
`coverage` output prints exactly that abbreviated form; an unambiguous prefix is
resolved now. And an absent roadmap section rendered as an empty heading, which
reads as "this milestone had no goals"; it says so instead, on both streams.

⚠️ **A fifth round found the SIGPIPE bug again, in the code written to fix the
regress above, in the same commit that documents fixing it elsewhere.**
`printf '%s\n' "$paths" | grep -qv '^reviews/'`: `grep -qv` exits at the first
non-matching line, `printf` dies of EPIPE, `pipefail` calls the pipeline failed,
and the leading `!` inverts that to true — so a commit whose changed-path list
overflows the pipe buffer was classified as review bookkeeping and **dropped
from the milestone entirely**, with the gate then reporting full coverage
without it. Measured: correct at 1000 changed files, wrong at 1500 (~57 KB);
with a 204 KB path list the commit vanished and the fixed version keeps it.
⚠️ It is exactly the **largest** commits that disappear — generated protocol
code, an imported corpus, a mass rename — and the same shared helper makes
`record` refuse to store a verdict naming one, so a reviewer who did read it
could not record having read it. A here-string fixes it. That this class has now
appeared **nine times across six scripts**, including inside its own fix and its
own documentation, says it is not carelessness but a property of the idiom: `set
-o pipefail` makes every `cmd | grep -q` a place where a *successful early exit*
is reported as failure. It belongs in the shell rules `portability.md` will grow.

Two smaller ones. A `task_id` the backlog lists but marks **done** discharged a
blocking finding — plausible to write, since the row that introduced the code is
often the one a finding is about, and fatal in effect: `next-task` reads `todo`,
so the finding is parked where nothing will look again, which is the exact state
rule 3 exists to prevent. The row must now still be open. And ⚠️ `known_task_ids`
reads the backlog from the **working tree** while artifacts and the argued
baseline are read from the index, so an unstaged row satisfies the gate locally
and fails it on CI — the same pass-here/red-there asymmetry this task fixed for
the artifact location. It is shared by three gates and is **M-1.39**, not
M-1.37's to change.

⚠️ **A sixth round found that round five's own fix punished the honest path.**
Requiring a cited backlog row to still be `todo` was checked by the gate on
every run — so the moment the finding's task was implemented and its row ticked
to `done`, the gate went red and stayed red, because the artifact is cumulative
and never re-read. The three ways out were re-opening a finished row (a lie),
arguing a finding the author had actually acted on, and re-recording the verdict
without the finding — **deleting a finding to make a check pass**, non-negotiable
2 exactly. The rule itself is right; its place was wrong. Openness is an
authoring-time question, enforced by `record` while the author is still there to
pick another row, and the gate is back to what the acceptance criterion says:
the task exists. ⚠️ Third time in this task that a check written to close a hole
made the correct behaviour the expensive one — worth naming as a review question
in its own right: *what does this gate cost someone who is doing the right
thing?*

The anti-regress carve-out was also too wide. Excluding every commit whose paths
are all under `reviews/` dropped a commit that only rewrote `reviews/README.md`
— real work, silently unreviewed, gate green. Only the verdict files cause the
regress, so it matches the artifact name now.

And ⚠️ **nothing said who runs a milestone review.** The inner loop is explicit
that a change is reviewed by an agent that did not write it, and the outer loop
had no equivalent — so as wired, an agent could drive a milestone to its last
commit and then review its own 24 commits, which is the one reader guaranteed
not to see their drift. Now stated in the packet, in both skills, and given an
adapter in `.claude/agents/milestone-reviewer.md` so it is a thing to do rather
than a thing to remember. ⚠️ It is still unenforceable — the artifact records a
`reviewer` string nothing can verify — and that is said where the other
unenforceable claims are said, rather than implied away.

⚠️ **A seventh round found that round six's edit had deleted the baseline read.**
Removing the gate-side openness check took `baseline_text="$(git show …)"` with
it — adjacent lines, one edit — leaving the argue branch referencing a variable
assigned nowhere. Under `set -u` that aborts the condition, `argued` stays 1, and
**the entire argue escape was dead code**: an operator who did exactly what the
gate's own remedy line told them to got the identical failure back, with a raw
`baseline_text: unbound variable` on stderr naming no gate. It failed hardest for
`kind: spec-wrong` at blocking severity — the one case the skill says must *not*
become a backlog row — so a milestone carrying one could never be completed.

⚠️ Two lessons, and the second is the one worth keeping. First: a removal is a
change, and this one was never run. Second and larger — **six of the seven
rounds found a defect on a path that had been described but never walked.** The
escape was documented in the skill, in the packet, and in the gate's remedy
note, and the documentation was written before anyone tried it. This is the
concrete form of non-negotiable 3 for gates rather than tests: *a failure path
nobody has walked is a claim, not a feature*, and it belongs in whatever
`sdd.md` grows for acceptance criteria. The fix here was one line; finding it
took walking the two documented steps once.

⚠️ **An eighth round, run after actually walking that same escape end to end,
returned `pass`** with one minor: `known_task_ids | grep -qxF` was the one site
in this file the earlier sweep for this exact SIGPIPE class missed — reachable
only past roughly 150x today's backlog size, so not urgent, but the same class
this task spent seven rounds eliminating and one line to close. Fixed on sight
rather than filed, since the file was already open.

**M-1.36** renames the `goal` skill because the name is one a tool is likely to
claim. ⚠️ The collision would be **silent**: a slash command resolves to one
procedure, and nothing announces that the other exists. `milestone` says what
the skill drives and is far less likely to be taken. Nothing about the loop
changed — only the name, the directory, the adapter, and the index entries.

⚠️ The general rule this suggests, and which no gate holds: **a skill's name is
part of its interface with the tool, not only with the reader.** `spec`, `tdd`,
`adr`, `research`, and `review` are all short enough to be claimed the same way.
M-1.30's `check-portability.sh` is the natural home for a check, if one is worth
having.

**M-1.35** is a defect M-1.9's work found in an already-`done` gate, and its
review is a warning about fixing diagnostics. The first fix made the gate speak
where it had been silent — and said something **false** in two cases: it ran the
comment-stripping branch in `HEAD` mode, where there is no message file, and it
stripped comments but **not** leading blank lines, which git also removes — so
on a `COMMIT_EDITMSG` whose first line is empty it returned that blank line as
the subject. Both produced "the commit message has no subject" for a commit
whose subject was plainly there. ⚠️ A wrong
diagnosis is worse than the silence it replaced, because it sends the author to
fix something that is not broken.

⚠️ The second fix then introduced a **regression the original did not have**.
`git commit` with an editor uses `cleanup=strip`, which removes comment lines;
`-m` and `-F` use `cleanup=whitespace`, which keeps them. So for
`git commit -m '#42 note' -m 'M-1.35: …'` the subject that lands is `#42 note`,
and a script that skips comment lines reads the *body*, reports
`ok … names M-1.35`, and admits a commit naming no task — the one thing this
gate exists to refuse. No script can know which cleanup mode git will apply, so
the ambiguity is now the failure: the subject is the first non-blank line, and a
comment there is refused rather than searched past. That also stops the
`commit.verbose` case reporting `got: diff --git a/f b/f`, a subject nobody
wrote.

**M-1.34** exists because doc 21 §4 says the reviewer receives "the relevant
standards", and until now that phrase had no referent. Handing a reviewer all
eight dilutes its attention across seven that do not apply — the same failure
§4 is trying to avoid by withholding the author's reasoning.

The mapping lives in each standard's own front matter rather than in a table
inside the script, for the reason M-1.33 established: two places holding one
fact is a promise to keep them in sync forever. ⚠️ Most patterns name Rust that
does not exist yet, so they are **unexercised until M0** — deliberate, but it
means the mapping is asserted rather than demonstrated for every crate-scoped
entry.

⚠️ The first independent review of this task found a defect the author's own
testing had not: `git diff --cached --name-only --diff-filter=ACMR` silently
drops deletions, so a **deletion-only change reported "nothing staged" and
routed to no standards at all** — including `git.md`, whose `*` is meant to be
unconditional. Deleting a test or a standard is exactly the commit class
non-negotiable 2 exists for, and it was the one class the router could not see.
The review also found `applies_to` claimed too few crates in two standards:
`security.md` did not claim `oqueue-buf` or `oqueue-checksum`, which are two of
the three crates where `unsafe` is allowed, and `performance.md` did not claim
`oqueue-broker`, `oqueue-store`, or `oqueue-compact`, which hold three rows of
its own hot-path table.

The second round found two more, both introduced by the fixes: the front-matter
parser **failed open** on a quoting style it did not handle — yielding a shorter
standards list with no diagnostic, which is worse than the loud failure it
already had for a missing key — and a comment justifying the rewrite made a
claim about `build-index.sh` that is **not true**. ⚠️ That leaves a real wart:
`which-standards.sh` accepts a block sequence and `build-index.sh` does not, so
a `tags:` written that way fails the index gate with a message about a missing
family tag. Reconciling the two parsers belongs to whichever task next touches
`build-index.sh`; it is recorded here rather than fixed, because doing it in
this commit would have widened it.

**M-1.32** names the rule most likely to be broken without noticing: ⚠️ **never
mix a refactor with a behaviour change**. It is the most common way an atomic
commit stops being one, it makes the diff unreviewable, and a bisect landing on
that commit cannot say which half broke. No script catches it, so it is review's
job.

It also records what happened earlier in this milestone: a pushed commit
carrying wrong counts in its message was left standing and corrected by a
follow-up rather than amended, because `main` is public and rewriting it breaks
every clone.

**M-1.6** is the first gate, and writing its negative cases immediately found
two defects in it. The subject-quality rule originally required ten characters,
and the first thing it rejected was `M-1.2: mission` — an accurate one-word
subject already in history. ⚠️ **The rule was wrong, not the commit**: length is
a proxy for meaning and a poor one, since it accepts `fix the thing` and rejects
`mission`. It is now a denylist of words that carry no information. The second
defect was a `warn` line whose backticks inside double quotes made it *execute*
`git log --oneline` rather than print it.

Both are the argument for M-1.15 in miniature: **a gate nobody has watched fail
is a gate nobody has tested.**

**M-1.13** implements progressive disclosure in four layers, because the
alternative — loading seven standards and 110,000 words of research into every
session — is impossible and would be useless if it were not. ⚠️ The load-bearing
piece is a skill's `description`: it is the only thing an agent sees before
deciding to load the body, so it must say *when to use this* rather than *what
this is*.

`.claude/agents/reviewer.md` is where doc 21 §4 stops being a design and starts
being a mechanism: a subagent whose prompt explicitly denies it the author's
reasoning, with a restricted tool set so it reads rather than writes.

**M-1.26** makes the structural limits real rather than aspirational.
`clippy::too_many_lines`, `cognitive_complexity`, and `too_many_arguments` are
all in the `pedantic` group, which is already denied workspace-wide, so the
thresholds in `clippy.toml` are enforced the moment a crate exists. ⚠️ The
500-line file limit needs its own script and an allowlist, because generated
protocol tables and exhaustive `match` arms over wire types are legitimately
large — M-1.27.

**M-1.28** is deliberately **not** a gate. Profiling is slow, needs a quiet
machine, and produces output requiring judgement; gating on it would either
stall the loop or produce alerts nobody trusts. The requirement is that it be
runnable at any moment without ceremony.

**M-1.25** closed a gap in the standards plan. M-1.5's list was adapted from a
service of a different shape — one that ships a single architecture, carries no
benchmarking methodology, and holds no customer key material. Four of the
categories oqueue actually needs had no standard planned at all. The remaining
M-1.5 set is now code standards only.

**M-1.21** deliberately leaves several NFRs **UNDERIVED** rather than choosing
plausible numbers. NFR-13 (aggregate throughput) blocks NFR-31 and gates
architectural decisions; NFR-55 and NFR-56 are constants that cannot be picked
before there is a workspace to measure. ⚠️ An invented number is
indistinguishable from a measured one a month later, which is why the standard
forbids it outright.

**M-1.20** corrected a conclusion rather than extending one. The first
encryption draft assumed BYOK might apply catalog-wide and derived a design from
AWS KMS's 100,000-key ceiling. At ~10,000 BYOK topics that ceiling is not
binding, and the resolution changes from *seal every region of every object* to
*segregate BYOK data into its own objects*. Both the corpus and the decision log
mark this as a correction, because a reader arriving at the earlier reasoning
would otherwise inherit a constraint that does not apply.

**M-1.17** was executed out of ID order, immediately before the first push. Task
IDs are stable, so the table is ordered by ID rather than by execution. The
substance of every borrowed practice survives — it is now stated as this
project's standard under a `[Practice]` marker, which keeps the honest
distinction from `[Assessment]` (our own reasoning) and `[Documented]`
(externally cited).
