#!/usr/bin/env bash
# The outer loop: cross-cutting review over a milestone's commits. `M-1.37`.
#
#   scripts/milestone-review.sh commits              the milestone's commits
#   scripts/milestone-review.sh coverage             which are reviewed, which are not
#   scripts/milestone-review.sh context              the packet for the uncovered ones
#   scripts/milestone-review.sh record --file v.json validate a verdict and store it
#
# All take an optional `--milestone M-1`; without it the milestone is the one
# HEAD is working in — see current_milestone below for why that is derived from
# history rather than from the backlog.
#
# ## Why an outer loop exists at all
#
# Per-commit review reads a delta against a task. It structurally cannot see
# what only shows up across commits: drift, two concepts that each passed review
# and contradict each other, an abstraction that should now be extracted or
# collapsed, a standard that quietly stopped being followed — and **the spec
# being wrong**, which no amount of code review reaches. Doc 21 §8.
#
# ⚠️ That document also records why this has to be mechanical: in practice the
# outer loop is "initiated by inspiration rather than by schedule". Someone
# notices, and writes a milestone about it. A phase that runs when somebody
# thinks of it is not a phase.
#
# ## What it is keyed to, and why that is not a diff
#
# The per-commit gate keys on `sha256(git diff --cached)` because the claim is
# about those bytes. Here the claim is about **a set of commits**, so that is
# the key. Reviews are incremental: each artifact names the commits it read, and
# the gate unions them. A new commit in the milestone is simply uncovered until
# some review covers it — which is the property that makes this checkpointable
# rather than a single pass at the end.
#
# ## What the packet gives the reviewer, and what it deliberately does not
#
# Commit messages in full, the task list, and a diffstat — then the instruction
# to read the **current state** of the files. ⚠️ A cross-cutting review that
# reads deltas is doing per-commit review again, more expensively. The question
# is not "was each step right" — that was already asked and answered — but "is
# where we arrived coherent".
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ⚠️ Tracked, unlike the per-commit reviewer's `target/review/`. See the note in
# check-milestone-review.sh: a milestone's coverage is a durable claim about
# history, so it has to survive `cargo clean` and reach a fresh clone.
REVIEW_DIR="reviews"
BACKLOG="docs/internal/product/backlog.md"
ROADMAP="docs/internal/product/roadmap.md"

# Every commit any stored review for this milestone claims to have read.
# ⚠️ From the index, so this agrees with the gate. An artifact written but not
# staged reports as no coverage here too, rather than showing a milestone
# covered that the gate will refuse.
covered_commits() {
  local ms="$1"
  python3 - "$REVIEW_DIR" "$ms" <<'PYEOF' || true
import json, subprocess, sys
review_dir, ms = sys.argv[1], sys.argv[2]
listing = subprocess.run(
    ["git", "ls-files", "--", f"{review_dir}/milestone-{ms}-*.json"],
    capture_output=True, text=True)
for path in sorted(listing.stdout.split()):
    blob = subprocess.run(["git", "show", f":{path}"], capture_output=True, text=True)
    if blob.returncode != 0:
        continue
    try:
        v = json.loads(blob.stdout)
    except Exception:
        continue
    if isinstance(v, dict):
        for c in v.get("commits", []):
            print(c)
PYEOF
}

# ---------------------------------------------------------------------------

SUB="${1:-}"
shift || true

MS=""
FILE=""
while (( $# > 0 )); do
  case "$1" in
    # ⚠️ `shift; shift || true`, never `shift 2`. A flag whose value is missing
    # leaves one argument, `shift 2` returns non-zero, and `set -e` kills the
    # script before the diagnostic below can run. See M-1.9's backlog note.
    --milestone) MS="${2:-}"; shift; shift || true ;;
    --file) FILE="${2:-}"; shift; shift || true ;;
    -h|--help) sed -n '2,9p' "${BASH_SOURCE[0]}" >&2; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; exit 2 ;;
  esac
done

require_tool git "install git" || finish
# ⚠️ `require_tool` only says the binary exists. Outside a repository git printed
# `fatal: not a git repository` to stderr and this said `skip <MS> has no commits
# yet` to stdout, exit 0 — asserting a 24-commit milestone had none, with the
# diagnostic on the stream nobody is piping. The sibling gate hard-fails here.
if ! git rev-parse --git-dir >/dev/null 2>&1; then
  fail "not a git repository; the milestone's commits cannot be enumerated"
  finish
