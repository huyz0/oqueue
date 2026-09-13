#!/usr/bin/env bash
# Backlog state cells are well-formed, and a task with a commit is not `todo`.
# `M4.27`.
#
#   scripts/check-backlog-rows.sh    the pre-commit hook path
#
# ## Why this exists, and why it is a gate rather than a habit
#
# Three instances in one sitting. `M4.0` and `M11.0` were opening rows whose
# state cell was never flipped, both with a commit in history. `M11.3`'s row had
# been split across three lines by a stray newline **and a blank line**, which
# carried the row's own `| done |` onto an orphaned fragment — and because a
# blank line terminates a GFM table, the fifteen rows below it stopped rendering
# as a table at all. All three survived their milestone's completion gate and
# its boundary review.
#
# ⚠️ **`M9.20` found this same defect a milestone earlier and fixed the
# instance.** That is the argument for a gate: a defect that recurs after being
# fixed by hand is a class, and a class needs a check. `M4.26` fixed the three
# instances; this refuses the fourth.
#
# ## ⚠️ The leg that would have caught all three
#
# **A task id named by a commit subject resolves to a row that is not `todo`.**
# A task with a commit is a task that happened. `check-commit-msg.sh` already
# proves the id *exists*; nothing said anything about its state, which is the
# whole gap `M4.0` and `M11.0` sat in.
#
# ⚠️ **A revert needs a backlog edit, and that consequence is stated here
# because nothing else states it.** `git revert <task commit>` restores the row
# to `todo` while the original subject stays in `git log`, and the revert's own
# subject (`Revert "M4.27: …"`) does not match the anchored id pattern — so this
# leg fails the revert. That is the leg behaving as specified rather than a bug:
# a reverted task did not happen, so its row should not read `done`. Set it to
# `dissolved` with a pointer to whatever replaces it, in the revert itself.
#
# ⚠️ **Scoped to `HEAD`, never to the staged message**, and that is load-bearing
# rather than incidental: the row is flipped to `done` in the very commit that
# closes it, so at pre-commit time the *incoming* subject legitimately names a
# row that is still `todo` in the index. Checking the staged message would fail
# every closing commit — which is every commit. `check-commit-msg.sh` splits
# those two modes for its own reasons and this borrows the distinction.
#
# ## ⚠️ What it declined to check, and no longer declines
#
# ⚠️ ~~**Cell count.**~~ — **checked since `M4.28`**, which escaped the
# thirteen rows that made it impossible. The note here used to explain why the
# stronger leg was declined: GFM splits cells on unescaped `|` *including*
# inside code spans, those rows carried one, and weakening the leg to accept
# them would have been the cheaper and worse move. `M4.28` fixed the rows
# instead, and the leg is the point of having done so. It counts unescaped
# delimiters rather than cells, because counting cells means splitting the row,
# which is the thing that is broken.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

BACKLOG="docs/internal/product/backlog.md"
SDD="docs/internal/standards/sdd.md"

require_tool python3 "apt-get install python3" || finish

# ⚠️ **The index, not the working tree**, matching `lib.sh`'s own
# `_backlog_from_index` and for its reason: an unstaged row that satisfied a
# gate locally and failed the identical gate on CI is the asymmetry `M-1.39`
# already fixed once. `|| true` keeps a missing backlog silent — that is not
# this gate's failure to report.
# ⚠️ **Through `lib.sh`**, which blanks fenced blocks, so this gate and
# `known_task_ids`/`open_task_ids` see the same bytes. Reading `git show`
# directly here is what let the two disagree about a fenced example row.
backlog_text="$(_backlog_from_index)"
# ⚠️ The *raw* text too, only to check the fences balance — see below.
backlog_raw="$(git show ":$BACKLOG" 2>/dev/null || true)"
if [[ -z "$backlog_text" ]]; then
  # ⚠️ **A skip only before the first commit.** A repository being bootstrapped
  # legitimately has no backlog. A repository *with history* that stages no
  # backlog has had it deleted or emptied — and skipping there let the one gate
  # whose subject is this file pass on the commit that removes it, including the
  # leg for "history points at a task the backlog no longer lists", at maximum
  # scale. Found by review; the `sdd.md` read below already failed hard here.
  if git rev-parse --verify HEAD >/dev/null 2>&1; then
    fail "$BACKLOG is missing or empty in the index, but this repository has history"
    note "every commit in HEAD names a task that no longer resolves to a row"
    finish
  fi
  skip "backlog rows (no staged $BACKLOG, and no commits yet)"
  finish
