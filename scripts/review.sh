#!/usr/bin/env bash
# Turn a review from a claim into an artifact. `M-1.9`.
#
#   scripts/review.sh hash                  sha256 of the staged diff
#   scripts/review.sh context --task M-1.9  the packet the reviewer is given
#   scripts/review.sh record --file v.json  validate a verdict and store it
#   scripts/review.sh show                  the stored verdict for this diff
#
# ## The mechanism, and why it is shaped like this
#
# Non-negotiable 3 says a test claim cannot be checked against an intention, and
# no script can fix that. Review claims *can* be fixed, by keying the verdict to
# the exact bytes reviewed:
#
#     hash = sha256(git diff --cached)  ->  target/review/<hash>.json
#
# Amend one byte and the hash moves, the artifact no longer matches, and
# `check-reviewed.sh` refuses the commit. There is no way to satisfy the gate
# except by reviewing the bytes actually being committed. Doc 21 §5.
#
# ## What this script does not do
#
# ⚠️ **It does not spawn the reviewer.** A shell script cannot start an agent in
# a way that works across tools, and hard-coding one vendor's CLI would put a
# procedure in the enforcement path — the fork this project's skills exist to
# avoid. So the split is:
#
#   - the script owns the hash, the packet, the schema, and the artifact
#   - the agent owns the judgement
#
# `context` writes a packet any tool can feed to any reviewer; `record` refuses
# anything that is not a well-formed verdict about *this* diff. Both halves are
# mechanical. The judgement in between is the part that was never mechanical.
#
# ## What the packet deliberately withholds
#
# The author's plan, transcript, and justification. Doc 21 §4: an author's
# rationale is persuasive by construction, and a reviewer given it grades the
# rationale instead of the code. The packet is assembled from the backlog and
# the diff alone precisely so that *the task says X and the diff does Y* stays
# visible.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

REVIEW_DIR="target/review"
BACKLOG="docs/internal/product/backlog.md"

# How many rounds a task gets. `review.md` rule 15a is where the number and the
# argument for it live; this is the packet's copy of it.
#
# ⚠️ **Two copies, and a test rather than a promise.** An earlier comment here
# claimed the cap was "named once" so packet and standard could not drift —
# which was false the moment it was written, since rule 15a states the number in
# prose. `tests/harness/review-packet.sh` compares the two, so changing one
# without the other reds CI instead of shipping a packet that contradicts the
# standard it cites.
REVIEW_ROUND_CAP=3

# Shows the reviewer the change, as a delta from the previously reviewed tree
# when there is one and as the whole staged diff when there is not.
#
# ⚠️ **Every path out of here falls back to the whole diff** (`M5.51`), and
# that direction is the point: a delta that silently showed nothing would be a
# review of nothing that recorded a verdict, which is the one failure this can
# have. So an absent sidecar, an unreadable one, a tree object git no longer
# has, and an empty delta each print the full diff and say why.
emit_diff() {
  local previous="$1" tree="" delta=""
  if [[ -n "$previous" && -f "$REVIEW_DIR/$previous.tree" ]]; then
    tree="$(tr -d '[:space:]' < "$REVIEW_DIR/$previous.tree")"
    # ⚠️ `cat-file -e`, because `target/` is disposable: a pruned or garbage
    # collected tree is an ordinary state here, not a broken one.
    if [[ -n "$tree" ]] && git cat-file -e "$tree^{tree}" 2>/dev/null; then
      delta="$(git diff "$tree" --cached 2>/dev/null || true)"
    fi
  fi

  if [[ -n "$delta" ]]; then
    printf '## What changed since the last round\n\n'
    printf 'Everything not shown here was in the diff round %s reviewed and\n' \
      "$((round - 1))"
    printf 'already carries a verdict. Re-reading it is what made review cost\n'
    printf 'more than the work it reviewed.\n\n'
    printf '```diff\n%s\n```\n\n' "$delta"
    printf 'The full staged diff is still what the verdict is bound to, and\n'
    printf '`git diff --cached` shows it if a finding needs its surroundings.\n\n'
    return
  fi

  printf '## The staged diff\n\n'
  if [[ -n "$previous" ]]; then
    printf '⚠️ The whole diff, although this is not round one: '
    if [[ ! -f "$REVIEW_DIR/$previous.tree" ]]; then
      printf 'the previous round\nrecorded no tree to compare against.\n\n'
    elif [[ -z "$tree" ]] || ! git cat-file -e "$tree^{tree}" 2>/dev/null; then
      printf 'the tree the previous round\nreviewed is no longer in the object database.\n\n'
    else
      printf 'the delta against the previous\nround is empty, so the change is elsewhere or the tree is unchanged.\n\n'
    fi
  fi
  printf '```diff\n'
  git diff --cached
  printf '```\n\n'
}

