#!/usr/bin/env bash
# A milestone that has started has its exit condition on disk, and its plan on
# the page.
#
# ## ⚠️ What this is for
#
# `milestone/SKILL.md` writes the driving loop as `until completion condition
# exits 0`, and `roadmap.md` names that condition per milestone. ⚠️ **For the
# whole of `M5` the named script did not exist** — it was `M5.27`, a `todo` row
# inside the milestone it was supposed to terminate. A loop whose exit test
# cannot be evaluated does not stop; it falls back to the only other reading
# available, *keep going until nothing is `todo`*, which that same skill names
# as a loop with no exit because a review of the work can always add rows to
# the list it is measured against.
#
# ⚠️ **Measured across every milestone that has started**, planned count from
# `roadmap.md` before its opening commit against rows in `backlog.md` today:
#
#     M0  18 -> 33    M1  18 -> 59    M2  18 -> 36    M3  19 -> 47
#     M9  16 -> 23    M10 14 -> 42    M11 12 -> 21
#     M4  18 -> 94    M5  20 -> 85
#
# ⚠️ **`M-1` is 48 -> 54 and is not one of the nine**, because its row was
# created while it was already running (`M-1.40` wrote the table): 48 is the
# last count its cell held, not a plan set before the work. Every other figure
# is the cell's value in the commit *before* that milestone's opening commit
# replaced it.
#
# Not one of the nine came in at its plan, and none overran by less than 1.4x.
# ⚠️ **The
# numbers are re-derivable and this comment is not the source**: the left-hand
# value is the `Tasks` cell this gate now requires to stay put, and the
# right-hand one is `grep -c '^| M4\.' docs/internal/product/backlog.md`.
#
# ## The two legs, and why the second is not bookkeeping
#
# ⚠️ **Every one of those left-hand numbers had been deleted.** The opening
# commit of each milestone replaced its `Tasks` cell with *see `backlog.md`* —
# a pointer to the list the work grows — so the plan was erased at the exact
# moment it became something to measure against, and the overrun above was
# invisible in the one table a human reads. Milestones that have not started
# still carry theirs. That is leg 2.
#
# ⚠️ **Neither leg asserts that a milestone is on plan**, and neither should:
# overrunning a decomposition is ordinary, and `sdd.md` says a plan is a
# hypothesis. What is not ordinary is being unable to *see* it, or having no
# terminating test at all.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT" || exit 1

ROADMAP="docs/internal/product/roadmap.md"

# ⚠️ **The staged bytes, not the working tree.** `check-milestone-handoff.sh`
# reads this same file through the index and says why: a gate on the commit
# path answers a question about what is being committed. Reading the worktree
# lets a staged `roadmap.md` with a pointer cell commit behind a clean one on
# disk, and fails a staged fix on a dirty tree. ⚠️ **The completion scripts are
# asked of the index too**, for the same reason: a `git rm`'d gate still on
# disk would otherwise pass. Found by `M5.84`'s second round.
# ⚠️ Captured once rather than re-run per use: `git show | read` under
# `pipefail` reports the writer's SIGPIPE as the pipeline's failure, which is
# `M-1.44`'s recorded idiom and read here as "the file is not in the index".
ROADMAP_TEXT="$(git -C "$REPO_ROOT" show ":$ROADMAP" 2>/dev/null || true)"

roadmap_text() {
  printf '%s\n' "$ROADMAP_TEXT"
}

if [[ -z "$ROADMAP_TEXT" ]]; then
  if [[ -f "$ROADMAP" ]]; then
    fail "$ROADMAP is not in the index"
    note "this gate reads what is being committed, not what is on disk"
    finish
  fi
  skip "milestone exit conditions (no $ROADMAP)"
  finish
fi

# ⚠️ **The table has to be found before its rows can be judged.** Round one
# made an unparsable *row* fail loudly; an unparsable *table* was still a
# silent `skip`, and it disables all ten started milestones at once rather than
# one. Measured: drop the leading `#` column — an ordinary edit to a Markdown
# table nobody thinks of as a gate input — and every row fails the numeric
# test, `started` stays 0, and the gate reports success over a tree whose
# `Tasks` cells are pointers. So the count of rows that *look like* milestone
# rows is taken first, by a pattern that does not depend on the columns.
skipped_not_started=0
looks_like=$(roadmap_text | grep -cE '^\| .*\[M-?[0-9]+\]\(milestones/' || true)
parsed=0

