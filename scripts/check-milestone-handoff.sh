#!/usr/bin/env bash
# A milestone declared complete has handed on every row it left open. `M4.73`.
#
#   scripts/check-milestone-handoff.sh [--milestone M4]
#
# ## The loop this ends
#
# `milestone-review` turns every blocking or major finding into a backlog row,
# and `sdd.md`'s decomposition rule says only the *current* milestone is
# decomposed — so every finding lands in the milestone being reviewed, and
# reading the commits that close them is the next round's job. Findings beget
# rows beget commits beget findings. M4's three boundary rounds read **32**,
# **13** and **17** commits, recorded **12**, **8** and **11** findings at
# blocking or major, and opened **8**, **10** and **7** rows; the series
# converges only because rows-per-commit happens to be under one, not because
# anything stops it. ⚠️ The commit and finding counts are in
# `reviews/milestone-M4-*.json`; the *row* counts are not, and reconciling the
# two needs both reasons. Round two cites eight ids for ten rows because the
# other two (`M4.56`, `M4.57`) were opened by the rule-15 harvest one of its
# findings demanded, not by a finding of their own; round three cites eight
# entries for seven rows because two findings share `M4.70`. The rows
# themselves are `76e204f`, `454e23b` and `88a5e86`.
#
# ⚠️ **The way out already existed and was used twice, by hand.** `M1` closed
# with **eleven** rows it had not worked, recorded as one row in `roadmap.md`'s
# "Deferred into a later milestone" table, and `M2.0` re-derived them as its own
# rows and marked each `dissolved` with a pointer. `M10.0` did the same for
# `M3.41`-`M3.46`. Neither skill said that was allowed, so it happened when
# somebody noticed rather than by rule — and a milestone whose only stated exit
# is "no rows left" has no exit at all once its own review can add rows.
#
# ⚠️ **This gate would have printed `ok` on both precedents, having checked
# nothing**, and that is worth saying rather than implying otherwise: those rows
# carried `deferred`, a *row state* `M1.52` defined and `M2.0` retired
# (`sdd.md`), so `open_task_ids` does not see them. The rule binds going
# forward, where `todo` is the only open state there is; the precedents are
# evidence that the handoff is the established answer, not evidence that this
# gate has ever caught anything.
#
# ## What it checks
#
# For every milestone whose `roadmap.md` state cell reads `complete`, every row
# of that milestone still in an open state (`sdd.md`'s `<!-- states:open -->`
# list, read through `lib.sh`'s `open_task_ids`) is named in `roadmap.md`'s
# "Deferred into a later milestone" table.
#
# ⚠️ **Only the row's first cell — what is Deferred — counts as naming.**
# Review measured why: scanning the whole row let any backticked id anywhere in
# the table discharge a row, and `roadmap.md`'s own prose is full of them. The
# row whose Deferred cell is "How a `minor` review finding gets scheduled" says
# in its Why column that "none became a row until `M0.27`-`M0.29` harvested them
# by hand" — an aside that hands nothing to anyone, and that silently discharged
# three ids of a complete milestone. Measured against this roadmap: the whole-row
# scan treated **112** ids as handed and the Deferred-cell scan treats **17**.
# The Into cell is excluded for the same reason in
# the other direction: it legitimately names the *discharging* task (`M2.0`),
# which is a row in the receiving milestone, not one being handed on.
#
# ⚠️ **Ranges count as naming.** The M1 row writes `M1.47`-`M1.51` rather than
# five ids, which is how a reader would write it, so the table's ids are
# expanded before the comparison. A range whose endpoints are different
# milestones is not expanded — its endpoints still count as named, since they
# are written down — and it is reported rather than silently skipped. A
# lettered range (`M4.15a`-`M4.15c`) expands over its letters, and an en-dash
# reads as a range separator like the ASCII hyphen — both because that is how
# this repository's own prose writes them.
#
# ⚠️ **The id form is `lib.sh`'s `TASK_ID_RE`, not a narrower transcription.**
# `open_task_ids` emits any `M<n>.<n><letter>`, and a hand-written copy that
# accepted only `a`-`d` made a row like `M7.18e` unsatisfiable — there would
# have been no way to hand it on short of editing this file. `lib.sh`'s own note
# says an id-form widening has to visit every hand-written transcription of it;
# this is one of them.
#
# ## `--milestone`, and the window that was zero commits wide
#
# ⚠️ **Without it this gate could not fail, and `M4.79` measured that.** The walk
# above inspects milestones whose cell reads `complete` and rows that are still
# open — but the *opening commit of the next milestone* flips that cell and
# dissolves the handed rows together, in one commit, because that is what the
# `milestone-review` skill instructs and what `7fb9738` (M9/`M9.21`) and `M10.0`
# (`M3.41`-`M3.46`) each did. No commit in this repository's history has ever
# been in the intermediate state, so an M5.0-shaped commit with **no deferral
# row naming anything** printed `ok ... has handed on the rows it left open`,
# rc 0. `m4-complete.sh` could therefore report green with the disposition
# question entirely unanswered, which is what `M4.73` was written to prevent.
#
# `--milestone M4` asks the question at the one time the answer still exists:
# the closing milestone's own boundary, before the next milestone's opening
# commit. It is `check-milestone-review.sh --milestone M4`'s shape, and
# `m4-complete.sh` runs the two side by side as legs 0 and 0b.
#
# It refuses three ways rather than one, because two of them are how a
# milestone-scoped check goes vacuous:
#
#   1. **The named milestone's cell already reads `complete`** — the M5.0-shaped
#      state. The rows have been dissolved and the evidence is gone, so the only
#      honest answer is that the question was asked too late. ⚠️ This is what
#      stops a completion gate going green *after* the next milestone opens,
#      which is the whole of the defect: reporting `ok` there is exactly the
#      vacuous pass, and it is the state the walk above cannot see either.
#   2. **The backlog holds no row of that milestone at all** — `backlog.md` is
#      the current milestone's list, so a typo'd or already-archived id names
#      nothing and would otherwise pass having checked nothing.
#   3. Otherwise the same test as the walk: every open row named in a deferral
#      row's first cell.
#
# ⚠️ **The bare invocation is unchanged, and has to be.** Pre-commit runs it, and
# the current milestone always has open rows — applying the milestone-scoped test
# there would make every commit of every milestone red.
#
# ## What it cannot check
#
# ⚠️ **That the handoff was honest.** The table row says which milestone
# receives the work; nothing here reads that milestone's plan to see the work
# arrive, and `AGENTS.md` already names the two places a deferral must appear
# for exactly this reason. What this buys is that a milestone cannot be declared
# complete while rows it opened are open and unmentioned — which is the half
# that was a promise.
#
# ⚠️ **It says nothing about whether a row *should* have been handed on.** That
# is `milestone-review`'s disposition rule, which is judgement: a finding whose
# subject is behaviour a client observes blocks the milestone, and a finding
# whose subject is a sentence does not. No script can tell those apart.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