fi

MS="${MS:-$(current_milestone)}"
if [[ -z "$MS" ]]; then
  fail "no milestone given, and no commit names a task"
  note "usage: scripts/milestone-review.sh $SUB --milestone M-1"
  finish
fi
# ⚠️ Needed by covered_commits, which every subcommand calls. Without it every
# artifact silently failed to parse, so `coverage` announced `ok … N not yet
# reviewed` for commits that were reviewed — an `ok` line asserting something
# false, which is worse than the skip lib.sh's contract asks for.
require_python || finish

mapfile -t ALL < <(milestone_commits "$MS")
# ⚠️ The twin of `check-milestone-review.sh`'s branch, and it must make the
# same distinction — `M1.46` fixed the gate and review found this one still
# short-circuiting `commits`, `coverage` *and* `context` with an exit-0 skip.
# That mattered because the gate's own failure message sends the operator
# here: they would have run the diagnosis and been told there is nothing to
# diagnose. An empty enumeration beside a non-zero raw count means every
# commit naming the milestone was review bookkeeping.
raw="$(git log --format='%H %s' | grep -cE "^[0-9a-f]+ ${MS//./\\.}\.[0-9]+[,:]" || true)"
if (( ${#ALL[@]} == 0 )); then
  if (( raw > 0 )); then
    fail "$MS names $raw commit(s), all of them milestone-review bookkeeping"
    note "milestone_commits excludes verdict-only commits, so there is no work to review"
    finish
  fi
  skip "$MS has no commits yet"
  finish
fi

mapfile -t COVERED < <(covered_commits "$MS")
declare -A IS_COVERED=()
for c in "${COVERED[@]}"; do IS_COVERED["$c"]=1; done

declare -a UNCOVERED=()
for c in "${ALL[@]}"; do
  [[ -n "${IS_COVERED[$c]:-}" ]] || UNCOVERED+=("$c")
done

case "$SUB" in

# ---------------------------------------------------------------------------
commits)
  printf '%s\n' "${ALL[@]}"
  ;;

# ---------------------------------------------------------------------------
coverage)
  # ⚠️ "commit(s) naming a task", not "commit(s)". A `Revert "…"` or merge
  # subject is not enumerated, so the milestone may hold more commits than this.
  ok "$MS has ${#ALL[@]} commit(s) naming a task; ${#UNCOVERED[@]} not yet reviewed"
  for c in "${UNCOVERED[@]}"; do
    note "uncovered $(git log -1 --format='%h %s' "$c")"
  done
  finish
  ;;

# ---------------------------------------------------------------------------
context)
  if (( ${#UNCOVERED[@]} == 0 )); then
    ok "every commit in $MS is already reviewed"
    finish
  fi

  first="${UNCOVERED[0]}"
  last="${UNCOVERED[-1]}"

  printf '# Milestone review packet: %s\n\n' "$MS"
  printf 'Commits to read: %d (of %d in the milestone)\n\n' \
    "${#UNCOVERED[@]}" "${#ALL[@]}"

  printf '## Who should be reading this\n\n'
  cat <<'TEXT'
⚠️ **An agent that did not drive this milestone.** The inner loop is explicit
that a change is reviewed by someone who did not write it (non-negotiable 4),
and every argument for that is stronger here, not weaker: the defects this
review exists to find — drift, a contradiction each half of which looked right,
a spec that should not have been written that way — are precisely the ones
invisible to whoever produced them, because producing them *was* the blind spot.

Nothing in the artifact can check this. It is bought by dispatching a fresh
agent, and saying so is the same discipline as non-negotiable 3.
TEXT
  printf '\n'

  printf '## What this review is for\n\n'
  cat <<'TEXT'
Not "was each commit right" — that was asked and answered per commit, by a
reviewer who had the task and the diff. This asks whether **where the work
arrived is coherent**, which only becomes visible across commits:

- **Drift.** A convention followed early and quietly abandoned.
- **Contradiction.** Two things that each passed review and disagree.
- **Abstraction.** Something now duplicated enough to extract, or an
  indirection that never earned itself and should collapse.
- **A standard that stopped being followed**, including one nothing enforces.
- ⚠️ **The spec being wrong.** No per-commit review reaches this, because every
  commit was faithful to a task that should not have been written that way.
- **A gate that passes for the wrong reason**, or one whose failure path nobody
  has run.

⚠️ **Read the current state of the files, not the diffs.** A cross-cutting
review that reads deltas is doing per-commit review again, more expensively.
The commit messages below are for intent and sequence; the code and documents
as they now stand are the subject.
TEXT
  printf '\n'

  printf '## The milestone, from the roadmap\n\n'
  # From this milestone's heading to the next one at the same level. ⚠️ The
  # trailing space in the pattern is what stops `M-1` matching `M-10`.
  roadmap_section="$(awk -v pat="^## ${MS} " '
    $0 ~ pat { p = 1; print; next }
    p && /^## / { exit }
    p { print }
  ' "$ROADMAP" 2>/dev/null || true)"
  # ⚠️ Say so rather than rendering nothing. Every milestone in the roadmap now
  # has its own `## M<n> — …` heading, so an empty section means one was renamed
  # or dropped — **not** that the milestone legitimately has no entry. Treat it
  # as a defect in the roadmap; an empty section reads as "the milestone had no
  # goals", and a cross-cutting review with no statement of intent is the wrong
  # review. (The roadmap once carried a `## M2–M7` span heading covering six
  # milestones at once; that is what this fallback was originally written for.)
  if [[ -z "${roadmap_section//[[:space:]]/}" ]]; then
    printf '⚠️ **No section for %s in %s.** Nothing here states what this\n' "$MS" "$ROADMAP"
    printf 'milestone was for; say so in the verdict rather than inferring it.\n'
    warn "no roadmap section for $MS; the packet says so"
  else
    printf '%s\n' "$roadmap_section"
  fi
  printf '\n'

  printf '## The tasks, from the backlog\n\n'
  backlog_rows="$(grep -E "^\| ${MS//./\\.}\.[0-9]+ \|" "$BACKLOG" || true)"
  if [[ -z "$backlog_rows" ]]; then
    printf '⚠️ **No task rows for %s in %s.**\n' "$MS" "$BACKLOG"
    warn "no backlog rows for $MS; the packet says so"
  else
    printf '%s\n' "$backlog_rows"
  fi
  printf '\n'

  printf '## The commits under review\n\n'
  for c in "${UNCOVERED[@]}"; do
    printf '### %s\n\n```\n' "$(git log -1 --format='%h %s' "$c")"
    git log -1 --format='%B' "$c"
    printf '```\n\n'
  done

  printf '## What changed across them\n\n```\n'
  # ⚠️ The first uncovered commit may be the **root**, which has no `~1`. It is
  # here: M-1.0 is this repository's root and its first unreviewed commit, so
  # the very first intended use hit this. The old fallback showed that one
  # commit's stat under a heading claiming it was the shape of all of them —
  # 73 files of research corpus, and none of the work under review. The empty
  # tree is the correct base for a range that starts at the root.
  base="${first}~1"
  git rev-parse --verify --quiet "${base}^{commit}" >/dev/null \
    || base="$(git hash-object -t tree /dev/null)"
  git diff --stat "$base" "$last"
  printf '```\n\n'

  printf '## Standards\n\n'
  printf 'All of them. This review is cross-cutting, so nothing is routed out:\n\n'
  for s in docs/internal/standards/*.md; do printf -- '- %s\n' "$s"; done
  printf '\n'

  printf '## How to return the verdict\n\n'
  printf 'Write JSON and hand it to:\n\n'
  printf '    scripts/milestone-review.sh record --file <path> --milestone %s\n\n' "$MS"
  printf '```json\n'
  cat <<JSON
{
  "milestone": "$MS",
  "commits": [$(printf '"%s", ' "${UNCOVERED[@]}" | sed 's/, $//')],
  "reviewer": "<model or agent name>",
  "verdict": "pass",
  "findings": [
    {
      "kind": "drift|contradiction|abstraction|standard-not-followed|spec-wrong|gate-wrong",
      "severity": "blocking|major|minor",
      "summary": "one sentence naming what is incoherent",
      "evidence": "the files or commits that show it, and what to compare",
      "task_id": "M-x.y"
    }
  ]
}
JSON
  printf '```\n\n'
  printf '⚠️ **Every blocking and major finding must name a `task_id` that the\n'
  printf 'backlog lists and has not marked `done`.** That is what "findings\n'
  printf 'become backlog tasks" means here — a finding recorded only in a review\n'
  printf 'artifact is one nothing reads, and a closed row is one next-task will\n'
  printf 'never surface again. Add the row first, then cite it. A finding you\n'
  printf 'judge non-actionable is `minor` and needs no task.\n\n'
  printf 'A blocking or major finding whose right outcome is **not** a task —\n'
  printf 'typically `kind: spec-wrong`, where the outcome is a decision — may be\n'
  printf 'recorded with no `task_id`. `record` stores it and prints its `id`;\n'
  printf 'the gate then refuses it until that id is argued in\n'
  printf '`baselines/review.txt` with a reason. ⚠️ Recording is not resolving.\n\n'
  printf 'An empty findings list is a valid outcome. Invented findings are worse\n'
  printf 'than none — but so is a clean verdict on a milestone nobody read.\n'
  ;;

# ---------------------------------------------------------------------------
record)
  require_python || finish
  [[ -n "$FILE" ]] || { fail "record needs --file <verdict.json>"; finish; }
  [[ -f "$FILE" ]] || { fail "no such file: $FILE"; finish; }
  mkdir -p "$REVIEW_DIR" || { fail "cannot create $REVIEW_DIR"; finish; }

  known="$(known_task_ids)"
  open_ids="$(open_task_ids)"

  rc=0
  python3 - "$FILE" "$MS" "$REVIEW_DIR" "$(printf '%s\n' "${ALL[@]}")" "$known" "$open_ids" <<'PYEOF' || rc=$?
import hashlib, json, subprocess, sys

src, ms, review_dir, all_commits, known, open_ids = sys.argv[1:7]
milestone_commits = set(all_commits.split())
known_tasks = set(known.split())
open_tasks = set(open_ids.split())
problems = []
unresolved = []   # blocking/major with no task_id: recorded, then refused by the gate

try:
    v = json.load(open(src))
except Exception as e:
    print(f"PROBLEM verdict is not valid JSON: {e}")
    raise SystemExit(2)
if not isinstance(v, dict):
    print("PROBLEM verdict must be a JSON object")
    raise SystemExit(2)

if v.get("milestone") != ms:
    problems.append(f"verdict says milestone {v.get('milestone')!r}, review was run for {ms!r}")

commits = v.get("commits") or []
if not isinstance(commits, list) or not commits:
    problems.append("commits is missing or empty — a review must name what it read")
    commits = []
# ⚠️ Checked against the real list. Otherwise a verdict could claim coverage of
# commits that do not exist, and the gate would report a milestone reviewed.
#
# An unambiguous abbreviation is resolved rather than refused. This tool's own
# human-facing output -- `coverage`, and the gate's `unreviewed` lines -- prints
# `%h`, so a reviewer working from what it showed them wrote the short form and
# got "which is not a commit in M-1", a statement that was false and pointed at
# the wrong problem. Ambiguity is still an error, and so is a prefix of nothing.
resolved = []
for c in commits:
    if c in milestone_commits:
        resolved.append(c)
        continue
    matches = [m for m in milestone_commits if m.startswith(c)]
    if len(matches) == 1 and len(c) >= 7:
        resolved.append(matches[0])
    elif len(matches) > 1:
        problems.append(f"commits names {c!r}, which is ambiguous — use the full 40-character SHA")
    else:
        problems.append(f"commits names {c!r}, which is not a commit in {ms}")
commits = resolved if not problems else commits
v["commits"] = commits

verdict = v.get("verdict")
if verdict not in ("pass", "changes-requested"):
    problems.append(f"verdict must be 'pass' or 'changes-requested', got {verdict!r}")

KINDS = ("drift", "contradiction", "abstraction", "standard-not-followed",
         "spec-wrong", "gate-wrong")
SEVERITIES = ("blocking", "major", "minor")
findings = v.get("findings", [])
if not isinstance(findings, list):
    problems.append("findings must be a list")
    findings = []

for i, f in enumerate(findings):
    where = f"findings[{i}]"
    if not isinstance(f, dict):
        problems.append(f"{where} must be an object")
        continue
    for key in ("summary", "evidence"):
        if not str(f.get(key, "")).strip():
            problems.append(f"{where}.{key} is empty")
    if f.get("kind") not in KINDS:
        problems.append(f"{where}.kind must be one of {'/'.join(KINDS)}, got {f.get('kind')!r}")
    if f.get("severity") not in SEVERITIES:
        problems.append(f"{where}.severity must be one of {'/'.join(SEVERITIES)}, "
                        f"got {f.get('severity')!r}")
    # This is the mechanism doc 21 §8 calls "findings become backlog tasks". A
    # finding that lives only in a review artifact is one nobody will act on --
    # not because the file is fragile (reviews/ is tracked) but because nothing
    # reads it. next-task reads the backlog, and so does everyone else.
    #
    # ⚠️ A missing task_id is **not** rejected here, and that asymmetry is
    # deliberate. The gate offers a second resolution -- argue the finding in
    # baselines/review.txt -- and arguing needs the finding's `id`, which only
    # exists once the verdict is recorded. Refusing to record was therefore
    # refusing the only path to the escape it documented, and it bit hardest on
    # `kind: spec-wrong`, where the skill says the outcome is a decision rather
    # than a task. So: record it, print the id, and let the gate demand a
    # resolution. Validation of shape here; enforcement of resolution there --
    # the same split check-reviewed.sh uses.
    if f.get("severity") in ("blocking", "major"):
        tid = str(f.get("task_id", "")).strip()
        if tid and known_tasks and tid not in known_tasks:
            problems.append(f"{where}.task_id {tid!r} is not a task the backlog lists")
        # ⚠️ Refused here, where the author can still pick another row -- not by
        # the gate, which would then flip red the moment the task was finished.
        elif tid and known_tasks and tid not in open_tasks:
            problems.append(
                f"{where}.task_id {tid!r} is a row the backlog marks done; a finding "
                f"needs one something will still act on, since next-task reads todo")
        elif not tid:
            unresolved.append(where)
    f["id"] = hashlib.sha256(
        (str(f.get("kind", "")) + "\0" + str(f.get("summary", ""))).encode()
    ).hexdigest()[:12]

unresolved_note = unresolved
actionable = [f for f in findings
              if isinstance(f, dict) and f.get("severity") in ("blocking", "major")]
if actionable and verdict == "pass":
    problems.append(f"verdict is 'pass' but {len(actionable)} finding(s) are blocking or major")
if verdict == "changes-requested" and not actionable:
    problems.append("verdict is 'changes-requested' but no finding is blocking or major")

if problems:
    for p in problems:
        print(f"PROBLEM {p}")
    raise SystemExit(2)

key = hashlib.sha256("\0".join(sorted(commits)).encode()).hexdigest()[:16]
dest = f"{review_dir}/milestone-{ms}-{key}.json"
v["recorded_at"] = subprocess.run(
    ["date", "-u", "+%Y-%m-%dT%H:%M:%SZ"], capture_output=True, text=True).stdout.strip()
v.setdefault("reviewer", "unknown")
with open(dest, "w") as fh:
    json.dump(v, fh, indent=2, sort_keys=True)
    fh.write("\n")

print(f"WROTE {dest}")
print(f"COVERS {len(commits)} commit(s)")
for f in findings:
    if isinstance(f, dict) and f.get("severity") in ("blocking", "major"):
        print(f"  {f['id']}  {f.get('severity')}  {f.get('kind')}  -> "
              f"{f.get('task_id') or 'NO TASK — the gate will refuse this'}")
if unresolved_note:
    print(f"UNRESOLVED {len(unresolved_note)}")
PYEOF

  if (( rc != 0 )); then
    fail "verdict rejected"
    finish
  fi
  ok "milestone review recorded for $MS"
  note "⚠️ stage it — an artifact that is not in the index does not count:"
  note "  git add $REVIEW_DIR"
  note "check-milestone-review.sh will now refuse any blocking or major finding"
  note "that names no backlog task and is not argued in baselines/review.txt"
  ;;

# ---------------------------------------------------------------------------
*)
  printf 'usage: scripts/milestone-review.sh {commits|coverage|context|record} [--milestone M-1] [--file F]\n' >&2
  exit 2
  ;;
esac

finish
