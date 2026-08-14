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
| M-1.5 | `standards/` — remaining code standards: `behavior`, `rust-style`, `error-handling`, `async-concurrency`, `contracts` | ⚠️ Every carried-over threshold is re-derived against oqueue or explicitly marked underived | done |
| M-1.6 | `scripts/lib.sh` + `check-commit-msg.sh` | Commit subject must name a task this file lists; `require_tool` skips with a named remedy rather than failing | done |
| M-1.7 | `check-drift.sh` + `check-tests-kept.sh` | A threshold made settable fails; a deleted test without `Removes-test:` fails | done |
| M-1.8 | `check-layering.sh` + `check-sans-io.sh` | Sideways dependency fails; a concrete socket type, real clock read, or object-store call in a library crate fails | done |
| M-1.9 | `review.sh` + `check-reviewed.sh` — the isolated reviewer and its gate | Review artifact keyed by staged-diff hash; amending one byte after review fails the commit | done |
| M-1.10 | `check-core-contract.sh` | A `pub trait` method-set change without every implementor and an ADR in the same commit fails | done |
| M-1.11 | `check-unsafe.sh` | `unsafe` outside the three named crates fails; every `SAFETY:` block has a baseline entry | done |
| M-1.12 | `check-budget.sh` — the pre-commit time budget as an enforced constant | Suite over budget fails; timings written as an artifact so erosion shows as a trend | todo |
| M-1.13 | `.agents/skills/` — `milestone`, `next-task`, `spec`, `tdd`, `review`, `adr`, `research`; `.claude/` adapters and the isolated reviewer subagent | Each parses as the Agent Skills spec; each calls `scripts/`, never a tool built-in; no vendor syntax outside `CLAUDE.md`; adapters contain pointers, not procedures | done |
| M-1.14 | `.pre-commit-config.yaml` (direct-to-main) + push-triggered CI | ⚠️ No gate keyed to `origin/main...`; PR-triggered gates rebased onto the previous commit | done |
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
| M-1.44 | `portability.md` — a shell-scripting section documenting the `set -o pipefail` SIGPIPE-under-`grep -q`/`head`/early-exit idiom | Names the idiom that recurred nine times across six scripts in this milestone (`build-index.sh`, `check-reviewed.sh`, `check-layering.sh`, `check-core-contract.sh`, `milestone-review.sh`, `check-milestone-review.sh`), gives the `\|\| rc=$?` / here-string remedies as rules, and is cited by name from at least one gate script comment rather than left as tribal knowledge in commit messages and backlog prose | todo |
| M-1.45 | Backport the crash-safety wrapper (`try`/`except Exception` around the Python body, distinct exit code 3 for an uncaught crash) from `build-index.sh`/`check-unsafe.sh` to `check-layering.sh` and `check-core-contract.sh` | Both scripts were written after `fadd095` (M-1.33's follow-up) established the wrapper and after `check-unsafe.sh` reused it, but neither adopted it; both currently avoid a false *pass* on crash only because no `print` executes before their risky `git`/file-read calls — an undocumented, unenforced invariant one added diagnostic print away from silently reintroducing the exact false-pass class the wrapper exists to prevent. Acceptance: both scripts exit a distinct non-1/0 code on an uncaught exception, verified against a contrived non-UTF-8 input the way `check-unsafe.sh`'s own suite already does | todo |

### Notes on specific tasks

**M-1.5** carries the hazard the hybrid decision named: a ported rule justified
by the other project's wire protocol, or a threshold derived from a test suite
that does not exist here, is **stale but authoritative** — worse than absent.

`behavior.md`'s scope was not specified anywhere before this task and had to
be decided rather than found: it is the *external* contract — what a Kafka
client, an operator, or another tenant may rely on staying true — as opposed
to the other four, which govern how the Rust reads and is structured
internally. That split is stated at the top of the file itself so a future
reader does not have to reverse-engineer it from the table of contents.
`code-structure.md` already covered the `unwrap`/`expect` ban and the
`unchecked_*` ban, and `testing.md` already covered where fakes live; the
four new files reference those rules rather than restating them, to avoid
the two places-one-fact hazard M-1.33 already named for generated indexes. Every gate a rule names is one of
M-1.7 through M-1.12, none of which exist yet — consistent with every other
standard in this repository, which routinely names a gate before the script
behind it is written.

**M-1.7** wires the second half of non-negotiable 2 — the first (never
lower a threshold) is `check-drift.sh`, forward-looking and heuristic,
since the repository has almost nothing for it to check yet: it flags a
threshold-shaped identifier read from an environment variable on the same
line, which is deliberately narrower than "any threshold ever weakened,"
a claim no grep gate can make honestly without a real parser. The second
(never delete a test to make a check pass) is `check-tests-kept.sh`, which
diffs `#[test]`-attributed function names between a file's old and new
content and demands a `Removes-test:` trailer for anything that
disappears — it does not and cannot judge whether the stated reason is
true, the same limit `check-commit-msg.sh` already accepts for task IDs.
Both were run against a contrived positive case in a scratch git repository
before being trusted, rather than only read — the same discipline M-1.6's
retrospective names ("a gate nobody has watched fail is a gate nobody has
tested"), ahead of M-1.15 making that a permanent, checked-in negative
suite rather than a one-off manual run.

⚠️ **That manual testing still missed the one check that mattered most: did
`check-drift.sh` pass on the repository it was being added to.** Review
found it did not — its own header comment states the two patterns the gate
looks for side by side, as worked examples, and both matched its own regex,
so the gate would have failed on itself forever from the moment this
commit landed. Fixed by excluding the script's own file from the scan, with
the reason written beside the exclusion rather than left implicit — it
holds no threshold of its own to protect, so nothing is lost, and a
scratch-file test with the same worked-example text under a different
filename confirmed the exclusion is narrow rather than a general carve-out.
Review also found a doc-comment claim in `check-tests-kept.sh` that was
backwards: it said a test moved to another file under the same name is
*not* flagged, when the per-file comparison the code actually performs
flags exactly that case, in the file the test disappeared from — the safer
direction to be wrong in, but the comment described the opposite of the
code next to it.

⚠️ **A second review round found the manual testing had checked the common
shape and no other.** `extract_tests`'s awk cleared its "pending test" flag
on any non-blank line that was not itself a `#[...]` attribute — so a
`#[test]` immediately followed by a doc comment, or by a multi-line
attribute's continuation lines, never reached a `fn` while the flag was
still set, and the test underneath was invisible to both the old and the
new side of the diff. A test deleted in that shape passed with `ok no test
was removed`: the exact acceptance criterion this task exists to satisfy,
silently unmet for two idiomatic Rust shapes. Fixed by letting the pending
flag survive doc comments, plain comments, blank lines, and an unmatched
multi-line attribute's continuation (tracked by an unbalanced `#[` with no
`]` yet) — verified against both shapes in a scratch repository, plus a
regression pass confirming the original cases (single-line attribute,
whole-file deletion, rename via delete-and-add) still hold.

⚠️ **A third review round found the second round's fix was still only the
common shape.** The continuation tracker set `pending` on close without
ever checking whether the attribute it had just spanned actually contained
"test" anywhere — only the *opening* line was checked, and a wrapped,
parameterized attribute like `#[tokio::test(\n  flavor =
"multi_thread"\n)]` puts `test` on that opening line by coincidence of
`tokio::test`'s name, while a differently-named or differently-wrapped
multi-line test attribute would not be recognized at all. Verified directly
with a `#[tokio::test(...)]`-shaped attribute deleted with no trailer: `ok
no test was removed`, silently. Realistic for this specific project, not a
contrived case — async test attributes wrapped across lines by `rustfmt`
are exactly the shape `async-concurrency.md`'s own subject matter produces.
Fixed by checking every line the attribute spans for "test", not only the
first, and setting `pending` when the attribute closes if any of them
matched. Verified against the exact wrapped-`tokio::test` case, a full
regression re-run, and the disclosed nested-`[...]`-in-a-string gap,
reconfirmed real. ⚠️ That reconfirmation needed a second try: the first
attempt at constructing it wrote a single-line `cfg_attr`, which the
single-line branch handles correctly regardless of the nested bracket and
so never exercised the continuation-tracker code path it was meant to
test — a version of the same lesson this task keeps producing, that a test
which looks like it covers a case can silently cover a different one.
Redone with a genuinely multi-line attribute, it reproduced.

A fourth review round, asked to try shapes no prior round had, found one
more: the `fn`-matching regex accepted `pub` and `pub async` but not
`pub(crate)`, `pub(super)`, or `pub(in path)` — an everyday visibility
modifier, not a contrived one, so a `#[test] pub(crate) fn ...` deleted
with no trailer passed silently. The round judged it non-blocking (this
repository has no `.rs` files yet, and a gap of this shape is the same
tier as the already-accepted one), but it was a one-line regex fix rather
than only a documentation addition, so it was fixed rather than deferred:
`pub(\([^)]*\))?` in place of a bare `pub`, verified against `pub(crate)`
and `pub(in crate::tests)` both being caught, plus the full regression set
once more. Both gates are wired into the local `.git/hooks/pre-commit` and
`commit-msg` scripts alongside M-1.6's and M-1.9's, following the same
precedent: local wiring now, `.pre-commit-config.yaml`'s portable version
in M-1.14.

**M-1.8** wires non-negotiable 5 (sans-I/O) and the crate-layering rule
`code-structure.md` rule 1 and `architecture.md` both already cited a gate
for. `check-layering.sh` reads each crate's `Cargo.toml` well enough to
answer one question — which workspace crates does this one depend on at
runtime — without a real TOML parser: it tracks section headers and, inside
`[dependencies]`, treats a bare `name = ...` line as a dependency and a
`[dependencies.name]` header as one too, careful not to mistake that
sub-table's own `path = ...` field for a second dependency named `path`.
`oqueue-broker` and `bin/oqueue` (package `oqueue`) are the two named
composer exceptions; `oqueue-testkit` may never appear in any crate's
`[dependencies]` regardless of composer status. `check-sans-io.sh` greps
every library-crate `.rs` file for three literal-identifier patterns — a
concrete socket type, a real clock read, an object-storage SDK call — each
exempting `oqueue-broker` (the I/O shell) and, for the object-storage
pattern only, `oqueue-store` (the `ObjectStore` seam's implementor).

Both were built against a fake scratch workspace rather than trusted from
reading alone, since the real repository has no crates yet for either gate
to exercise: a five-crate `Cargo.toml` tree (including a genuine
`oqueue-testkit` crate, without which the dev-only check cannot fail —
caught on the first attempt, when an empty scratch workspace made the
testkit-in-runtime-deps case pass for the wrong reason, no `oqueue-testkit`
crate existing at all to be flagged) confirmed every rule in both scripts:
a sideways dependency, `oqueue-testkit` leaking into `[dependencies]`, the
table-header dependency form parsing correctly, a socket type and a real
clock read and an S3 SDK call each flagged in an ordinary library crate,
and `oqueue-broker`/`oqueue-store` correctly exempt from exactly the
patterns their own role requires and nothing more (`oqueue-store` calling
the S3 SDK passed; `oqueue-store` opening a raw `TcpStream` still failed).

⚠️ Review found two real gaps the happy-path testing above did not reach,
each severe enough on its own that neither was left to a "does not catch"
disclosure. `check-layering.sh`'s flow-form parser matched `name = ...` but
not Cargo's dotted-key shorthand `name.workspace = true` — an ordinary,
idiomatic way to centralize dependency versions across a workspace, not an
obscure one — so a genuinely sideways dependency written that way passed
clean, and the same gap would have let `oqueue-testkit` leak into a runtime
`[dependencies]` block undetected. **Blocking**, because it defeats this
task's first acceptance criterion outright for a realistic Cargo idiom, not
a contrived one. Fixed by matching the key up to its first `.` rather than
requiring the whole thing to be a bare identifier, verified against the
exact `oqueue-index.workspace = true` sideways case and an
`oqueue-testkit.workspace = true` runtime leak, both now caught. Second,
`check-sans-io.sh`'s socket pattern used `\b` word-boundary anchors — the
exact portability hazard `check-drift.sh` had already named and worked
around one task earlier, reintroduced here with no note explaining why it
was safe this time (it was not). **Major**, since GNU and BSD grep disagree
about `\b` and portability.md makes macOS a first-class development
platform this gate has to run on. Fixed by replacing it with an explicit
non-identifier-character (or line-start/end) boundary — a different,
equally portable answer to the same problem `check-drift.sh` solves by
dropping any boundary at all and accepting the substring false-positive
risk instead, which the second review round caught this file
misdescribing as identical to `check-sans-io.sh`'s approach when the two
scripts actually take opposite ones. Corrected here rather than left
standing. Verified
against start-of-line, end-of-line, and ordinary matches.

A second review round passed both fixes and found two further minor
issues, both fixed rather than deferred. `check-layering.sh` still has one
disclosed gap left: a dependency renamed via Cargo's `package` key (e.g.
`sideways = { package = "oqueue-other", path = "..." }`) records the local
alias as the dependency name and never resolves what it actually points
at, so a renamed sideways dependency is invisible to every rule in the
script rather than checked under its real identity — nothing in this
workspace's conventions calls for renaming an internal crate, so this is
added to "What this does not catch" as a real, undisclosed-until-now gap
rather than fixed outright, since closing it needs the value-parsing this
script deliberately avoids. And this file's own paragraph above originally
claimed the sans-I/O fix reused "the same" boundary technique
`check-drift.sh` uses — false: `check-drift.sh` uses no boundary at all,
by design, and says so in its own header; the two scripts solve the `\b`
problem in opposite ways. Corrected above.

Both gates currently skip cleanly on this repository (`has_rust()` for the
layering gate, an empty `git ls-files` glob for sans-I/O) and stay
unexercised here until M0.

**M-1.10** wires non-negotiable 6. `check-core-contract.sh` extracts each
`pub trait Name { ... }` block by brace depth and reduces it to a **set**
of normalized `fn` signatures — a required method captured up to its `;`,
a provided method's default body skipped by depth so it is never
misread as further signatures — then compares that set between HEAD and
the staged index for every changed `.rs` file. A trait whose set differs
requires every file that currently implements it (`impl Name for` found
anywhere in the tracked tree) to also be part of this commit's changed
files, and an ADR under `docs/internal/product/decisions/` in the same
commit. Doc 19 §3.2's own limits are kept deliberately: the gate does not
try to verify an implementor's body actually matches the new signature,
only that its file is present in the commit — the same "checks only what's
mechanical" posture `check-drift.sh` and `check-tests-kept.sh` already
state for their own gates.

⚠️ Built and tested against a scratch git repository with real commits
(unlike M-1.8's gates, this one needs history — it compares HEAD against
the index, not two states of a working tree), and the manual testing found
a real defect before any review did: `index_content()` built the staged
-index git reference as `git show "::path"` — a doubled colon, from
passing `":"` as the ref into a helper that already appends `:` itself —
which fails with exit 128 on every call. The failure was swallowed to
`None` by the same helper, and every caller already treats `None` as "this
file doesn't exist," so the result was silent and wrong in the most
dangerous direction: every trait in a changed file looked like it had
vanished entirely rather than changed, `changed_traits` still populated
correctly from the `HEAD`-side comparison, but the *new* side read as
empty — coincidentally still detecting "a change occurred" while reporting
the wrong implementors (none, since `impls_by_trait` was built entirely
from the same broken helper and came back empty too). Two of the four
manual test cases run before this was found happened not to exercise the
one code path where it mattered, which is the same lesson M-1.7's own
retrospective already recorded once this task: a test that looks like it
covers a case can silently cover a different one. Fixed by not
concatenating two colons; reverified against the full case set — sideways
implementor missing, ADR missing, both present, a doc-comment-only edit, a
default-body-only edit, an unrelated function added to the same file, and
a brand-new trait in a brand-new file — all before this was ever sent to
review.

⚠️ Review found a second defect in the same shape as the first, one layer
further in. `pub trait Name` and `impl Name for` are found by plain
substring search over the raw file text, with no comment stripping — so a
trait name mentioned in a *later* comment (a stale doc example,
commented-out old code, a migration note) overwrote the real trait's entry
in the dict this builds, because matches are visited in source order and
the last one wins. Reproduced: a real, breaking signature change to a
`pub trait`, with the trait's old shape quoted verbatim in a trailing
comment, made the gate report `ok no pub trait's method set changed` —
exactly the failure mode `contracts.md` rule 13 promises cannot happen
("editing a doc comment on a trait is not a contract change"), except here
a comment made a *real* change invisible rather than a non-change visible,
the opposite direction and the more dangerous one. Fixed with a
best-effort comment stripper — `/* */` blanked out nesting-aware, `//` to
end of line, skipping a `//` that falls inside a simple `"..."` string —
applied before both the trait-signature extraction and the implementor
scan, since both are vulnerable to the identical shadowing. Not
string-literal-aware across a multi-line raw string; documented in the
script rather than solved, since a raw string inside a trait's own
signature is rare enough to accept. Reverified against the exact
comment-shadowed reproduction plus the full original case set once more.

⚠️ A second review round found the string-tracking half of that fix had the
opposite bug, reached through a different case than the one just closed: a
char literal holding a double quote, `'"'` — plausible in any wire-protocol
codec comparing a byte against a quote character, not contrived — has no
matching close on the line, so the scanner stayed "inside a string" for
the rest of the line with nothing to end it, and a genuine trailing `//`
comment was never cut. That comment's text then leaked into the real
trait's signature set exactly like round 1's defect, just through the
string-tracker instead of the missing comment-strip. Measured in the
*opposite* direction from round 1 though: every case tried turned a
non-change into a false `CHANGED`, never a real change into a false
`unchanged` — still wrong, but not the more dangerous of the two
directions this task exists to prevent. Fixed narrowly for the one
ambiguous case (`'"'` specifically), not with a general char-literal
lexer, and reverified against the exact reproduction plus the full case
set including the round-1 comment-shadow scenario and nested block
comments.

⚠️ **A third review round found the two fixes above had never actually been
combined correctly, and it was the dangerous direction again.** Both
rounds patched their own pass in isolation — the block-comment pass ran
first, over the raw file, with no string awareness at all; the string
tracker ran second, per line, over whatever the first pass had already
produced. An unmatched `/*` inside an ordinary string literal (a MIME
type, a glob pattern, a log message — plausible in ordinary code, not
adversarial) had nothing to stop it, because the pass that would have
recognized "this is a string" ran *after* the pass that blanks comments.
The result: everything from that `/*` to end of file was blanked, silently
deleting every trait declaration after it — including ones with a real,
breaking signature change — from extraction. Reproduced two ways: the
unmatched `/*` immediately before the changed trait, and buried in an
unrelated helper function earlier in the file; both made a genuine
breaking change to `pub trait Framer` disappear entirely, gate reporting
`ok` and exiting 0. The doc comment's own claim that this exposure was
"confined to a trait's own signature or an `impl ... for` line" was itself
wrong — the two-pass structure meant the blast radius was the rest of the
file, and the review that found this said so explicitly rather than taking
the disclosed-limitation framing at face value.

Fixed by replacing both passes with one: a single left-to-right scan
tracking string, block-comment, and line-comment state together, checked
in that order — a block comment can only *begin* when the scanner is not
already inside a string, which is what makes an unmatched `/*` inside a
string literal inert instead of catastrophic. The char-literal special
case from round 2 carries over unchanged. Reverified against every case
from all three rounds in one pass: both unmatched-`/*`-in-a-string
reproductions (now correctly detected as changed), the round-1
comment-shadow case, the round-2 `'"'` false-positive case, escaped
single-quote and escaped-backslash char literals each hiding a real
change behind a trailing comment, a nested block comment mentioning an
unrelated trait, and a doc-comment-only edit.

**M-1.11** closes the non-negotiables list except rule 3. `check-unsafe.sh`
enforces two things: `unsafe` outside `oqueue-buf`/`oqueue-codec`/
`oqueue-checksum` fails outright, and every `unsafe fn`/`unsafe { ... }`
site inside those three needs an immediately-preceding `// SAFETY:` comment
whose text has a baseline entry in the new `baselines/unsafe.txt`, id =
`sha256(file + "\0" + the comment's own text)` truncated to 12 hex
characters — the same shape `baselines/review.txt` already established for
argued findings and `testing.md` rule 17's mutants baseline, chosen for
the identical reason: keyed on content, not a line number, so an unrelated
edit above a baselined block never invalidates its entry. `unsafe impl
Send`/`unsafe trait` marker forms are confined to the three crates like
everything else but are not required to carry a SAFETY comment — doc 18
§5.7's six conditions are framed around a block or function with a
precondition to state, and a marker trait has none.

Built and verified against a scratch git repository: unsafe confined with
a baselined SAFETY comment passes; the identical block with no baseline
entry yet fails and prints the exact id to add; unsafe in a non-allowed
crate fails; an `unsafe fn` with no SAFETY comment at all fails; an
`unsafe impl Send` marker needs no comment but is still confined; and an
unstaged baseline entry does not satisfy the gate, read from the index for
the same reason `baselines/review.txt` is.

Round 1 found two real defects and one piece of unnecessary weight:

- **blocking**: the per-file read only caught `OSError`. A non-UTF-8 byte in
  a scanned file raises `UnicodeDecodeError`, which is not an `OSError` —
  the scanner died with Python's own exit code 1, indistinguishable on the
  bash side from "no violations found", and every file after the crashing
  one in scan order was silently never checked. Reproduced with a file
  containing one raw `\xff` byte. Fixed two ways: the read is now wrapped in
  `except (OSError, UnicodeDecodeError)`, so one unreadable file is skipped
  rather than killing the run, and the whole scan body is wrapped in
  `def build(): ...` called from a top-level `try/except Exception` that
  maps any *other* unexpected exception to a distinct exit code (3),
  matching `build-index.sh`'s own established crash-safety pattern for the
  identical reason. A skipped file is not silent either: it now prints as a
  `warn` naming the path, because a file the scanner could not read is a
  file not scanned for unsafe, and `lib.sh`'s own contract is that a gate
  states what it checked even when it passes.
- **major**: `UNSAFE_SITE` was matched with `re.search` per line, so
  `unsafe` and `{` on separate lines — valid, ordinary Rust that `rustfmt`
  does not forbid — evaded the SAFETY-comment requirement entirely.
  Reproduced with `unsafe` alone on one line and `{` on the next. Fixed by
  matching against the whole file's text once with `re.finditer` and
  mapping each match's start offset back to a line number via
  `bisect.bisect_right` over a precomputed table of line-start offsets,
  rather than searching line by line.
- **minor**: `require_sha256 || finish` was dead weight — the id is computed
  in the embedded Python via `hashlib`, never shelled out to `sha256sum`, so
  the tool-presence check gated nothing. Removed.

Reverified the full original suite after the fix (well-formed pass, missing
baseline, missing SAFETY comment, unsafe in the wrong crate, marker-impl
exemption, unstaged baseline) with no regression, plus the two new cases
above and a forced-exception run confirming the crash path fails loudly
with a distinct message rather than reporting a false "ok".

Round 2 found two more real defects, both in the round-1 fix itself:

- **blocking**: the SAFETY-comment lookback anchored to the line of the
  `fn`/`{` token (`m.start(2)` in the round-1 regex), not the line of the
  `unsafe` keyword. For a split-line site — the exact shape round 1's major
  fix was written to detect — the line immediately above the detected site
  was the `unsafe` line itself, not the comment above it, so a correctly
  placed `// SAFETY:` comment above `unsafe` was invisible to the lookback.
  Reproduced: `// SAFETY: ...` above `unsafe`, `{` on the next line, a
  correct staged baseline entry for that exact comment text — failed with
  "no SAFETY: comment" anyway. The single-line form (`unsafe {` together)
  passed with the same baseline entry, confirming the anchor was the only
  variable. Fixed by capturing the `unsafe` keyword itself as its own group
  and anchoring the lookback (and the reported line number) to its
  position, not the trailing token's.
- **major**: rule 1's confinement match was still the original whole-word
  `unsafe`, unchanged since before round 1 — it matches the English word in
  prose, not only the Rust keyword. Reproduced: a file outside the three
  crates containing only a doc comment reading "this crate ... never uses
  unsafe code, unlike oqueue-buf which does" failed the gate with no
  `unsafe` keyword anywhere in it. Not a security bypass (errs toward
  over-blocking), but likely to produce spurious failures once library
  crates carry real prose about their own safety posture. Fixed by
  requiring one of the tokens that actually follows the keyword in Rust
  syntax (`fn`, `impl`, `trait`, `extern`, or `{`), matched over the whole
  file's text so a confinement violation split across a line break (the
  same shape round 1 fixed for the SAFETY-required subset) can't evade rule
  1 either — the bash-side `grep -n` rule 1 previously used could not have
  spanned lines even if its pattern had required a trailing token, which is
  why detection for both rules now lives in the one whole-text Python scan
  rather than half in `grep` and half in Python.