fi

# ⚠️ The vocabulary is **`sdd.md`'s**, read from the file rather than written
# here, so the two cannot drift. The `ok`/`fail` for that read is the gate's
# first leg: `M4.27`'s own task row told the implementer to read the list from
# `sdd.md`, and `sdd.md` did not have one — the list was added there and this
# reads it back, checked in both directions the way `AGENTS.md`'s
# script-existence paragraph and `.agents/skills/README.md`'s are.
sdd_text="$(git show ":$SDD" 2>/dev/null || true)"
if [[ -z "$sdd_text" ]]; then
  fail "cannot read $SDD from the index, so the state vocabulary is unknown"
  note "this gate refuses to fall back to a list of its own — that is the drift it exists to prevent"
  finish
fi

# HEAD's commit subjects, for the not-`todo` leg. Empty before the first commit,
# which the checker treats as "nothing to check" rather than as a failure.
subjects="$(git log --pretty=%s 2>/dev/null || true)"

# ⚠️ **Through files, not argv.** The backlog is ~500 KB and passing it as an
# argument returns `Argument list too long` — which the fail-closed branch below
# correctly reported as "the rows were not checked" rather than as a pass, but
# reported it about this script's own bug. Found by running it.
scratch="$REPO_ROOT/target/tmp/backlog-rows.$$"
mkdir -p "$scratch"
printf '%s' "$backlog_text" > "$scratch/backlog.md"
printf '%s' "$backlog_raw"  > "$scratch/backlog-raw.md"
printf '%s' "$sdd_text"     > "$scratch/sdd.md"
printf '%s' "$subjects"     > "$scratch/subjects.txt"

rc=0
python3 - "$scratch" "$TASK_ROW_RE" "$TASK_STATES_OPEN" "$TASK_ID_RE" <<'PYEOF' || rc=$?
import re, sys, pathlib

scratch = pathlib.Path(sys.argv[1])
backlog = (scratch / "backlog.md").read_text()
backlog_raw = (scratch / "backlog-raw.md").read_text()
sdd = (scratch / "sdd.md").read_text()
subjects = (scratch / "subjects.txt").read_text()
problems = []

# ⚠️ **From `lib.sh`, never written here.** `M4.27`'s own task row forbids a
# third parser, and the first version of this gate was one: a Python
# transcription of the pattern `known_task_ids` and `open_task_ids` already use.
# `M10.33` had to widen that pattern in three places at once for `M10.18a`; a
# copy in a second language would have made it four.
TASK_ROW = re.compile(sys.argv[2])
DELIMITER = re.compile(r'^\|[\s:|-]+\|$')

# --- leg 0: the vocabulary, from sdd.md, checked both ways -------------------
#
# The list is the bullet block between `sdd.md`'s `states` markers. Parsing that
# block rather than grepping the whole file keeps a passing mention of `todo`
# elsewhere in the standard from silently widening the set.
# ⚠️ **Markers, not a prose anchor, and not a count.** An earlier version keyed
# on the sentence "A state is one of exactly three words" and hardcoded `3`, so
# a standard that legitimately gained a fourth state could not be edited without
# also editing this script — while the comment above claimed the gate only reads
# it. Found by review.
block = re.search(r'<!-- states:start -->\n(.*?)<!-- states:end -->', sdd, re.S)
if not block:
    problems.append("sdd.md: cannot find the state-vocabulary list between "
                    "'<!-- states:start -->' and '<!-- states:end -->'; "
                    "this gate will not guess one")
    vocab = set()
