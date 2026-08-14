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
milestones stay as roadmap entries until their turn.

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
| M-1.16 | `scripts/gates/m-1-complete.sh` — the milestone's own completion condition | Asserts every non-negotiable names a passing script, except rule 3 | todo |
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
