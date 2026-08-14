#!/usr/bin/env bash
# Every commit in a milestone has been read by a cross-cutting review. `M-1.37`.
#
#   scripts/check-milestone-review.sh [--milestone M-1]
#
# ## What this makes structural
#
# Doc 21 §8 designs an outer loop and then records why it does not happen: it is
# "initiated by inspiration rather than by schedule". Someone notices something,
# and writes a milestone about it. The fix it names is to make the phase a
# **required part of every milestone's completion** — which is this gate, and
# which is why a milestone's completion condition should call it.
#
# ## What it checks
#
#   1. every commit whose subject names a task in this milestone is covered by
#      some stored review artifact — except a commit whose every changed path is
#      under `reviews/`, which only records a review and would otherwise make
#      the gate unpassable by regress (see `milestone_commits` in lib.sh)
#   2. no artifact claims a commit the milestone does not contain
#   3. every blocking or major finding names a backlog task that exists, or is
#      argued in baselines/review.txt
#
# Rule 3 is the one with teeth. A cross-cutting finding that lives only in a
# review artifact is a finding nobody will act on — not because the file is
# fragile (these are tracked) but because **nothing reads it**: `next-task`
# reads the backlog, and so does everyone else. Becoming a backlog row is what
# makes it real, and it is the step doc 21 §8 names.
#
# ## What it cannot check
#
# ⚠️ **That the review was any good.** A reviewer can read nothing and return an
# empty findings list, exactly as with the per-commit gate. What this buys is
# that the commits were *named* — so a milestone cannot be declared complete
# while some of its work has never been looked at as a whole. Same class as
# non-negotiable 3, and worth saying rather than implying otherwise.
#
# ⚠️ **When to checkpoint.** Nothing here bounds how many commits may accumulate
# before a review, and no honest constant is available — it depends on how much
# a reviewer can hold at once. Running it once at the end of a long milestone is
# permitted by this gate and is a bad idea; the `milestone-review` skill says so
# and cannot enforce it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

REVIEW_DIR="reviews"
BASELINE="baselines/review.txt"
BACKLOG="docs/internal/product/backlog.md"

MS=""
while (( $# > 0 )); do
  case "$1" in
    --milestone) MS="${2:-}"; shift; shift || true ;;
    -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}" >&2; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

require_python || finish

# ⚠️ Hard failures, not skips. Without git this gate cannot enumerate the
# milestone's commits, and an empty list is indistinguishable from "everything
# is reviewed" — it printed `skip … (no commits yet)` and exited 0 for a
# milestone whose every commit was unread. A gate that cannot run must not
# report success; portability.md rule 10 asks only that the two be
# distinguishable, and here `skip` made them identical.
if ! command -v git >/dev/null 2>&1; then
  fail "git not found; the milestone's commits cannot be enumerated"
  finish
fi
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  fail "not a git repository; the milestone's commits cannot be enumerated"
  note "this gate reads history, so it cannot run against a materialised tree"
  finish
fi

MS="${MS:-$(current_milestone)}"
if [[ -z "$MS" ]]; then
  skip "milestone review (no commit names a task, so there is no milestone)"
  finish
fi

mapfile -t ALL < <(milestone_commits "$MS")