Reverified the full original suite again after both fixes, plus a
correctly-annotated split-line site (now passes), a disallowed-crate file
containing only prose mentioning "unsafe" (now passes), and a
disallowed-crate site split across a line break (still correctly caught,
confirming the rule-1 move into Python didn't lose the round-1 span
coverage) — no regression anywhere.

Round 3 found two more real defects, both about legitimate Rust the earlier
rounds' fixes had not accounted for:

- **blocking**: the SAFETY lookback stopped at the first line that was
  neither a `//` comment nor blank — an attribute (`#[inline]`, `#[cold]`,
  `#[target_feature(...)]`) between a correct `// SAFETY:` comment and the
  `unsafe fn`/`unsafe {` it documents made the comment invisible, exactly
  the pattern doc 18 §5.7 and the performance standard recommend for
  hot-path unsafe functions. Reproduced with a `// SAFETY:` comment,
  `#[inline]`, then `unsafe fn` — failed despite the comment being present
  two lines above. Fixed by marking every line that belongs to an attribute
  (tracking `[`/`]` depth so a multi-line attribute, e.g. a wrapped
  `#[cfg_attr(...)]`, is covered too — the same forward `in_attr`-style scan
  `check-tests-kept.sh` already uses for its own attribute lines) and having
  the lookback skip over those lines rather than stopping at them.