else:
    vocab = set(re.findall(r'^- `([a-z]+)`', block.group(1), re.M))
    if not vocab:
        problems.append("sdd.md: the state-vocabulary markers are there but "
                        "name no states (expected '- `state` — meaning' bullets)")

# ⚠️ **`lib.sh`'s idea of *open* is checked against `sdd.md` too, both ways.**
# For one round this was `TASK_STATES_CLOSED='done|dissolved'` in `lib.sh` and
# nothing compared it to anything — a second unchecked copy of the vocabulary
# under a comment claiming the opposite. Review added a fourth state to the
# standard, set a row to it, and `open_task_ids` called that row open while this
# gate said `ok`: the `dissolved`-looked-open bug this task fixes, one level up.
open_block = re.search(r'<!-- states:open:start -->\n(.*?)<!-- states:open:end -->',
                       sdd, re.S)
lib_open = {s for s in sys.argv[3].split("|") if s}
if not open_block:
    problems.append("sdd.md: cannot find the open-state list between "
                    "'<!-- states:open:start -->' and '<!-- states:open:end -->'")
else:
    sdd_open = set(re.findall(r'^- `([a-z]+)`', open_block.group(1), re.M))
    if sdd_open != lib_open:
        problems.append(
            f"lib.sh's TASK_STATES_OPEN is {sorted(lib_open)} but sdd.md's "
            f"open-state list is {sorted(sdd_open)} — open_task_ids and the "
            f"standard disagree about which rows something will still act on")
    stray = sdd_open - vocab
    if stray:
        problems.append(
            f"sdd.md lists {sorted(stray)} as open but does not define "
            f"{'it' if len(stray) == 1 else 'them'} in the state vocabulary")

lines = backlog.split("\n")

# --- legs 1 and 2: table structure, and the rows inside a task table ---------
#
# ⚠️ Runs, not a line-by-line scan, because the defect that recurred is a table
# that *stopped being one*. A blank line ends a GFM table, so a row split by a
# stray newline leaves every row below it outside any table — and each of those
# orphans still looks like a perfectly good row on its own line. Only the run
# they sit in reveals that it has no header.
# ⚠️ **Fences are already blanked by `lib.sh`** — the same bytes
# `known_task_ids` and `open_task_ids` read, which is what stops this gate and
# those two disagreeing about a row quoted inside a ``` block.
#
# ⚠️ **What is checked here is that they *balance*.** This file tracked fences
# itself with a bare `fence = not fence` and no balance check, so an unclosed
# ``` — one deleted closing line, or a block added without its close — silently
# blanked every row from there to the end of the file and the gate reported
# `ok` for a backlog it had stopped reading. Review reproduced it: a `todo` row
# with a commit in HEAD, and a row with no state cell, both below an unclosed
# fence, passed. That is the fail-open this script's header and its `rc != 0`
# branch exist to prevent.
# ⚠️ **`[ \t\v\f\r]`, which is C-locale `[[:space:]]` minus the `\n` these lines
# are already split on** — and `lib.sh` now pins `LC_ALL=C` so its awk means
# exactly that too, rather than whatever the ambient locale decides. This has been
# wrong in both directions: `lstrip()` was *wider* (it strips U+00A0, which awk
# does not), then `[ \t]` was *narrower*, and narrower is the fail-open one —
# a `\f`-prefixed fence toggled awk's `infence`, blanking the rest of the file,
# while the count here stayed even and reported no imbalance, so every row below
# went unchecked with the gate saying `ok`. Both were found by review
# reproducing them. Two readers meant to agree byte-for-byte must use one
# definition, not two spellings of an intended one.
fence_lines = [n for n, line in enumerate(backlog_raw.split("\n"), 1)
               if re.match('^[ \t\v\f\r]*```', line)]
if len(fence_lines) % 2:
    problems.append(
        f"backlog.md: {len(fence_lines)} code-fence lines, an odd number — the "
        f"last is at line {fence_lines[-1]} and nothing closes it, so every row "
        f"below it is read as fenced and checked by nothing")

