#!/usr/bin/env bash
# A milestone declared complete has handed on every row it left open. `M4.73`.
#
#   scripts/check-milestone-handoff.sh
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
  ROADMAP="$ROADMAP" OPEN_IDS="$open_ids" python3 - <<'PY'
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
        problems.append(
            f"{milestone} is `complete` in {roadmap_path} and leaves "
            f"{len(stranded)} open row(s) no deferral row names: "
            + ", ".join(stranded)
        )

print("\n".join(problems))
PY
)"

if [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "$p"
  done <<< "$problems"
  note "a milestone closes by dispositioning what it found, not by emptying its backlog"
  note "add a row to $ROADMAP's \"Deferred into a later milestone\" table whose"
  note "first cell names them, each in backticks -- the second cell is the"
  note "receiving milestone, and an id anywhere else in the row reads as prose"
  note "and the receiving milestone's plan, which AGENTS.md requires and this cannot read"
  finish
fi

ok "every milestone marked complete has handed on the rows it left open"
finish