- **major**: rule 1's confinement regex required one of `fn`/`impl`/`trait`/
  `extern`/`{` after `unsafe`, which does not include edition-2024's
  `#[unsafe(no_mangle)]` attribute-wrapper syntax — rustc itself requires
  the wrapper for `no_mangle`/`export_name`/`link_section`, so this is
  genuine unsafe-introducing syntax, not an oversight to exempt. Reproduced:
  a disallowed-crate file containing only `#[unsafe(no_mangle)]` over an
  `extern "C" fn` passed with zero violations reported. Fixed by adding
  `\(` to the site regex's trailing-token alternation — `unsafe` is a
  reserved word, so `unsafe(` cannot be anything but this wrapper form, no
  further disambiguation needed. Treated as confinement-only like `impl`/
  `trait`/`extern`, not SAFETY-required: it is an attribute annotation, not
  a block asserting one inline runtime invariant.

Reverified the full suite a third time after both fixes (12 cases: the six
from round 1 unchanged, plus the round-2 split-line and prose cases, plus a
multi-line-attribute-annotated site now passing, an attribute-only site
with no SAFETY comment still correctly failing, and the `#[unsafe(...)]`
wrapper correctly confined) — no regression.

Round 4 found one more real defect in round 3's own fix, plus a doc/code
mismatch:

- **blocking**: `attribute_lines()`'s depth tracker counted every `[`/`]`
  character on a line, including ones inside a string literal that is
  itself an attribute argument's value. An attribute whose string value
  contains an unbalanced `[` (`#[doc = "array[ unmatched bracket..."]`, a
  plausible `#[serde(rename = "items[0]")]`) pushed `depth` permanently
  above zero, so `in_attr` never cleared — every line for the rest of the
  file, including later genuine `// SAFETY:` comments, was silently
  swallowed into the attribute-skip set instead of being read as a comment
  or used as a boundary. Reproduced with exactly the `#[doc = "array[...`
  case; a correctly placed SAFETY comment two lines later failed the gate.
  Fixed by stripping ordinary double-quoted string content
  (`"(?:\\.|[^"\\])*"`) from each line before counting brackets — the same
  string-awareness discipline `check-core-contract.sh`'s own brace-depth
  scan had to learn the hard way. Disclosed, not chased further: a raw
  string or an unterminated (non-compiling) string literal inside an
  attribute can still miscount, the same category of accepted gap
  `check-core-contract.sh` already discloses for itself.
- **minor**: the header claimed the SAFETY/site association tolerated
  "blank lines ... in between", but the lookback actually stops at the
  first blank line — it never skipped them, only comments and attributes.
  The stricter code was correct (an accidental blank-line gap should not
  associate a comment with a much later, unrelated site); the header was
  wrong. Fixed by correcting the header's wording, not the behavior.