# Gates excluded from the "already passed" list in the packet, each for a
# reason. Everything else matching scripts/check-*.sh is discovered, so a gate
# added by a later task appears here without anyone remembering to add it.
#
#   check-commit-msg.sh  the message does not exist yet at review time; run with
#                        no argument it would check HEAD, i.e. the *previous*
#                        commit, and report a pass about the wrong thing
#   check-reviewed.sh    checks for the artifact this run is about to produce
#   check-milestone-review.sh
#                        ⚠️ two reasons, and the second is the sharper one.
#                        It is a *milestone completion* gate, not a per-commit
#                        one — a commit is not required to have had its whole
#                        milestone re-read. And it reads git history, which the
#                        materialised tree deliberately does not have, so under
#                        the planted `.git` it fails; before that plant existed
#                        it did something worse and reported `passed` for a
#                        milestone whose every commit was unread.
# ⚠️ `check-mutants.sh` is here because it *cannot* run meaningfully in this
# harness: it narrows to `git diff --cached`, and the staged tree this script
# plants has no usable index, so the gate skips — which rendered as "passed" in
# every packet and told the reviewer a gate had run when it had not. `M0.17`.
#
# ⚠️ **`check-mutants-baseline.sh` is excluded because it takes arguments.**
# `M4.60` split the baseline's staleness check out of `check-mutants.sh` so
# it can run once over a sharded run's union, which means it needs a
# survivors directory and a shard count — and the discovery below runs every
# `check-*.sh` bare. Run with none it prints its usage and exits 1, so every
# packet from here on would carry a permanently red row, in a list the
# reviewer is told not to re-check. That is worse than a missing row: it
# trains the next reviewer to ignore the one place a genuinely broken gate
# would show. The nightly is where it runs; `tests/gates/negative.sh` is
# where it is watched failing.
#
# ⚠️ **`check-budget.sh` is excluded because it measures a run, not a tree**
# (`M5.67`). Its input is `target/timings/suite.tsv`, which `target/` keeps out
# of every tree this materialises, so it can only ever report itself unrun —
# and a row that is permanently unrun trains the same blindness a permanently
# red one does. The suite's wall clock is the commit hook's question and the
# nightly's, not a reviewer's.
# ⚠️ **Two gates are named here rather than run, and the reason is
# containment** (`M5.76`). `check-crate.sh` and `check-coverage.sh` invoke
# `cargo` — a workspace build, a test run, and an instrumented `llvm-cov`
# build — and the scratch tree has no `target/` of its own, so each one is a
# from-scratch build. Measured on this repository, on the materialised tree:
# `check-coverage.sh` 74.8 s wall and **827 s of CPU**, `check-crate.sh`
# 120.2 s and **1149 s** — 11.1 and 9.6 cores of the 20 this host has, so
# roughly half of it for three and a quarter minutes. That is ~195 s and ~33
# CPU-minutes on **every review packet**. ⚠️ **Not saturation, and the
# difference matters**: a decision sized from "the host is pinned" would be
# sized from a figure nothing measured, and what actually bounds an
# uncontained build on WSL2 is memory rather than cores.
#
# ⚠️ **And it happens on the host, which is the part that is not merely slow.**
# `AGENTS.md` says reviewing stays on the host precisely because `review.sh`
# needs no toolchain, and says in the same breath that anything invoking
# `cargo` goes through `scripts/docker-test.sh` — containment, not tidiness: on
# WSL2 an unbounded build exhausts the VM and takes the session with it. A
# packet that runs two uncontained from-scratch builds is the one thing that
# rule forbids, done by the script that exists to check the rules.
#
# ⚠️ **Reported, not silently dropped**, which is the difference between this
# list and `GATES_EXCLUDED` above. An excluded gate leaves no row at all, and a
# reviewer cannot tell a gate nobody ran from a gate nobody thought of. These
# two are green in the container on every commit — the hook runs them there —
# so what the packet owes the reviewer is the statement that it makes no claim
# about them, and why.
GATES_NOT_RUN_HERE=(
  "check-crate.sh|invokes cargo; reviewing stays on the host and an uncontained workspace build is what AGENTS.md's container rule forbids (1149 s of CPU against a scratch tree). The commit hook runs it in the container."
  "check-coverage.sh|invokes cargo llvm-cov; an instrumented from-scratch build, 827 s of CPU against a scratch tree, uncontained on the host. The commit hook runs it in the container."
)

# The reason this gate is named rather than run, or empty if it is not in the
# list above.
not_run_reason() {
  local want="$1" entry
  for entry in "${GATES_NOT_RUN_HERE[@]}"; do
    if [[ "${entry%%|*}" == "$want" ]]; then
      printf '%s' "${entry#*|}"
      return 0
    fi
  done
  return 1
}

GATES_EXCLUDED=(
  check-commit-msg.sh
  check-reviewed.sh
  check-milestone-review.sh
  check-mutants.sh
  check-mutants-baseline.sh
  check-budget.sh
)

staged_hash() {
  # --diff-filter is deliberately absent: a deletion is part of what is being
  # reviewed. Binary files appear as a marker line, which still changes the
  # hash when their content changes.
  git diff --cached | sha256_stdin
}