i, runs = 0, []
while i < len(lines):
    if lines[i].startswith("|"):
        start = i
        while i < len(lines) and lines[i].startswith("|"):
            i += 1
        runs.append((start, lines[start:i]))
    else:
        i += 1

for start, run in runs:
    body = run
    has_rows = any(TASK_ROW.match(l) for l in run)
    header = None
    if len(run) >= 2 and DELIMITER.match(run[1]):
        header, body = run[0], run[2:]
    elif has_rows:
        # A run of task rows with no header+delimiter above them: the M11.3
        # signature. Naming the first row is what makes it findable.
        problems.append(
            f"backlog.md:{start + 1}: {len(run)} table line(s) with no header "
            f"and delimiter above them, starting "
            f"{run[0][:60]!r} — a row split by a stray newline leaves the rows "
            f"below it outside any table, and they stop rendering as one")
        continue
    elif start > 0 and not lines[start - 1].strip():
        # ⚠️ **A headerless run with no task rows is still a broken table**, and
        # this used to `continue` silently — so splitting a row of the M0 notes
        # table (`| Plan item | Task(s) | How |`) orphaned the rows below it with
        # exactly M11.3's rendering damage and nothing said so. Found by review.
        # ⚠️ **Scoped by what precedes the run, not by its length.** A
        # `|`-leading line inside an *open paragraph* is a GFM lazy continuation
        # — a shell pipeline wrapped across lines, which `backlog.md` really
        # does — and it starts no table however many lines follow it. A run that
        # a **blank line** precedes had a paragraph break before it, so it was
        # meant to start something. Keying on run length got this wrong in both
        # directions at once: it missed a split that orphaned exactly one row,
        # and it refused two wrapped prose lines with a diagnosis about a table
        # that never existed. Both found by review, reproduced.
        problems.append(
            f"backlog.md:{start + 1}: {len(run)} table line(s) with no header "
            f"and delimiter above them, starting {run[0][:60]!r} — not task "
            f"rows, but a table that stopped rendering as one all the same")
        continue
    else:
        continue

    # Only tables whose last header cell is `State` hold task rows. The M0 notes
    # table is three columns and legitimately ends a row in `dissolved`; reading
    # the header is what tells the two apart.
    #
    # ⚠️ **A run holding task rows must be recognised as a task table, and if it
    # is not, that is a failure and never a skip.** This tested
    # `header.endswith("| State |")` and skipped anything else silently — so one
    # missing space in `| ... | State|`, or a fifth column, disabled every leg
    # for that whole table while the gate reported ok. The backlog already
    # carries two header spellings and a milestone-opening commit hand-writes a
    # new one, so header text demonstrably drifts in this file. Failing open is
    # the outcome this script's header and its `rc != 0` branch both exist to
    # prevent; this filter was the one place it still did.
    last_cell = header.strip().strip("|").split("|")[-1].strip().lower()
    if last_cell != "state":
        if has_rows:
            problems.append(
                f"backlog.md:{start + 1}: this table holds task rows but its "
                f"header's last column is {last_cell!r}, not 'State', so none "
                f"of its rows can be checked: {header[:70]!r}")
        continue

    for offset, line in enumerate(body):
        n = start + 3 + offset
        if not line.strip():
            continue
        m = TASK_ROW.match(line)
        if not m:
            problems.append(f"backlog.md:{n}: inside a task table but not a "
                            f"task row: {line[:60]!r}")
            continue
        tail = re.search(r'\|[ \t]*([A-Za-z-]+)[ \t]*\|[ \t]*$', line)
        if not tail:
            problems.append(f"backlog.md:{n}: {m.group(1)} has no state cell "
                            f"— the row does not end in '| <state> |'")
            continue
        # ⚠️ **Cell count, the leg `M4.27` declined and `M4.28` earned.**
        # GFM splits a row on every unescaped `|`, *including* inside a code
        # span — so a row quoting `| done |`, a shell `||`, or a type like
        # `Name(&'a str) | Id(TopicId)` renders with its Notes column
        # truncated there, its State column filled from whatever followed,
        # and the rest of the row dropped from every rendered view. Thirteen
        # rows carried one; `M4.28` escaped them, and this is what stops a
        # fourteenth.
        #
        # ⚠️ **Five unescaped pipes, not four cells.** Counting cells would
        # mean splitting the row, which is the thing that is broken; counting
        # the delimiters that would do the splitting is the same fact
        # measured before it does damage. ⚠️ `(?<!\\)` is why an escaped
        # pipe does not count — and why a row may legitimately contain as
        # many as it likes.
        #
        # ⚠️ **They read correctly by coincidence today, which is the trap.**
        # Where the truncated fragment happens to be the row's real state the
        # rendered cell is right anyway; the identical construction in a
        # `todo` row displays `done`.
        # ⚠️ **The remedy has to match the direction.** Too many delimiters
        # is the defect this leg was built for; too few is a dropped cell or
        # a delimiter somebody escaped by mistake, and telling that author
        # to "escape a pipe" sends them the wrong way. Found by review.
        unescaped = len(re.findall(r'(?<!\\)\|', line))
        if unescaped > 5:
            problems.append(
                f"backlog.md:{n}: {m.group(1)} has {unescaped} unescaped "
                f"'|' where a task row has exactly 5 — an unescaped pipe "
                f"inside a cell splits the row when rendered. Escape it "
                f"as '\\|'"
            )
        elif unescaped < 5:
            problems.append(
                f"backlog.md:{n}: {m.group(1)} has {unescaped} unescaped "
                f"'|' where a task row has exactly 5 — a cell is missing, "
                f"or a delimiter was escaped that should not be"
            )
        state = tail.group(1)
        if vocab and state not in vocab:
            problems.append(f"backlog.md:{n}: {m.group(1)} has state "
                            f"{state!r}, which sdd.md does not define "
                            f"({', '.join(sorted(vocab))})")
        # ⚠️ Two state cells in a row is what the first, wrong repair of M11.3
        # produced — it read the row as truncated and appended a second one.
        if re.search(r'\|[ \t]*[A-Za-z-]+[ \t]*\|[ \t]*[A-Za-z-]+[ \t]*\|[ \t]*$', line) and \
                re.search(r'\|[ \t]*(%s)[ \t]*\|[ \t]*(%s)[ \t]*\|[ \t]*$'
                          % ("|".join(vocab or ["x"]), "|".join(vocab or ["x"])),
                          line):
            problems.append(f"backlog.md:{n}: {m.group(1)} ends in two state "
                            f"cells, not one")