Reverified the full suite a fourth time (14 cases: the twelve above, plus
the bracket-in-attribute-string case now passing, plus a blank line
between comment and site still correctly failing) — no regression.

Round 5 found the same defect *class* as round 4 recurring through a
second, unstripped path, plus one more prose false positive:

- **blocking**: `attribute_lines()`'s bracket counter had no concept of
  `/* ... */` block comments. A block comment containing commented-out,
  attribute-shaped text with an unbalanced bracket count (kept-for-context
  reference code, a rejected draft) set `in_attr` and never cleared it, the
  identical whole-file cascading failure round 4 fixed for string literals,
  reached by an unstripped comment instead. A companion case, a single
  attribute containing a bracket inside a char literal
  (`#[my_tool_attr('[')]`), showed the round-4 fix (string-only stripping)
  was one instance of the general problem, not the problem itself.
- **major**: the `\(` alternative added in round 3 for `#[unsafe(...)]`
  reopened round 2's prose defect for that one token -- `unsafe(` matched
  inside an ordinary `//` comment that merely named the syntax by way of
  explaining a crate didn't need it, the same class round 2 eliminated for
  the other trailing tokens but never re-checked when `\(` was added.

Both are the same root cause surfacing a third and fourth time: matching
and counting brackets against raw source text cannot distinguish real code
from a comment or a string that merely contains code-shaped text, and
every incremental, single-shape fix (strings in round 4, now attributes
found via comments) left the next shape uncovered. Fixed by stopping the
one-shape-at-a-time patching and porting `check-core-contract.sh`'s
tested `strip_comments()` state machine wholesale, extended to also blank
string and single-quote-char-literal *interiors* (that file only ever
needed comments blanked; this gate's regex matches on syntax shape, which
strings and char literals can just as easily contain). `SITE` and the
attribute-bracket counter now both run against this one stripped text
instead of the raw one; the SAFETY-comment lookback still reads the raw
lines, since it needs the real comment text to hash, not a blanked one.
Char-literal recognition uses a bounded lookahead for the two shapes that
occur in practice (`'x'`, `'\x'`) rather than a full lifetime/char-literal
disambiguator -- the same "narrow special case, not a full lexer" posture
`check-core-contract.sh` already established for the identical ambiguity
in its own scanner, and it turned out to subsume that file's separate
`'"'`-char-literal special case for free, since char-literal detection
here happens at the opening quote, before the interior `"` is ever visited
on its own.