# Run the deterministic gates against **the staged bytes**, not the working
# tree.
#
# ⚠️ This distinction is the whole point. The hash and the diff come from the
# index; if the gates ran in the working tree they could report a pass for a
# version of a file that is not being committed — stage a broken change, restore
# the file, and the packet would assert a gate passed while the commit carries
# the breakage. The reviewer is explicitly told not to re-check these, so a
# false line here is a hole nothing downstream closes.
#
# ⚠️ Two constraints pull against each other here, and both are kept.
#
# The tree must not look like part of a git repository. Under `target/` it is
# still inside the work tree, so `git rev-parse` there resolves to the real
# `.git`: a gate written as `git ls-files … | xargs grep` would run, see zero
# files because the cwd is an ignored subdirectory, exit 0, and the packet would
# print "passed" for a change it never looked at — while telling the reviewer
# not to re-check it. So a deliberately invalid `.git` file is planted at the
# root, and such a gate errors instead of silently seeing nothing. It then lands
# in GATES_EXCLUDED with a reason, which is what this design assumes.
#
# And the tree must stay under `target/`. build.md rule 19 keeps scratch out of
# the system temp directory because it is commonly a RAM-backed tmpfs — 16 GB on
# the machine this is developed on — and once M0's workspace exists this copy is
# the whole Rust source tree. `mktemp -d` put it in RAM and left it there on an
# interrupted run, with nothing to reap it; `target/tmp` is reclaimed by
# `cargo clean` and by rule 20's age sweep.
# Makes the scratch tree a repository the gates can actually read.
#
# ⚠️ **`M5.67`, and the defect it closes is a false green.** The tree used to
# carry a planted `.git` *file* — enough to stop a gate reading the real tree,
# which was the point, but a gate that then finds nothing tracked calls
# `lib.sh`'s `skip` and exits **zero**. Nine of the twenty-two gates did
# exactly that, and until `M5.55` taught the packet to tell a skip from a pass
# every one of them was reported to every reviewer as green while checking
# nothing. Reporting them honestly was `M5.55`; giving them something to read
# is this.
#
# ⚠️ **An alternates file, not a clone.** `git init` plus
# `objects/info/alternates` pointing at the real object store costs no copy and
# no pack transfer, which matters because this runs on every packet. What it
# buys is `HEAD`: the two gates that diff against it — `check-core-contract`
# and `check-tests-kept` — need the parent commit's objects reachable, and
# `git add -A` then makes the index differ from `HEAD` by exactly the staged
# diff, which is the comparison they are written against.
#
# ⚠️ **A failure here is reported, never swallowed.** A scratch tree that is
# not a repository leaves every git-reading gate skipping, and the caller says
# so — the whole point of this task is that an unreadable tree must not look
# like a clean one.
stage_as_repository() {
  local tree="$1" objects head
  objects="$(cd "$REPO_ROOT" && git rev-parse --git-path objects 2>/dev/null)" || return 1
  [[ -d "$objects" ]] || return 1
  case "$objects" in
    /*) ;;
    *) objects="$REPO_ROOT/$objects" ;;
  esac
  rm -f "$tree/.git"
  clean_git init -q "$tree" || return 1
  printf '%s\n' "$objects" > "$tree/.git/objects/info/alternates" || return 1
  # ⚠️ **`HEAD` is set only if the real repository has one.** A tree with no
  # commits yet is the bootstrap case, and a gate that says "no HEAD yet" there
  # is telling the truth rather than failing.
  if head="$(cd "$REPO_ROOT" && git rev-parse --verify HEAD 2>/dev/null)"; then
    clean_git -C "$tree" update-ref HEAD "$head" || return 1
  fi
  # `add -A` against the checked-out staged bytes, so `git diff --cached` here
  # is the diff under review.
  #
  # ⚠️ **`--force`, so the materialised `.gitignore` cannot remove a path.**
  # Every file here came from `git checkout-index`, so every one of them is
  # tracked — but a path that is tracked *and* ignored would be skipped by a
  # plain `add`, and would then read as a staged deletion to every diff-reading
  # gate. There are none today; `--force` is what keeps that true of tomorrow
  # rather than of today.
  clean_git -C "$tree" add -A --force || return 1
}

# `git`, with the caller's git environment removed.
#
# ⚠️ **Because an inherited variable here writes to the *real* repository.**
# With `GIT_INDEX_FILE` set, `git -C "$tree" add -A` writes the scratch tree's
# entries into the real index, pointing at blobs in the scratch object store —
# and the `RETURN` trap then deletes those objects, leaving the change under
# review unreadable. With `GIT_DIR` set, `git init "$tree"` re-initialises the
# real repository and creates nothing at `$tree` at all. Both return zero, so
# neither trips the failure branch. Measured in `M5.67`'s first round.
clean_git() {
  env -u GIT_DIR -u GIT_INDEX_FILE -u GIT_WORK_TREE -u GIT_OBJECT_DIRECTORY \
      -u GIT_ALTERNATE_OBJECT_DIRECTORIES -u GIT_COMMON_DIR -u GIT_NAMESPACE \
      git "$@" 2>/dev/null
}

# Runs one gate and says what actually happened to it.
#
# ⚠️ **Three outcomes, not two, and the third is the point of `M5.55`.** A gate
# that cannot run here exits **zero** — `lib.sh`'s `skip` prints a line and the
# gate `finish`es clean — so a packet with two outcomes reported it as
# **passed**. That is a verdict the packet never obtained, printed to a
# reviewer as one it did: the exact converse shape `M0.17` closed, arrived at
# from the other side.
#
# ⚠️ **Reviewing stays on the host** (`AGENTS.md`), and the host has no
# guarantee of `cargo-llvm-cov` or a nightly toolchain — so "could not run
# here" is the ordinary case for two or three gates in this list rather than an
# anomaly. Saying so is what lets a reviewer spend attention on what is
# genuinely unchecked instead of on a gate the container is green on.
#
# ⚠️ **A skip line and no `ok` line**, not a skip line alone. `check-budget.sh`
# prints `skip suite budget (…)` on a run that did check everything else, and
# calling that "not run" would understate it in the other direction. What no
# `ok` means is that the gate reached none of its own checks.
#
# ⚠️ **Both patterns allow leading whitespace, and that is load-bearing.**
# `lib.sh` prints `ok` as `'%s  ok %s %s'` — two spaces before the word — so an
# anchored `/^ok /` matches nothing any gate emits, and every gate that skipped
# a leg after passing several checks was reported as having run none.
# `check-conformance-matrix.sh` is the measured case: two `ok` lines, then two
# `skip image pins` lines. Found by `M5.55`'s first round, in the packet that
# round was reading.
report_gate() {
  local label="$1" out grc=0 reason
  shift
  out="$("$@" 2>&1)" || grc=$?
  if (( grc != 0 )); then
    printf -- '- %s: **FAILED**\n' "$label"
    return 1
  fi
  # ⚠️ `awk`, and it strips its own ANSI: `lib.sh` colours `skip` when stdout is
  # a terminal, and a `grep '^skip '` misses the coloured form. `sed
  # 's/\x1b...//'` was the alternative and `\x1b` is a GNU extension
  # (`portability.md` rule 21) — the same escape-class trap `M5.51` hit with
  # `\t`.
  reason="$(awk '
    { line = $0; gsub(/\033\[[0-9;]*m/, "", line) }
    line ~ /^[[:space:]]*ok /   { saw_ok = 1 }
    line ~ /^[[:space:]]*skip / && !skipped {
      sub(/^[[:space:]]*skip /, "", line)
      skipped = line
    }
    END { if (!saw_ok && skipped != "") print skipped }
  ' <<< "$out")"
  if [[ -n "$reason" ]]; then
    printf -- '- %s: **not run here** — %s\n' "$label" "$reason"
  else
    printf -- '- %s: passed\n' "$label"
  fi
  return 0
}

run_gates_on_staged_tree() {
  local h="$1" tree base excluded g rc=0 reason

  # Reap orphans from earlier runs, not only this run's tree. An interrupted
  # run leaves its own behind, and the per-hash `rm -rf` below never matches it
  # because the hash has moved on.
  rm -rf target/tmp/review-staged-* 2>/dev/null || true

  tree="target/tmp/review-staged-$h"
  rm -rf "$tree"
  mkdir -p "$tree" || {
    printf -- '- ⚠️ could not create a scratch directory; no gate was run\n'
    return 1
  }
  # ⚠️ A RETURN trap fires on function return, **not on a signal**, so it does
  # not cover the interrupted run that motivated it. The sweep above is what
  # actually reaps that; this covers the normal and early-return paths.
  trap 'rm -rf "'"$tree"'"' RETURN
  if ! git checkout-index -a --prefix="$tree/" 2>/dev/null; then
    printf -- '- ⚠️ could not materialise the staged tree; no gate was run\n'
    return 1
  fi
  if ! stage_as_repository "$tree"; then
    # ⚠️ **The barrier goes back up, and that is not optional.** `$tree` lives
    # under `target/`, inside the repository being reviewed, so a scratch tree
    # with no `.git` of its own has git walk *up* and find the real one — and
    # every gate would then check the working tree instead of the staged
    # bytes, reporting a verdict about the wrong thing while looking green. A
    # `.git` file is not a repository, so a git-reading gate finds nothing and
    # says so, which is the honest degradation.
    rm -rf "$tree/.git"
    printf 'not a git repository — planted by review.sh so no gate reads the real tree\n' \
      > "$tree/.git"
    printf -- '- ⚠️ the scratch tree could not be made a repository, so every\n'
    printf -- '  git-reading gate below reports itself **not run here**\n'
  fi

  for g in "$tree"/scripts/check-*.sh; do
    [[ -f "$g" ]] || continue
    base="$(basename "$g")"
    excluded=0
    for x in "${GATES_EXCLUDED[@]}"; do
      [[ "$base" == "$x" ]] && excluded=1
    done
    (( excluded )) && continue
    if reason="$(not_run_reason "$base")"; then
      printf -- '- %s: **not run here** — %s\n' "$base" "$reason"
      continue
    fi
    report_gate "$base" bash "$g" || rc=1
  done

  if [[ -f "$tree/scripts/build-index.sh" ]]; then
    report_gate 'build-index.sh --check' bash "$tree/scripts/build-index.sh" --check || rc=1
  fi

  return $rc   # the RETURN trap removes the tree
}

require_staged() {
  if git diff --cached --quiet; then
    fail "nothing is staged, so there is nothing to review"
    finish
  fi
}

# Extract one task's row from the backlog, verbatim. The backlog is the only
# statement of intent the reviewer gets, which is why it is quoted rather than
# summarised.
task_row() {
  local id="$1"
  grep -E "^\| ${id//./\\.} \|" "$BACKLOG" || true
}

# ---------------------------------------------------------------------------

SUB="${1:-}"
shift || true

TASK=""
FILE=""
while (( $# > 0 )); do
  case "$1" in
    # ⚠️ `shift; shift || true`, never `shift 2`. With the flag present but its
    # value missing — `review.sh context --task`, which is what an unset
    # variable interpolates to — `shift 2` has one argument left, returns
    # non-zero, and `set -e` kills the script with zero bytes on both streams.
    # The `fail "no task given"` diagnostic written for exactly that case sits
    # thirty lines below and never runs. `${2:-}` anticipated the missing
    # value; the shift did not.
    --task) TASK="${2:-}"; shift; shift || true ;;
    --file) FILE="${2:-}"; shift; shift || true ;;
    -h|--help) sed -n '2,7p' "${BASH_SOURCE[0]}" >&2; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

# Every subcommand below hashes the staged diff, so the tool is checked once
# here rather than in four places — three of which would otherwise reach
# `sha256sum: command not found` and exit 127, a code no gate here promises.
case "$SUB" in hash|context|record|show) require_sha256 || finish ;; esac

case "$SUB" in

# ---------------------------------------------------------------------------
hash)
  require_staged
  staged_hash
  ;;

# ---------------------------------------------------------------------------
context)
  require_staged

  TASK="${TASK:-${OQUEUE_TASK:-}}"
  if [[ -z "$TASK" ]]; then
    fail "no task given"
    note "usage: scripts/review.sh context --task M-1.9   (or set OQUEUE_TASK)"
    note "the task is what the diff is checked against; without it a reviewer"
    note "can only ask whether the code is nice, which is the linter's job"
    finish
  fi

  row="$(task_row "$TASK")"
  if [[ -z "$row" ]]; then
    fail "$TASK is not in $BACKLOG"
    note "a review against a task nobody wrote cannot check conformance"
    finish
  fi

  h="$(staged_hash)" || { fail "could not hash the staged diff"; finish; }

  printf '# Review packet\n\n'
  printf 'Task: %s\n' "$TASK"
  printf 'Staged-diff sha256: %s\n\n' "$h"

  printf '## The task, verbatim from the backlog\n\n'
  printf '%s\n\n' "$row"

  printf '## Standards this change is judged against\n\n'
  printf 'Read each of these. They were selected from the staged paths, not chosen\n'
  printf 'by the author.\n\n'
  # ⚠️ Neither the exit status nor stderr may be dropped here. Discarding them
  # meant that a standard with no `applies_to` — which M-1.5 is about to add
  # five of — killed this script mid-packet under `set -e`, leaving a file that
  # looked like plausible Markdown but contained no diff, no gate list and no
  # verdict instructions, with nothing on stderr saying why. A reviewer handed
  # that file reviews nothing and its `pass` satisfies the gate.
  # ⚠️ A fixed path under target/tmp, not `mktemp`. A bare `ws_err="$(mktemp …)"`
  # is itself a call whose non-zero return is meaningful: an unusable TMPDIR —
  # a cleaned CI job directory, a read-only /tmp, a full tmpfs — killed this
  # function one line before the guard written to catch exactly this, emitting
  # a truncated packet with no diff and no FAIL line. It also contradicted the
  # argument 40 lines above for keeping scratch out of the system temp dir.
  ws_out=""; ws_rc=0
  mkdir -p target/tmp || { fail "cannot create target/tmp"; finish; }
  ws_err="target/tmp/which-standards.err"
  ws_out="$(./scripts/which-standards.sh 2>"$ws_err")" || ws_rc=$?
  if (( ws_rc != 0 )); then
    # Straight to stderr, not through `note`, which writes to stdout — and
    # stdout is the packet. A diagnostic that lands inside a truncated packet
    # is a diagnostic nobody reads.
    fail "which-standards.sh failed; the packet would name no standards"
    cat "$ws_err" >&2
    rm -f "$ws_err"
    finish
  fi
  rm -f "$ws_err"
  printf '%s\n' "$ws_out" | sed 's/^/- /'
  printf '\n'

  # ⚠️ **Selected from the paths, like the standards above** (`M5.52`,
  # `review.md` rule 12a). What the reviewer is judged against was derived and
  # what it was asked to look at was improvised per task, which is how
  # attention goes to whatever was most recently on the author's mind.
  # ⚠️ Neither its exit status nor its stderr is dropped, for the reason the
  # standards block above gives: a selector that failed silently would leave
  # the packet asserting a brief it never printed.
  printf '## What to look at, selected from the paths\n\n'
  # ⚠️ **stdout into the packet, stderr to stderr**, which is what the
  # standards block above does and what an earlier draft of this claimed to do
  # while folding the two together with `2>&1`. A diagnostic inside the packet
  # is a diagnostic the reviewer reads as a brief: a deletion-only change made
  # the lens section read "no paths, so no lenses" with no FAIL line anywhere.
  lens_out=""
  lens_rc=0
  lens_out="$(./scripts/review-lenses.sh)" || lens_rc=$?
  if (( lens_rc != 0 )); then
    fail "review-lenses.sh failed; the packet would name no lenses"
    finish
  fi
  printf 'These are the classes of defect that have got through in this\n'
  printf 'repository on paths like these, with the tasks that earned each one.\n'
  printf 'They route attention; every deterministic gate runs regardless.\n'
  printf '%s\n\n' "$lens_out"

  printf '## Deterministic gates, and what each one actually did\n\n'
  printf 'Run against the staged bytes, not the working tree. Do not re-check a\n'
  printf 'gate that passed — attention spent there is attention not spent on\n'
  printf 'what only a reader can catch.\n\n'
  printf '⚠️ **not run here** is neither a pass nor a failure, and it covers\n'
  printf 'three different things — read each row'"'"'s reason, because they are not\n'
  printf 'worth the same. A gate may have *run* and found nothing of its own to\n'
  printf 'check (no .rs file staged, say), which is a satisfied gate rather than\n'
  printf 'a hole. A gate may need a toolchain this host has not got. And two —\n'
  printf '`check-crate.sh` and `check-coverage.sh` — are deliberately not\n'
  printf 'invoked: the toolchain is here, but they are from-scratch cargo builds\n'
  printf 'and reviewing runs on the host, where AGENTS.md forbids an uncontained\n'
  printf 'build. Those two are green in the container on every commit, because\n'
  printf 'the commit hook runs them there.\n\n'
  printf 'A packet that printed a verdict it never obtained would be telling a\n'
  printf 'reviewer the tree is checked where it is not.\n\n'
  gate_failed=0
  run_gates_on_staged_tree "$h" || gate_failed=1
  printf '\n'

  # ⚠️ **The round number and the rule are printed, not left to memory**
  # (`M5.51`). Rule 15 — a `minor` on a `pass` is recorded and the commit
  # lands — is read at the start of a task, and the verdict that tests it
  # arrives many turns later carrying one word. `M5.50` spent five rounds on a
  # documentation-only commit for exactly that reason.
  round_info=""
  if [[ -x "$(command -v python3 || true)" ]]; then
    round_info="$(python3 "$REPO_ROOT/scripts/lib/review_rounds.py" "$TASK" "$h" "$REVIEW_DIR" 2>/dev/null || true)"
  fi
  # ⚠️ Here-strings rather than `printf | sed | head` (`portability.md` rules
  # 21-22): `head` closing the pipe early is a `SIGPIPE` this script takes
  # under `pipefail`, and the value is already in a variable.
  round="$(awk -F= '$1 == "ROUND" { print $2; exit }' <<< "$round_info")"
  previous="$(awk -F= '$1 == "PREVIOUS" { print $2; exit }' <<< "$round_info")"
  [[ -n "$round" ]] || round=1

  printf '## Which round this is\n\n'
  printf 'This is round %s of %s for %s.\n\n' "$round" "$REVIEW_ROUND_CAP" "$TASK"
  # ⚠️ The shape of the rounds, and it is `review.md` rule 15a's shape rather
  # than a second account of it. A branch for a three-round shape was written
  # here and removed in the same task: it was unreachable at every cap this
  # repository has had, which makes it `M5.53`'s prose landed early rather than
  # code, and `M5.53` is where the cap and the sentence move together.
  printf 'Round one finds; round two fixes and finds in the fix; round three\n'
  printf 'verifies.\n'
  # ⚠️ **This clause states `review.md` rule 15a and must move with it.** The
  # cap above is a number the standard and this script share; what a blocking
  # finding does to it is prose that lives only there, and an earlier draft of
  # this banner shipped `M5.53`'s intended rule under the rule as it stands —
  # so a reviewer read "the commit is too big, split it" while the author read
  # 15a's "correctness has no round limit". Two readers, two procedures, one
  # packet.
  printf 'A **blocking** finding lifts the cap: correctness has no round\n'
  printf 'limit. A `minor` never lifts it, and a reviewer still finding new\n'
  printf 'blocking defects past round %s is describing a change that should be\n' \
    "$REVIEW_ROUND_CAP"
  printf 'withdrawn and re-cut rather than re-polished. A round past the cap\n'
  printf 'for anything that is not blocking needs a signed line in\n'
  printf '`reviews/overrides.md`, and no standing authority to sign one is in\n'
  printf 'force.\n\n'
  printf '⚠️ Only `blocking` and `major` stop a commit. A `pass` carrying\n'
  printf '`minor` findings lands: they are recorded in the commit body or become\n'
  printf 'a backlog row. ⚠️ **Opening a round for a `minor` is forbidden**\n'
  printf '(`review.md` rule 15), not discouraged, and fixing one quietly is\n'
  printf 'the same thing: the fix moves the hash, and a moved hash is a round.\n'
  printf 'Measured here: `M5.50` took five rounds on a change to one Markdown\n'
  printf 'file, and every round after the first was opened for a finding that\n'
  printf 'never blocked.\n\n'
  printf 'So report severity honestly and do not hunt for a minor to justify the\n'
  printf 'round. An empty findings list is a valid and expected outcome.\n\n'

  if [[ -n "$previous" ]]; then
    printf '## What earlier rounds left open\n\n'
    # ⚠️ **`awk`, not `sed`, because the fields are tab-separated.** `\t` in a
    # `sed` script is a GNU extension and BSD `sed` reads it as a literal `t`,
    # so on macOS — a supported platform, `portability.md` rule 2 — the
    # substitutions matched nothing while the `grep` above still saw the line.
    # The heading and "⚠️ Verify these" printed over an empty list, which the
    # reviewer cannot tell from "there were none": the one failure this whole
    # change must not have, in the section that exists to prevent it.
    # ⚠️ **And a here-string, not `printf | grep`** (`portability.md` rules
    # 21-22): `round_info` is already a variable, and under `pipefail` a
    # producer whose reader exits early is a failure this script would take.
    open_list="$(awk -F'\t' '
      $1 == "FINDING"  { printf "- [%s] %s — %s: %s\n", $2, $3, $4, $5 }
      $1 == "SCENARIO" { printf "  scenario: %s\n", $2 }
    ' <<< "$round_info")"
    if [[ -n "$open_list" ]]; then
      printf '%s\n\n' "$open_list"
      printf '⚠️ Verify these. A finding an earlier round raised and the author\n'
      printf 'did not fix is what this round exists to catch.\n\n'
    else
      printf 'Nothing blocking or major. An earlier round passed or raised only\n'
      printf 'minors, and a minor does not open a round.\n\n'
    fi
  fi

  emit_diff "$previous"

  printf '## What you are not given\n\n'
  printf 'The author'"'"'s plan, transcript, or justification, and you must not go\n'
  printf 'looking for them. Reconstruct intent from the task and the diff alone.\n'
  printf 'Follow `.agents/skills/review/SKILL.md`.\n\n'

  printf '## How to return the verdict\n\n'
  printf 'Write JSON and hand it to:\n\n'
  printf '    scripts/review.sh record --file <path> --task %s\n\n' "$TASK"
  printf '⚠️ **Record every verdict, `changes-requested` as much as `pass`.**\n'
  printf 'The round counter above and the delta the next round is shown are\n'
  printf 'derived from recorded verdicts and from nothing else, so a round that\n'
  printf 'ends without one costs the next round its delta and makes it call\n'
  printf 'itself round one. A verdict that only ever existed as a message is a\n'
  printf 'verdict the harness cannot see (`M5.51`).\n\n'
  printf 'The task is named twice on purpose: the verdict carries one and the\n'
  printf 'command carries one, and a mismatch means the verdict came from a\n'
  printf 'different review. It is refused rather than stored.\n\n'
  printf '```json\n'
  cat <<JSON
{
  "task_id": "$TASK",
  "diff_sha256": "$h",
  "reviewer": "<model or agent name>",
  "verdict": "pass",
  "findings": [
    {
      "file": "path/to/file",
      "line": 42,
      "severity": "blocking",
      "summary": "one sentence naming the defect",
      "failure_scenario": "concrete inputs or state that produce a wrong result"
    }
  ]
}
JSON
  printf '```\n\n'
  printf 'Severity is one of blocking, major, minor. `verdict` is "pass" only when\n'
  printf 'there are no blocking findings. An empty findings list is a valid and\n'
  printf 'expected outcome; invented findings are worse than none.\n'

  if (( gate_failed )); then
    printf '\n' >&2
    warn "a deterministic gate is failing — fix that before spending a review"
  fi
  ;;

# ---------------------------------------------------------------------------
record)
  require_staged
  require_python || finish
  TASK="${TASK:-${OQUEUE_TASK:-}}"
  [[ -n "$FILE" ]] || { fail "record needs --file <verdict.json>"; finish; }
  [[ -f "$FILE" ]] || { fail "no such file: $FILE"; finish; }

  h="$(staged_hash)" || { fail "could not hash the staged diff"; finish; }
  mkdir -p "$REVIEW_DIR" || { fail "cannot create $REVIEW_DIR"; finish; }

  # ⚠️ **The reviewed index, written as a tree, so the next round can be shown
  # a delta** (`M5.51`). A `diff_sha256` identifies a review but cannot be
  # diffed against; a tree can. `git write-tree` writes objects and changes no
  # ref, so this neither moves the index nor creates anything a later command
  # has to clean up. ⚠️ **It is written before the verdict is validated on
  # purpose**: a rejected verdict leaves a tree nobody reads, and the converse
  # — a stored verdict with no tree — silently costs the next round its delta.
  # A failure here is not fatal for the same reason: the fallback is the whole
  # diff, which is what every round got before this existed.
  if ! git write-tree > "$REVIEW_DIR/$h.tree" 2>/dev/null; then
    rm -f "$REVIEW_DIR/$h.tree"
    note "could not record the staged tree; the next round will get the whole diff"
  fi

  # Validation is the anti-slop half of this script. The rules that matter:
  #
  #   - diff_sha256 must equal the hash of what is staged *now*, so a verdict
  #     cannot be carried over from an earlier version of the change
  #   - every finding needs a concrete failure scenario, because a finding that
  #     cannot say how it fails is a style opinion and style is the linter's job
  #   - verdict and findings must agree, so a reviewer cannot list blocking
  #     findings and then pass
  # `|| rc=$?` rather than a bare call followed by `rc=$?`: lib.sh sets `set -e`,
  # so an uncaught non-zero exit here would kill the script before it could
  # report anything, losing the summary line and returning python's exit code
  # instead of the 1 that every gate in this repository promises.
  rc=0
  python3 - "$FILE" "$h" "$REVIEW_DIR/$h.json" "$TASK" <<'PYEOF' || rc=$?
import hashlib, json, os, subprocess, sys

src, expected_hash, dest, asked_task = sys.argv[1:5]
problems = []

try:
    v = json.load(open(src))
except Exception as e:
    print(f"PROBLEM verdict is not valid JSON: {e}")
    sys.exit(2)

if not isinstance(v, dict):
    print("PROBLEM verdict must be a JSON object")
    sys.exit(2)

got = v.get("diff_sha256", "")
if got != expected_hash:
    problems.append(
        "diff_sha256 does not match what is staged now.\n"
        f"        verdict says: {got or '(missing)'}\n"
        f"        staged diff:  {expected_hash}\n"
        "        the change moved after the review; re-review it")

if not v.get("task_id"):
    problems.append("task_id is missing")
# ⚠️ Cross-checked, not merely accepted. A verdict reused from a previous change
# carries the previous task's ID; the artifact would then claim the diff was
# judged against acceptance criteria it never saw, while the commit subject —
# validated independently by check-commit-msg.sh — names a different task. The
# whole point of the artifact is that traceability, so nothing may be silently
# wrong about it.
elif asked_task and v["task_id"] != asked_task:
    problems.append(
        f"verdict says task {v['task_id']!r} but the review was run for "
        f"{asked_task!r}")

verdict = v.get("verdict")
if verdict not in ("pass", "changes-requested"):
    problems.append(f"verdict must be 'pass' or 'changes-requested', got {verdict!r}")

findings = v.get("findings", [])
if not isinstance(findings, list):
    problems.append("findings must be a list")
    findings = []

SEVERITIES = ("blocking", "major", "minor")
for i, f in enumerate(findings):
    where = f"findings[{i}]"
    if not isinstance(f, dict):
        problems.append(f"{where} must be an object")
        continue
    for key in ("file", "summary", "failure_scenario"):
        if not str(f.get(key, "")).strip():
            problems.append(f"{where}.{key} is empty")
    if f.get("severity") not in SEVERITIES:
        problems.append(
            f"{where}.severity must be one of {'/'.join(SEVERITIES)}, "
            f"got {f.get('severity')!r}")
    # A stable identity for the finding, so an argued finding stays argued
    # across unrelated edits. Keyed on what the finding *is* -- file and
    # claim -- never on the diff hash, which moves for reasons that have
    # nothing to do with this finding.
    f["id"] = hashlib.sha256(
        (str(f.get("file", "")) + "\0" + str(f.get("summary", ""))).encode()
    ).hexdigest()[:12]

def of(sev):
    return [f for f in findings if isinstance(f, dict) and f.get("severity") == sev]

blocking, major = of("blocking"), of("major")

if blocking and verdict == "pass":
    problems.append(
        f"verdict is 'pass' but there are {len(blocking)} blocking finding(s)")
# `changes-requested` is allowed for major findings too. Pinning it to
# `blocking` alone left a reviewer who found two genuine major defects no way to
# say so: the only accepted spelling was `pass`, the gate then reported a clean
# pass, and the findings went into a gitignored file and were never seen again.
if verdict == "changes-requested" and not blocking and not major:
    problems.append(
        "verdict is 'changes-requested' but no finding is blocking or major")

if problems:
    for p in problems:
        print(f"PROBLEM {p}")
    sys.exit(2)

# Stamped by the script rather than supplied by the reviewer: the reviewer has
# no reason to be trusted with a clock, and nothing depends on it being theirs.
v["recorded_at"] = subprocess.run(
    ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], capture_output=True, text=True
).stdout.strip()
v.setdefault("reviewer", "unknown")
# `M2.8` (M1.48): who recorded this, beside the hash it binds. The identity
# is `OQUEUE_SESSION` when the driving session exports one, else a weak but
# honest host:parent-pid fallback. This is diagnosis, not locking -- M1.48's
# row already rejected the lockfile: the value is that check-reviewed.sh can
# show *whose* verdict sits in the index when the staged bytes moved, which
# is how two sessions sharing one index stops being invisible.
v["recorded_by"] = os.environ.get("OQUEUE_SESSION") or (
    f"{os.uname().nodename}:ppid-{os.getppid()} (OQUEUE_SESSION unset)")

with open(dest, "w") as fh:
    json.dump(v, fh, indent=2, sort_keys=True)
    fh.write("\n")

print(f"WROTE {dest}")
print(f"FINDINGS {len(findings)} total, {len(blocking)} blocking, {len(major)} major")
# The ids are printed because arguing a finding needs one, and baselines/review.txt
# told the reader to look here for it. Reading them out of the JSON by hand was
# the actual workflow, which is a documented claim that was not true of the code.
for f in findings:
    if f.get("severity") in ("blocking", "major"):
        print(f"  {f['id']}  {f.get('severity')}  {f.get('file')}: {f.get('summary')}")
PYEOF

  if (( rc != 0 )); then
    fail "verdict rejected"
    finish
  fi
  ok "verdict recorded for $h"
  note "$REVIEW_DIR/$h.json"
  ;;

# ---------------------------------------------------------------------------
show)
  require_staged
  h="$(staged_hash)" || { fail "could not hash the staged diff"; finish; }
  if [[ -f "$REVIEW_DIR/$h.json" ]]; then
    cat "$REVIEW_DIR/$h.json"
  else
    printf 'no review recorded for the staged diff (%s)\n' "$h" >&2
    exit 1
  fi
  ;;

# ---------------------------------------------------------------------------
*)
  printf 'usage: scripts/review.sh {hash|context|record|show} [--task ID] [--file F]\n' >&2
  exit 2
  ;;
esac

finish