ROADMAP="docs/internal/product/roadmap.md"
BACKLOG="docs/internal/product/backlog.md"

# `check-milestone-review.sh`'s own parsing, same shape and same exit 2 for an
# argument this gate does not know.
MS=""
MS_GIVEN=0
while (( $# > 0 )); do
  case "$1" in
    --milestone) MS="${2:-}"; MS_GIVEN=1; shift; shift || true ;;
    -h|--help) sed -n '2,4p' "${BASH_SOURCE[0]}" >&2; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

# ⚠️ **A malformed id is refused by shape, and what that buys is the message.**
# `--milestone` with nothing after it leaves `MS` empty and `--milestone M4.79`
# names a *task*; review measured both already refused one step later, by the
# "holds no row of" branch, so this guard closes no vacuous pass that was open.
# What it closes is a misdirection: without it, `--milestone M4.79` fails with
# "backlog.md holds no row of M4.79", and the reader goes and writes one.
if (( MS_GIVEN )) && [[ ! "$MS" =~ ^M-?[0-9]+$ ]]; then
  fail "--milestone takes a milestone id like M4, not '$MS'"
  finish
fi

require_python || finish

for f in "$ROADMAP" "$BACKLOG"; do
  if [[ ! -f "$f" ]]; then
    fail "$f is missing, so nothing can be said about milestone handoff"
    finish
  fi
done

# ⚠️ **Read from the index, not the worktree**, for `lib.sh`'s own reason: a
# row that counts while unstaged rewards the path that leaves no trace in
# history, and this gate and `open_task_ids` must see the same bytes.
open_ids="$(open_task_ids)"

problems="$(
  ROADMAP="$ROADMAP" BACKLOG="$BACKLOG" OPEN_IDS="$open_ids" \
  ONLY="$MS" ONLY_GIVEN="$MS_GIVEN" python3 - <<'PY'
import os
import re
import subprocess
import sys
from itertools import takewhile

roadmap_path = os.environ["ROADMAP"]