# The sequence table's rows are `| n | [ID](…) | name | kind | deps | tasks |
# condition | state |`. Anything else in the file — the deferral table, prose —
# has a different shape and is skipped by the field count.
started=0
while IFS='|' read -r _ _ idcell _ _ _ taskscell condcell statecell _; do
  id="$(sed -E 's/.*\[([^]]+)\].*/\1/' <<< "$idcell" | tr -d ' ')"
  [[ -n "$id" ]] || continue
  # ⚠️ **An unrecognised state fails; it does not `continue`.** A row this
  # filter drops is a milestone this gate never examines, and it reports
  # success having examined it — the shape `M5.38` and `M5.39` are about. Two
  # measured ways in: an escaped `\|` anywhere in the row shifts every field so
  # the condition cell is read as the state, and a decorated or differently
  # cased cell (`In Progress`, `in progress ⚠️`) matches nothing. Found by
  # `M5.84`'s first round, which deleted `m5-complete.sh` and watched the gate
  # print eighteen green checks.
  state="$(tr '[:upper:]' '[:lower:]' <<< "$statecell" | tr -d ' ')"
  case "$state" in
    notstarted*) skipped_not_started=$((skipped_not_started + 1)); continue ;;
    inprogress*|complete*) ;;
    *)
      fail "$id has state '$(tr -d ' ' <<< "$statecell")', which is none of"
      note "not started / in progress / complete -- so this gate cannot tell"
      note "whether it has started, and a row it cannot read is a row it does"
      note "not check. Fix the cell, or the field shift that produced it: an"
      note "escaped pipe anywhere in the row moves every column right"
      continue
      ;;
  esac
  started=$((started + 1))
  parsed=$((parsed + 1))

  # ── leg 1: the exit condition is a command that exists ──────────────────
  #
  # ⚠️ **Existence and executability, not that it passes.** A milestone gate is
  # allowed to be red for as long as its milestone is open — that is what it is
  # for. What may never happen is the loop having nothing to evaluate.
  # ⚠️ `|| true`, because under `set -e` a `grep` that matches nothing takes
  # the whole script down mid-loop — which made the empty-condition branch
  # below dead code, left this milestone's leg 2 unevaluated, and stopped every
  # row beneath it being checked with no message saying so. Found by `M5.84`'s
  # second round.
  cond="$(grep -oE '[A-Za-z0-9_./-]+\.sh' <<< "$condcell" | head -1 || true)"
  if [[ -z "$cond" ]]; then
    fail "$id is $state and names no completion condition a loop could run"
    note "milestone/SKILL.md drives 'until completion condition exits 0'"
  elif ! git -C "$REPO_ROOT" cat-file -e ":$cond" 2>/dev/null; then
    fail "$id is $state and its completion condition $cond is not on disk"
    note "a milestone whose exit test does not exist cannot terminate: the loop"
    note "falls back to 'until nothing is todo', which a review of the work can"
    note "always defeat by filing a row -- M5 went 20 planned to 85 that way"
    note "write the gate as the decomposition's first task, before any other row"
  elif [[ "$(git -C "$REPO_ROOT" ls-files --stage -- "$cond" | awk '{print $1}')" != "100755" ]]; then
    fail "$id is $state and $cond is not executable in the index (mode must be 100755)"
  else
    ok "$id: $cond"
  fi

  # ── leg 2: the plan is still legible ────────────────────────────────────
  tasks="$(tr -d ' ' <<< "$taskscell")"
  # ⚠️ **A trailing marker is allowed and a pointer is not.** `roadmap.md`
  # flags cells with `⚠️` throughout — the original `Tasks` cells were written
  # `44 ⚠️` — so requiring a bare integer would refuse the file's own
  # convention. What this rejects is the cell being a *reference* to the list
  # the work grows, which is what erased every one of these numbers.
  if [[ "${tasks%⚠️}" =~ ^[0-9]+$ ]]; then
    tasks="${tasks%⚠️}"
    actual="$(git -C "$REPO_ROOT" show ":docs/internal/product/backlog.md" 2>/dev/null \
                | grep -c "^| ${id}\." || true)"
    ok "$id: planned $tasks, backlog holds ${actual:-0}"
  else
    fail "$id is $state and its Tasks cell is '$tasks' rather than the planned count"
    note "a pointer to backlog.md is a pointer to the list the work grows, so the"
    note "plan is erased at the moment it becomes something to measure against"
    note "git log -p -- $ROADMAP has the number this cell held before $id opened"
  fi
done < <(roadmap_text | awk -F'|' 'NF >= 9 && $2 ~ /^ *[0-9]+ *$/')

# ⚠️ **Parsed fewer rows than the table has is a failure, not a skip.**
if (( looks_like == 0 )); then
  fail "$ROADMAP holds no row naming a milestone -- the sequence table is gone or renamed"
elif (( parsed + skipped_not_started < looks_like )); then
  fail "$ROADMAP has $looks_like milestone row(s) and this gate could read $((parsed + skipped_not_started))"
  note "the sequence table's columns moved: this gate reads | # | id | name |"
  note "kind | depends | tasks | condition | state |, and a row it cannot read"
  note "is a milestone it does not check"
elif (( started == 0 )); then
  skip "milestone exit conditions (no milestone has started yet)"
fi

finish