Reverified the full suite a fifth time (17 cases: the fourteen above,
plus the block-comment-with-unbalanced-bracket case now passing, a
bracket-in-char-literal attribute now passing, and `unsafe(no_mangle)`
named inside a `//` comment outside the allowed crates no longer failing)
— no regression.

Round 6 found the most severe defect of the six rounds: a real false
*pass* (every prior round's dangerous-direction findings were near
misses, caught before shipping; this one was live in the diff sent to
review), plus a second false-rejection defect in the same family as
rounds 4-5, plus a cosmetic one:

- **blocking**: `strip_for_scan`'s `'string'` state applies
  backslash-escaping to every double-quoted string uniformly, but raw
  strings (`r"..."`, `r#"..."#`) have no escape sequences in real Rust.
  Reproduced: `r"\"` (a common shape -- any Windows-style path literal, or
  a regex fragment ending in a literal backslash) ends its content with a
  backslash directly before the closing quote; the escape-tracking state
  interpreted that quote as escaped and kept consuming -- silently
  blanking every real character after it, including a genuine
  `unsafe { ... }` block outside the three allowed crates, until an
  unrelated later `"` or end of file. `check-unsafe.sh` reported `ok` on a
  file that plainly violates rule 1. This is the dangerous direction
  non-negotiable 7 exists to prevent, and the disclosed "what this does
  not catch" note previously understated it by lumping raw strings in
  with "does not compile" cases -- this one compiles and is ordinary.
  Fixed by recognizing the `r`/`br` prefix and pound-count via a backward
  lookahead from the opening quote (mirroring how rustc's own lexer
  resolves it) and closing on the real rule: `"` followed by exactly that
  many `#`, no escaping in between. Verified against `r"\"`,
  `r#"[a-z]+\"#`, and `br"C:\some\path\"` (pound-delimited and byte-string
  forms) each followed by a real unsafe site, and confirmed an ordinary
  escaped string (`b"a\\\"b"`) is unaffected.