def from_index(path: str) -> str:
    out = subprocess.run(
        ["git", "show", f":{path}"], capture_output=True, text=True, check=False
    )
    return out.stdout if out.returncode == 0 else ""


roadmap = from_index(roadmap_path)
if not roadmap:
    # Same silence `lib.sh` keeps for an unstaged backlog: a missing index entry
    # is a fresh checkout, not this gate's failure to report.
    # ⚠️ **Except under `--milestone`, which review measured as a fourth vacuous
    # route**: the existence check above reads the *worktree*, so a roadmap
    # removed from the index with the file still on disk reached this line and
    # printed `ok <MS> has handed on every row it leaves open`, rc 0 — a named
    # claim about a milestone, made from nothing. Silence is the right answer
    # for a walk over every milestone and the wrong one for a question about a
    # named one.
    if os.environ.get("ONLY_GIVEN") == "1":
        print(
            f"NO_ROWS\t{roadmap_path} has no entry in the index, so nothing can "
            f"be said about what {os.environ.get('ONLY', '')} handed on"
        )
    sys.exit(0)

problems: list[str] = []

# ⚠️ `lib.sh`'s `TASK_ID_RE`, transcribed rather than narrowed — see the header.
TASK_ID = r"M-?[0-9]+\.[0-9]+[a-z]?"


def _split_suffix(part: str) -> tuple[str, str]:
    """`15a` -> `(15, a)`; `15` -> `(15, '')`."""
    digits = "".join(takewhile(str.isdigit, part))
    return digits, part[len(digits) :]

# --- which milestones claim to be complete -----------------------------------
complete: list[str] = []
for line in roadmap.splitlines():
    m = re.match(r"^\|\s*\d+\s*\|\s*\[(M-?\d+)\]", line)
    if not m:
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if cells and cells[-1] == "complete":
        complete.append(m.group(1))

# --- or the one milestone the caller named, checked before its cell flips -----
only = os.environ.get("ONLY", "")
if os.environ.get("ONLY_GIVEN") == "1":
    if only in complete:
        # The M5.0-shaped state: the header's case 1. Asking now is asking
        # after the rows were dissolved, so a green answer means nothing.
        # ⚠️ Marked rather than reported as a problem, because the caller turns
        # this one into a `skip` — see the shell below for why it must not be a
        # failure.
        print(
            f"TOO_LATE\t{only} already reads `complete` in {roadmap_path}, so its "
            f"open rows have been dissolved and this check can no longer see "
            f"them -- it must run at {only}'s own boundary, before the next "
            f"milestone's opening commit"
        )
        sys.exit(0)
    backlog = from_index(os.environ["BACKLOG"])
    if not re.search(rf"^\|\s*{re.escape(only)}\.", backlog, re.M):
        # The header's case 2: a milestone with no rows would pass having
        # inspected nothing.
        print(
            f"NO_ROWS\t{os.environ['BACKLOG']} holds no row of {only}, so there "
            f"is nothing to say about what it handed on"
        )
        sys.exit(0)
    complete = [only]

# --- which ids the deferral table names --------------------------------------
# The table is everything between its own heading and the next one.
handed: set[str] = set()
in_table = False
for line in roadmap.splitlines():
    if line.startswith("## "):
        in_table = line.strip() == "## Deferred into a later milestone"
        continue
    if not in_table or not line.startswith("|"):
        continue
    cells = [c.strip() for c in line.strip().strip("|").split("|")]
    if not cells:
        continue
    # The first cell is what is Deferred; everything after it is the receiver
    # and the rationale, and ids there are prose (see this file's header).
    deferred_cell = cells[0]
    for lo, hi in re.findall(
        rf"`({TASK_ID})`\s*[-\u2010-\u2015]\s*`({TASK_ID})`", deferred_cell
    ):
        lo_ms, lo_n = lo.rsplit(".", 1)
        hi_ms, hi_n = hi.rsplit(".", 1)
        if lo_ms != hi_ms:
            problems.append(
                f"{roadmap_path}: the deferral table writes the range {lo}-{hi}, "
                f"whose ends are different milestones -- it is read as two "
                f"single ids rather than a range, so nothing between them is "
                f"handed on"
            )
            continue
        if not (lo_n.isdigit() and hi_n.isdigit()):
            # A lettered range (`M4.15a`-`M4.15c`) expands over its letters
            # when the numeric part matches; taking the endpoints alone
            # stranded every id between them, which review measured.
            lo_num, lo_suffix = _split_suffix(lo_n)
            hi_num, hi_suffix = _split_suffix(hi_n)
            if lo_num == hi_num and lo_suffix and hi_suffix and lo_suffix <= hi_suffix:
                handed.update(
                    f"{lo_ms}.{lo_num}{chr(c)}"
                    for c in range(ord(lo_suffix), ord(hi_suffix) + 1)
                )
            else:
                handed.update({lo, hi})
            continue
        if int(hi_n) < int(lo_n):
            problems.append(
                f"{roadmap_path}: the deferral table writes the range {lo}-{hi} backwards"
            )
            continue
        handed.update(f"{lo_ms}.{n}" for n in range(int(lo_n), int(hi_n) + 1))
    handed.update(re.findall(rf"`({TASK_ID})`", deferred_cell))