# --- leg 2b: a state cell outside any table -----------------------------------
#
# ⚠️ **The leg that catches a split at a table's *last* row.** Legs 1 and 2 find
# a split by its consequence — rows below it orphaned from their header — so a
# row broken at the bottom of a table orphans nothing and slipped through. The
# fragment it leaves is a line of prose ending in `| todo |`, outside every
# table, rendering as a paragraph. Found by review, which reproduced exactly
# that against the previous version and got `ok`.
#
# ⚠️ Scoped to lines outside any *headed* run, which is what keeps the M0 notes
# table's legitimate `| ... | dissolved |` row from tripping it: that row sits
# inside a run with a header, so it is never considered here.
in_table = set()
for start, run in runs:
    if len(run) >= 2 and DELIMITER.match(run[1]):
        in_table.update(range(start, start + len(run)))

for n, line in enumerate(lines):
    # ⚠️ Fenced lines arrive blank from `lib.sh`, so `line.strip()` below drops
    # them here, in the duplicate-id scan and in the state map alike. They had
    # to be excluded in three separate places when this gate tracked fences
    # itself, and one of the three was missed for a round: a fenced example
    # naming a real id sent the author to "flip" a frozen `done` row that was
    # already correct. Doing it once, upstream, is why that cannot recur.
    if n in in_table or not line.strip():
        continue
    tail = re.search(r'\|[ \t]*([A-Za-z-]+)[ \t]*\|[ \t]*$', line)
    if tail and vocab and tail.group(1) in vocab:
        problems.append(
            f"backlog.md:{n + 1}: ends in the state cell "
            f"'| {tail.group(1)} |' but is not inside any table — half of a row "
            f"broken by a stray newline renders as a paragraph: {line[:60]!r}")