- **major**: `attribute_lines()` tracked bracket depth as a whole-line net
  count (`ln.count('[') - ln.count(']')`), so a line where an attribute
  closes and unrelated real code follows on the *same* line
  (`#[derive(Debug)] struct Padding([u8; 4]);`) still balanced to zero for
  the line as a whole (the struct's own `[u8; 4]` also nets to zero) and
  the entire line -- including the struct declaration -- was marked
  transparent. The SAFETY lookback then skipped straight over it,
  letting an unrelated, far-above `// SAFETY:` comment attach to an
  unsafe site it never described: a false pass on the baseline-coverage
  check specifically, the same dangerous direction as the blocking
  finding, just one level down. Reproduced exactly as described. Fixed by
  scanning each attribute line character by character to find the exact
  column its own bracket closes at, and only marking a line transparent
  when nothing but whitespace follows that column -- a line mixing an
  attribute with real code is now left unmarked and acts as an ordinary
  boundary.
- **minor**: the `char_lit` state's fallback branch blanked every
  non-escape, non-closing character to a plain space, including `\n`,
  contradicting the function's own documented "every newline is preserved"
  invariant. Unreachable in practice (a bare newline inside `'...'` is not
  valid, compiling Rust), but fixed for free while already in this code:
  now preserves `\n` like every other blanking branch does.

Reverified the full suite a sixth time (20 cases: the seventeen above,
plus the raw-string-masking case now correctly failing on its real
`unsafe` site, the attribute-with-trailing-code case now correctly
requiring its own SAFETY comment, and two extra probes -- a
pound-delimited raw string and an escaped byte string, both followed by a
real unsafe site that is still correctly caught) — no regression.

Round 7 asked the reviewer to scrutinize the string-handling machinery
hardest, since round 6 had found a real false pass there; that machinery
held. It found a second false pass instead, in code unchanged since round
1:

- **blocking**: the main scan loop deduplicated by line number --
  `if i in seen_lines: continue` -- on the mistaken assumption that a line
  holds at most one relevant `unsafe` site. `unsafe fn f() { unsafe { ... }
  }`, or an edition-2024 `#[unsafe(no_mangle)] pub unsafe extern "C" fn
  foo() { unsafe { ... } }`, puts two or more genuinely distinct `unsafe`
  occurrences on one line, and every one after the first was silently
  never examined for a SAFETY comment or baseline entry -- a real,
  uncommented, unbaselined unsafe block reported as compliant.
  Reproduced both ways: a nested `unsafe fn`/`unsafe {}` pair sharing a
  line with only the outer site's comment baselined passed cleanly, and
  the FFI-export shape above with an entirely empty baseline file also
  passed. `SITE.finditer` never produces two matches for the same keyword
  occurrence -- each match consumes its own boundary character and
  scanning resumes strictly after it -- so every match was already a
  genuinely distinct occurrence; there was nothing to legitimately dedup,
  and the dedup itself was the bug. Fixed by removing it. A shared
  physical line means sites sharing that line also share whichever
  comment sits directly above the line, at this gate's line-level
  granularity (unchanged, documented) -- two sites needing two different
  justifications is exactly what pulling the inner block onto its own
  line already achieves, same as any other case where this gate's
  granularity asks for a line split.

Reverified the full suite a seventh time (both repros above now correctly
failing, plus the full twenty-case suite unchanged) — no regression.

Round 8, asked to scrutinize the whole file for any remaining way a real
site could be silently missed given the last two rounds' pattern, found
one more real false pass, plus confirmed round 7's fix introduced no new
false positive:

- **blocking**: `unsafe extern` is ambiguous by itself -- `unsafe extern
  "C" fn foo() { ... }` is a real function definition with a body and
  exactly one inline invariant to state (indistinguishable in shape from
  `unsafe fn`); `unsafe extern "C" { fn foo(); }` is a foreign block, a
  list of declarations with nothing to assert. `SITE` only ever captured
  the token immediately after `unsafe`, which is `extern` either way, so
  every `unsafe extern "ABI" fn` site was silently classified with the
  marker-exempt block form and never checked for a SAFETY comment or
  baseline entry at all -- the header's own "what this does not catch"
  section already correctly scoped the disclosed exemption to *blocks*
  only, but the code did not actually enforce that distinction. Reproduced
  with a bare, uncommented `pub unsafe extern "C" fn my_callback(...)` in
  an allowed crate against an empty baseline: reported `ok`. Fixed by
  peeking past the optional ABI string literal for the real continuation
  (`fn` vs `{`) and reclassifying accordingly; an unrecognized or
  ambiguous continuation resolves to requiring a SAFETY comment rather
  than to the exempt form, since a false rejection here costs a second
  look and a false exemption costs nothing and is invisible -- the same
  fail-safe default this file has settled on for every dangerous-direction
  fix from round 6 onward.
- Confirmed, not a defect: round 7's dedup removal does not introduce
  spurious double-reporting for a genuinely single site --
  `SITE.finditer` cannot produce two matches for one keyword occurrence,
  verified again directly.

Reverified the full suite an eighth time (21 cases: the twenty above,
plus five extra probes around `unsafe extern` -- a bare uncommented
extern fn now correctly failing, the same fn with a comment now passing,
an extern *block* still correctly exempt, a bare `extern fn` with no ABI
string still correctly requiring SAFETY, and an extern fn outside the
three allowed crates still correctly confined) — no regression.

Round 9, asked to hunt specifically for a fifth dangerous-direction false
pass given the pattern of the previous four (each in a different
mechanism: string handling, attribute-line tracking, line dedup, extern
disambiguation), reasoned through Rust's actual function-qualifier grammar
(`const|async → unsafe → extern → fn`, meaning `unsafe` is always
immediately followed by `extern` or `fn` for a function item, so no legal
function-definition shape exists that `SITE`'s trailing-token set doesn't
already recognize) and tested adversarial `unsafe extern` shapes directly
against the real script: a function-pointer type field, a malformed
double-ABI-string, a no-ABI-string form, an ABI string containing an
escaped quote, and an `unsafe impl` sharing a line with a genuine
`unsafe { ... }` block. **Pass, no findings** -- the first round of nine
to return clean.

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
gate triggered by one silently never runs. `.pre-commit-config.yaml` wires
every existing gate script as a local hook (`default_install_hook_types:
[pre-commit, commit-msg]`, so one `pre-commit install` gets both stages),
which is what makes a fresh clone inherit the same enforcement this repository
only ever had by hand-copying scripts into `.git/hooks` — a gap M-1.9 and
M-1.6 each left open in turn. `.github/workflows/gates.yml` runs the identical
config via `pre-commit run`, on `push: branches: [main]` rather than
`pull_request`, since there are none to trigger on.

While wiring the local hook it turned out the hand-installed one on this
machine had drifted: it ran only `check-reviewed.sh` and `check-drift.sh`,
never picking up `check-layering.sh`, `check-sans-io.sh`,
`check-core-contract.sh`, or `check-unsafe.sh` as each landed — exactly the
"a gate nothing invokes is a preference too" warning `AGENTS.md` already
carries, just discovered concretely rather than left abstract. Updated the
local hook (untracked, machine-specific, superseded by this task's own
tracked config) to match while pip access to install the real tool is
unavailable here.

The CI half surfaced two adaptations the acceptance criterion's own wording
only gestures at:

- **The "rebase onto the previous commit" adaptation is not just for a future
  `origin/main...`-based gate — it is needed now.** Every current
  pre-commit-stage gate compares the **staged index against HEAD**, which
  means nothing after a push: there is no staged state, everything is already
  committed. The direct-to-main equivalent of "staged" is the pushed commit's
  own tree, and of "HEAD" is its parent — so the workflow does
  `git reset --soft HEAD~1` before running the pre-commit-stage gates (which
  leaves the last commit's changes staged against its parent, verified by
  running this exact sequence against a disposable clone of this repository's
  real history and confirming `git diff --cached --stat` showed the expected
  commit's files), then `git reset --hard "$GITHUB_SHA"` before the two
  commit-msg-stage gates, which read HEAD's own subject and its diff against
  its parent directly rather than the index. `fetch-depth: 2` is required for
  `HEAD~1` to resolve at all — the default shallow clone has no parent
  commit; a guard (`git rev-parse HEAD~1`) handles the repository's actual
  first commit, where no rebase target exists.
- **`check-reviewed.sh` cannot run in CI at all, and this is not a gap to
  close.** Its verdict lives in gitignored `target/review/`, which
  `.gitignore`'s own comment says is "consumed on the machine that made
  [it]" — deliberate, since the verdict gates *creating* the commit and is
  regenerable, not a durable record the way `reviews/` is. A fresh CI
  checkout never has it, for any commit, reviewed or not, so running the
  check there cannot distinguish "reviewed, but off-machine" from "never
  reviewed" — it would fail every push regardless of whether review actually
  happened. Per-commit review is architecturally a local, pre-commit-time-only
  gate: the commit cannot exist in history unless it already passed locally,
  under the honesty assumption non-negotiable 3 names as the one thing no
  script enforces. `SKIP: check-reviewed` in the CI job says why, not just
  that.

⚠️ **What could not be verified in this environment, disclosed rather than
silently assumed correct: the `pre-commit` tool itself.** No `pip` module is
available in this sandbox and no attempt was made to reach the network for
one, so `pre-commit install` and `pre-commit run` were never actually
executed here. What *was* verified directly: `.pre-commit-config.yaml` and
`.github/workflows/gates.yml` both parse as valid YAML (`python3 -c "import
yaml; yaml.safe_load(...)"`, catching a real defect this way — unquoted
`on:` parses as the boolean key `true` under PyYAML's default loader, a
known YAML 1.1 quirk GitHub's own parser special-cases but a generic one does
not; quoted to `"on":` and reverified); every hook `entry` script exists and
is executable; and every entry's actual command, run directly with the exact
arguments `pre-commit` would pass (no arguments for the six pre-commit-stage
hooks, one message-file path for the two commit-msg-stage hooks), behaves as
each gate's own extensive existing test history already established — this
task added no new check logic, only decided when the existing checks run.
The `pre-commit` framework's own dispatch is trusted the way this project
already trusts `git`, `bash`, and `python3` without re-verifying them from
first principles: a widely used, independently maintained tool, not
something built or owned here.

Review found two real defects, both reproduced directly rather than reasoned
about from the diff:

- **blocking**: `.pre-commit-config.yaml`'s own header claimed it wired
  "every gate script in scripts/," and it didn't — `build-index.sh --check`
  (M-1.33's own stated pre-commit gate for the generated index regions) was
  named nowhere in the new config, the CI workflow, or the pre-existing
  hand-installed local hook, so nothing anywhere would have caught a stale
  index once this task's hooks became the enforcement path. Fixed by adding
  it as a ninth hook (`build-index-check`, pre-commit stage, `entry:
  scripts/build-index.sh --check`) and to the local hook file.
- **major**: the CI workflow's reset sequence only ever examined the
  push's tip commit against its immediate parent — correct for a
  single-commit push, silently wrong for the common case here, since the
  `milestone` skill's own working style is many commits before one push.
  Reproduced by building a disposable clone, stacking a deliberately
  malformed commit (`wip: drop it, no task id, no trailer`) behind a clean
  tip commit, and confirming the original workflow's exact commands
  reported green — the malformed commit was never examined. Fixed by
  walking every commit in `github.event.before..github.sha` in order,
  checking each one out and running the same reset-and-check sequence
  against it individually, rather than only the range's final commit.
  `fetch-depth: 2` became `fetch-depth: 0` (full history) since a fixed
  shallow depth just moves the identical silent-miss failure to whatever
  batch size exceeds it, and how large a push can be is not bounded by
  design here. Reverified the exact malformed-commit scenario against the
  corrected loop: the buried commit is now caught (`gate_status=1`,
  correctly distinguishing it from the two clean commits surrounding it in
  the same simulated push).

Round 2 found one more real defect, more severe in practical terms than
either round-1 finding because it was not hypothetical: it is the literal
state of this repository's own pending history.

- **blocking**: the corrected multi-commit walk ran `pre-commit run`
  against every commit in the pushed range unconditionally — including
  commits older than the commit that introduces `.pre-commit-config.yaml`
  itself. Reproduced against real history, not a contrived scenario: this
  repository's actual `main` was, at review time, several commits ahead of
  `origin/main` (M-1.5 through M-1.37), none of which contain the new
  config file. Installing the real `pre-commit` tool and replaying the
  workflow's exact commands against that real range failed every one of
  those commits with `.pre-commit-config.yaml is not a file` — a false
  failure indistinguishable from a real one, and "essentially guaranteed to
  fire on the very first real push." The same problem generalizes past
  this one file: `scripts/check-commit-msg.sh` and
  `scripts/check-tests-kept.sh` themselves postdate several early
  commits in the walked range (they were introduced by M-1.6), so calling
  them unconditionally hits the identical class of false failure further
  back. Fixed with one general guard (`present_at`, `git cat-file -e
  "$commit:$path"`) applied to all three invocations, rather than three
  separate special cases — a commit is now checked only against what
  existed at that commit, matching `portability.md` rule 10's "a missing
  prerequisite skips with a named reason; it does not fail" for a tool,
  applied here to a file's own existence in history instead. Reverified
  against the real repository's actual pending range (31 commits, `before`
  set to the commit preceding this whole recent batch): the guard
  correctly identifies exactly one commit (this task's own) as having the
  config, correctly skips `check-tests-kept.sh` for the 25 commits that
  predate M-1.7, correctly skips `check-commit-msg.sh` for the 10 that
  predate M-1.6, runs every gate that does apply, and the loop exits 0 —
  every one of those commits is genuinely already-valid, already-`done`
  work. Reverified the round-1 buried-malformed-commit scenario once more
  on top of this fix to confirm no regression: still caught,
  `gate_status=1`.

Round 3, with network access this time to install the real `pre-commit`
tool, replayed the workflow's exact commands against this repository's
actual unpushed history (`origin/main` at `75bf42d`, local `HEAD` six
commits ahead) rather than a contrived one, and confirmed the concrete
case round 2 found broken now exits 0 exactly as intended, with no
regression on the round-1 scenario. **Pass, no findings.**

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