# --- every open row of a complete milestone must be named --------------------
open_ids = [i for i in os.environ.get("OPEN_IDS", "").split() if i]
for milestone in sorted(complete):
    prefix = f"{milestone}."
    stranded = sorted(
        i for i in open_ids if i.startswith(prefix) and i not in handed
    )
    if stranded:
        # ⚠️ The clause differs by mode, because the claim differs: the walk
        # reports a milestone that *says* it is complete, and `--milestone`
        # reports one that is *about to*. Writing "is `complete`" in the second
        # case would name a cell a reader would then go and fail to find.
        claim = (
            "is about to be declared complete"
            if os.environ.get("ONLY_GIVEN") == "1"
            else f"is `complete` in {roadmap_path}"
        )
        problems.append(
            f"{milestone} {claim} and leaves "
            f"{len(stranded)} open row(s) no deferral row names: "
            + ", ".join(stranded)
        )

print("\n".join(problems))
PY
)"

# ⚠️ **`TOO_LATE` is a skip, not a failure, and `M4.79`'s review is why.** The
# condition it names — the milestone's cell reads `complete` — is permanent, so
# failing on it would make `m4-complete.sh` exit non-zero from `M5.0` onward
# with no edit that restores it. `M4.77` exists because `m0-complete.sh` was red
# four milestones after M0 closed, and `M0.30`'s own body rejects the shape in
# terms: a gate red by design most of the time makes a real failure
# indistinguishable from work in progress. Worse, two deferrals are *discharged*
# by this gate's exit code flipping 0 to non-zero later — FR-21's durable
# offsets (`roadmap.md`, M6 task 7c) and FR-40's `GroupGrants` (M12 task 3a) —
# and a permanently red gate pre-empts both signals.
# ⚠️ **Exit 3, not `finish`.** The caller has to tell "proved nothing" from
# "proved it" and an exit code is the only channel it has; `finish` speaks only
# 0 and 1. A skip that exits 0 would let `m4-complete.sh` report leg 0b green
# having inspected nothing, which is the vacuous pass this whole task closes.
if [[ "$problems" == TOO_LATE$'\t'* ]]; then
  skip "${problems#*$'\t'}"
  note "this check proves nothing once the next milestone's opening commit has landed"
  exit 3
fi

if [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "${p#*$'\t'}"
  done <<< "$problems"
  # ⚠️ **The remedy is only printed for the problem it answers.** `--milestone`'s
  # two refusals are about *when* and *what* the question was asked of; telling
  # their caller to add a deferral row would send them to edit a table that is
  # not what refused them.
  # ⚠️ **The remedy is printed unless it is the wrong remedy**, and the test is
  # on the marker rather than on the complaint's wording: `--milestone`'s
  # `NO_ROWS` refusal is about *what* was asked, so telling its caller to add a
  # deferral row sends them to edit a table that is not what refused them.
  # ⚠️ Anything else keeps the notes, including every complaint the bare walk
  # makes — an earlier version tested for one complaint's text and silently
  # dropped the notes from the crossed-range and backwards-range failures,
  # whose fix is the very sentence about first cells and backticks.
  if [[ "$problems" != NO_ROWS$'\t'* ]]; then
    note "a milestone closes by dispositioning what it found, not by emptying its backlog"
    note "add a row to $ROADMAP's \"Deferred into a later milestone\" table whose"
    note "first cell names them, each in backticks -- the second cell is the"
    note "receiving milestone, and an id anywhere else in the row reads as prose"
    note "and the receiving milestone's plan, which AGENTS.md requires and this cannot read"
  fi
  finish
fi

if (( MS_GIVEN )); then
  ok "$MS has handed on every row it leaves open"
else
  ok "every milestone marked complete has handed on the rows it left open"
fi
finish