# --- leg 3: a task with a commit resolves to a row, and that row is not `todo`
#
# ⚠️ **Last-wins would hide a `todo` behind a duplicate.** Review reproduced it:
# set a row to `todo`, append a second well-formed row with the same id and
# `done` below it, and a dict built by overwriting reports the task closed. So
# duplicates are a failure in their own right rather than something this leg
# silently resolves one way — `known_task_ids` would emit such an id twice too.
states, seen = {}, {}
for n, line in enumerate(lines, 1):
    m = TASK_ROW.match(line)
    if not m:
        continue
    tid = m.group(1)
    if tid in seen:
        problems.append(f"backlog.md:{n}: {tid} already has a row at line "
                        f"{seen[tid]} — an id names exactly one row, and a "
                        f"duplicate hides whichever state is read second")
    seen[tid] = n
    tail = re.search(r'\|[ \t]*([A-Za-z-]+)[ \t]*\|[ \t]*$', line)
    if tail:
        states[tid] = tail.group(1)

# ⚠️ **`lib.sh`'s `TASK_ID_RE`, not a copy.** This hand-wrote the id shape twice,
# which made this gate the third parser its own task row forbids — and it failed
# *open*: review widened `TASK_ID_RE` the way `M10.33` once did, and a subject
# naming the new form matched the row but not this pattern, so the commit
# contributed no ids and an unflipped cell passed with `ok`.
ID = sys.argv[4]
committed = set()
for subject in subjects.split("\n"):
    head = subject.split(":")[0]
    if re.match(r'^(%s)(,\s*(%s))*$' % (ID, ID), head.strip()):
        # ⚠️ `finditer`+`group(0)`, not `findall`: the moment `TASK_ID_RE` gains a
        # capturing group — which the comment above anticipates — `findall`
        # returns the group's contents, so the gate failed closed but reported
        # `names -3` and pointed the author at the backlog instead of `lib.sh`.
        committed.update(m.group(0) for m in re.finditer(ID, head))

for tid in sorted(committed):
    # ⚠️ **Resolving to a row is half the leg, and it was missing.** The task row
    # says "every id named by a commit subject resolves to a row that is not
    # `todo`"; this read `states.get(tid) == "todo"`, so an id whose row had been
    # *deleted* passed silently — review reproduced it by removing a `done` row
    # whose commit is in HEAD and got `ok`. `check-commit-msg.sh` does not cover
    # it either: that validates only the *incoming* subject, never history.
    if tid not in states:
        problems.append(f"backlog.md: a commit in HEAD names {tid}, which has "
                        f"no row — history points at a task the backlog no "
                        f"longer lists")
    elif states[tid] == "todo":
        problems.append(f"backlog.md: {tid} is `todo`, but a commit in HEAD "
                        f"names it — a task with a commit is a task that "
                        f"happened, so the cell was never flipped")

for line in problems:
    print(f"PROBLEM {line}")
# ⚠️ **2, not 1.** An uncaught exception in this checker also exits 1, so a
# `re.PatternError` from the `TASK_ID_RE` widening this script's own comments
# anticipate landed in the `rc == 1` branch and reported "backlog rows are
# malformed" — pointing the author at `backlog.md` when the defect is in
# `lib.sh`. 2 routes it to the fail-closed branch written for exactly that.
sys.exit(2 if problems else 0)
PYEOF

rm -rf "$scratch"

if (( rc == 2 )); then
  fail "backlog rows are malformed — see the problems above"
  finish
elif (( rc != 0 )); then
  # ⚠️ Without this the gate fails **open**: any exit code the checker does not
  # choose itself — 137 from an OOM kill, 143 from SIGTERM — would fall past the
  # branch above and reach `ok`, reporting well-formed rows for a run that never
  # finished. `build-index.sh` carries the same branch for the same reason.
  fail "backlog-row checker died (exit $rc); the rows were not checked"
  finish
fi

ok "backlog rows are well-formed and every committed task is closed"
finish