if (( ${#ALL[@]} == 0 )); then
  skip "milestone review ($MS has no commits yet)"
  finish
fi

# Read every artifact once: which commits are covered, and which findings are
# actionable. Done in Python because the alternative is parsing JSON in bash.
#
# ⚠️ **From the index, not the disk.** Per-commit review artifacts live in
# gitignored `target/` and that is right — they are keyed to a staged diff that
# is about to become a commit, and they are consumed on the machine that made
# them. A milestone's coverage is the opposite: a durable claim about history
# that a second agent, a fresh clone, and CI all have to be able to check. On
# disk it did not survive `cargo clean`, so M-1.16's completion condition would
# have passed on exactly one machine and been permanently red everywhere else —
# a gate that only one person can run is a preference again. Tracked, and read
# through `git show :path` for the reason baselines/review.txt is: a file that
# counts while unstaged rewards the path that leaves no trace in history.
parsed=""
parsed="$(
  python3 - "$REVIEW_DIR" "$MS" <<'PYEOF'
import json, subprocess, sys
review_dir, ms = sys.argv[1], sys.argv[2]
covered, findings, bad = set(), [], []
listing = subprocess.run(
    ["git", "ls-files", "--", f"{review_dir}/milestone-{ms}-*.json"],
    capture_output=True, text=True)
for path in sorted(listing.stdout.split()):
    blob = subprocess.run(["git", "show", f":{path}"], capture_output=True, text=True)
    if blob.returncode != 0:
        bad.append(path)
        continue
    try:
        v = json.loads(blob.stdout)
    except Exception:
        bad.append(path)
        continue
    if not isinstance(v, dict):
        bad.append(path)
        continue
    for c in v.get("commits", []):
        covered.add(str(c))
    for f in v.get("findings", []):
        if isinstance(f, dict) and f.get("severity") in ("blocking", "major"):
            findings.append((str(f.get("id", "MISSING-ID")),
                             str(f.get("severity")),
                             str(f.get("task_id", "")).strip() or "-"))
print("COVERED " + (" ".join(sorted(covered)) or "-"))
print("FINDINGS " + (" ".join(f"{i}:{s}:{t}" for i, s, t in findings) or "-"))
print("BAD " + (" ".join(bad) or "-"))
PYEOF
)" || parsed=""

if [[ -z "$parsed" ]]; then
  fail "could not read the milestone review artifacts"
  note "$REVIEW_DIR/milestone-$MS-*.json, read from the index"
  note "check that python3 runs: python3 -c 'print(1)'"
  finish
fi

covered_line="$(printf '%s\n' "$parsed" | grep '^COVERED ' | cut -d' ' -f2-)"
findings_line="$(printf '%s\n' "$parsed" | grep '^FINDINGS ' | cut -d' ' -f2-)"
bad_line="$(printf '%s\n' "$parsed" | grep '^BAD ' | cut -d' ' -f2-)"

if [[ "$bad_line" != "-" ]]; then
  for b in $bad_line; do
    fail "milestone review artifact is not readable JSON: $b"
  done
  finish
fi

declare -A IS_COVERED=()
if [[ "$covered_line" != "-" ]]; then
  for c in $covered_line; do IS_COVERED["$c"]=1; done
fi

# 1. every milestone commit is covered
uncovered=0
for c in "${ALL[@]}"; do
  if [[ -z "${IS_COVERED[$c]:-}" ]]; then
    uncovered=$((uncovered + 1))
    note "unreviewed $(git log -1 --format='%h %s' "$c")"
  fi
done
if (( uncovered > 0 )); then
  fail "$uncovered of ${#ALL[@]} commit(s) in $MS have not been read as a whole"
  note "run: scripts/milestone-review.sh context --milestone $MS"
  note "an artifact that exists but is not staged does not count"
  finish
fi

# 2. no artifact claims a commit the milestone does not contain
#
# ⚠️ Two very different states, and conflating them wedged this gate shut. A
# commit history no longer contains is an **amended or rebased** one: git.md
# rule 21 endorses amending freely before a push and this project never pushes
# unasked, so a message typo fixed after a review left an artifact naming a SHA
# that no longer exists. That failed forever — the argued baseline resolves
# findings, not coverage — and the only escape was hand-deleting a gitignored
# file no message named, which also discarded the coverage of every other
# commit in the milestone. That is a stale artifact, not a false claim, and it
# is already handled: the amended commit is simply uncovered until re-reviewed.
# A commit that *does* exist and belongs elsewhere is the real error rule 2 is
# for, and stays a failure.
declare -A IS_REAL=()
for c in "${ALL[@]}"; do IS_REAL["$c"]=1; done
misclaimed=0
if [[ "$covered_line" != "-" ]]; then
  for c in $covered_line; do
    [[ -n "${IS_REAL[$c]:-}" ]] && continue
    # ⚠️ Reachability from HEAD, not object existence. `git commit --amend`
    # leaves the old commit in the object database — reachable from the reflog
    # until gc — so `git cat-file -e` succeeds for it and called every amend a
    # misclaim, which is the bug this branch exists to fix.
    if git merge-base --is-ancestor "$c" HEAD 2>/dev/null; then
      fail "a review claims commit $c, which exists but is not part of $MS"
      misclaimed=$((misclaimed + 1))
    else
      warn "a review claims $c, which HEAD no longer reaches (amended or rebased)"
      note "that review is superseded; the commit that replaced it is covered above or not"
    fi
  done
fi
# ⚠️ `finish` here. Without it a rule-2 failure fell through to rule 3 and the
# run ended by printing `ok … all commits covered` under its own FAIL line —
# two contradictory statements, and the reassuring one last.
if (( misclaimed > 0 )); then
  finish
fi

# 3. every blocking or major finding became a task, or is argued
#
# ⚠️ Loud when the backlog lists no ids. Silently skipping the membership half
# of this rule while still printing `ok` makes a check that stopped running look
# like a check that passed — and this file's own header calls rule 3 the one
# with teeth. check-reviewed.sh handles the identical state the same way.
known="$(known_task_ids)"
if [[ -z "$known" ]]; then
  skip "backlog membership (the backlog lists no task ids)"
fi

# ⚠️ Read once into a variable, and `|| true`. `git show … | grep -q` is a pipe
# whose left side dies of SIGPIPE when grep exits at the first match on a large
# baseline; `pipefail` then reports that as failure, and a genuinely argued
# finding reads as unargued — with a message naming a cause that is not the
# cause. check-reviewed.sh avoids this because its helper carries the `|| true`.
baseline_text="$(git show ":$BASELINE" 2>/dev/null || true)"

# The same warning the per-commit gate gives, for the same reason: the baseline
# is read from the index, so an edit that was written but not staged looks
# exactly like no edit at all — and the operator repeats it.
if [[ -f "$BASELINE" ]] && ! git diff --quiet -- "$BASELINE" 2>/dev/null; then
  warn "$BASELINE has unstaged edits, which are ignored — stage them to argue a finding"
fi

unresolved=0
if [[ "$findings_line" != "-" ]]; then
  for entry in $findings_line; do
    id="${entry%%:*}"
    rest="${entry#*:}"
    sev="${rest%%:*}"
    tid="${rest#*:}"

    # An argued finding needs no task. The reason is required, exactly as in
    # check-reviewed.sh, and for the same reason: an id alone suppresses a
    # finding while recording nothing about why.
    # ⚠️ An `if` rather than `|| argued=$?`. The `||` form only fires when grep
    # *fails*, so the variable never became 0 and the escape could never work —
    # a finding argued in the baseline was still reported unresolved. Same
    # family as the `set -e` instances M-1.9 collected: exit-status plumbing
    # written so that the interesting branch is the one that cannot run.
    argued=1
    if [[ "$id" =~ ^[0-9a-f]{12}$ ]]; then
      if grep -qE "^[[:space:]]*${id}[[:space:]]+[^[:space:]]" <<< "$baseline_text"; then
        argued=0
      fi
    fi
    if (( argued == 0 )); then
      warn "$sev finding $id is argued in $BASELINE"
      continue
    fi

    if [[ "$tid" == "-" ]]; then
      fail "$sev finding $id names no backlog task"
      note "add a backlog row and cite it, or stage '$id  <why it is not a task>' in $BASELINE"
      unresolved=$((unresolved + 1))
    # ⚠️ `-F`, and a here-string rather than `printf | grep`. `-F` because
    # without it the id is a regular expression, and a hand-written artifact
    # whose task_id is `.*` matches every backlog row — satisfying the one rule
    # this gate calls the one with teeth while naming no real task. The
    # here-string because `grep -qxF` exits at the first match, `printf`'s
    # remaining write can then SIGPIPE, and `pipefail` reports that early exit
    # as failure — misreporting a `task_id` that genuinely exists as unknown.
    # The rest of this file was swept for this exact class; this site was
    # missed the first time, found only by an eighth review actually running it.
    elif [[ -n "$known" ]] && ! grep -qxF "$tid" <<< "$known"; then
      fail "$sev finding $id names $tid, which the backlog does not list"
      unresolved=$((unresolved + 1))
    else
      # ⚠️ Existence, not openness. Demanding the row still be `todo` on every
      # run made the gate flip red the moment the finding's task was *done* —
      # and stay red, because the artifact is cumulative and never re-read. The
      # only ways through were re-opening a finished row (a lie), arguing a
      # finding the author had actually acted on, or re-recording the verdict
      # without the finding, which is deleting a finding to make a check pass.
      # Punishing the honest path is the failure mode this project has now hit
      # three times. Whether the row was open **when it was cited** is a real
      # rule and `record` enforces it, where the author is present to fix it.
      note "$sev finding $id became $tid"
    fi
  done
fi

if (( unresolved > 0 )); then
  note "a finding that stays in a review artifact is one nothing reads; the backlog is what is read"
  finish
fi

# ⚠️ Says what it counted. Coverage is enumerated from subjects that *begin*
# with a task id of this milestone, so the shapes check-commit-msg.sh exempts —
# `Revert "…"`, a merge — are never required to be covered, and neither is a
# multi-id subject whose first id belongs elsewhere. "all N commits covered"
# claimed more than that; this claims what is true. Closing the gap means
# deciding what a revert's coverage even means, which is not this task.
ok "$MS: all ${#ALL[@]} commit(s) whose subject names an $MS task are covered"
finish
