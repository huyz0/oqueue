#!/usr/bin/env bash
# A benchmark's hot-path marker names a real row in performance.md's table.
# `M-1.29`, `performance.md` rules 18-19.
#
#   scripts/check-hot-path-bench.sh
#
# ## ⚠️ A deliberate narrowing of the literal acceptance criterion
#
# `M-1.29`'s backlog row says "every hot path named in performance.md rule 18
# has a benchmark" — read literally, all eight, always. The eight rows
# belong to code that does not all land at once: `RecordBatch encode/decode`,
# CRC-32C, and Varint decode are M2's; Compaction throughput is M5's (see
# each milestone's own plan). A gate enforcing "all eight, every commit"
# from the moment any crate exists would stay red from M0's first commit
# until M5 ships, failing every commit on every unrelated task in between —
# not what rule 19 ("so the list above cannot silently rot") is asking for,
# read against what "rot" means: the table becoming *inaccurate*, not the
# table being *incomplete* while the code it describes has not been written.
#
# What this checks instead, and can check honestly at every point in
# history:
#
#   1. **Hard failure.** A `// hot-path: <name>` marker (in a tracked `.rs`
#      file) naming something rule 18's table does not list — a typo, or a
#      benchmark whose comment was never updated after the table changed.
#      This direction has no "too early" state; a marker either matches a
#      real row or it does not, regardless of how much of the table is
#      covered yet.
#   2. **Per-row, via `NOT_YET_BUILT` below.** A row with no marker is a
#      failure UNLESS it is named in that allowlist, the same shape
#      `check-file-size.sh` uses for its own exceptions: an array entry
#      with a reason, in this file, so adding to it is a diff someone
#      reviews. Every row starts allowlisted, because no crate exists yet.
#      The task that adds a path's benchmark removes that path's entry in
#      the same commit — after which losing the marker again (a rename, a
#      deleted benchmark) is a real hard failure for that row alone, while
#      rows still genuinely unbuilt stay allowlisted. This is what answers
#      rule 19's "cannot silently rot" without punishing code that has not
#      been written yet: coverage is enforced per row from the moment each
#      row is actually built, not deferred as a single all-eight switch.
#
# ⚠️ **This is a narrower reading than the acceptance criterion states, made
# explicitly rather than silently** — flag it in review or the next
# milestone-review checkpoint if the intent really was a hard, all-eight
# gate from day one. The mechanism above is what makes the narrowing
# temporary and self-correcting rather than a standing exception: nothing
# needs to change in this script's logic as milestones land, only entries
# removed from `NOT_YET_BUILT`.
#
# ## The marker convention
#
# No benchmark exists yet, so this task is also the first place the marker
# shape is made concrete: a comment line `// hot-path: <exact table text>`
# immediately associated with the benchmark function that covers it,
# anywhere under a tracked `.rs` file. The name must match rule 18's "Path"
# column **exactly**, including the ` / ` spacing — a looser match (fuzzy,
# case-insensitive) would let a marker drift from the table's own wording
# silently, the opposite of this gate's purpose.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "hot-path benchmarks (no Cargo.toml yet)"
  finish
fi

# Rows with no benchmark yet that are not, currently, a failure -- each
# entry's reason names what has to exist first, checked against this
# repository's milestone plans rather than guessed. Delete a row's entry in
# the same commit that adds its `hot-path:` marker; do not add an entry for
# a row that already has one.
declare -A NOT_YET_BUILT=(
  ["RecordBatch encode / decode"]="M2 has not landed (docs/internal/product/milestones/M2.md)"
  ["CRC-32C over representative sizes"]="M2 has not landed (docs/internal/product/milestones/M2.md)"
  ["Varint decode (and the paths that avoid it)"]="M2 has not landed (docs/internal/product/milestones/M2.md)"
  ["Offset→object index lookup"]="M3 has not landed (docs/internal/product/milestones/M3.md)"
  ["Buffer allocation and pooling"]="no crate exists to allocate or pool a buffer in yet"
  ["Produce path end to end"]="no produce path exists yet"
  ["Fetch: tail (cached) and cold (ranged GET)"]="no fetch path exists yet"
  ["Compaction throughput"]="no compaction exists yet"
)

PERFORMANCE_STD="docs/internal/standards/performance.md"
if [[ ! -f "$PERFORMANCE_STD" ]]; then
  fail "$PERFORMANCE_STD not found; hot-path markers cannot be checked against it"
  finish
fi

# rule 18's table: the section from "## Hot-path benchmarks" to the next
# "## " heading, table rows only -- the same `| X | Y |`-shape guard
# `check-requirements-trace.sh` uses, for the same reason: a bare `cut
# -d'|'` on a line with no `|` at all prints that whole line unchanged
# (GNU cut's documented default), and this section's own prose (rule 19)
# has no pipes but plenty of words a looser filter would misparse as a row.
section="$(sed -n '/^## Hot-path benchmarks$/,/^## /p' "$PERFORMANCE_STD" || true)"
declare -A TABLE_PATHS=()
while IFS= read -r row; do
  # The table lives inside numbered-list item 18, so every real row is
  # indented (markdown list continuation) rather than starting at column 0 --
  # confirmed by reading the actual file rather than assuming GFM's usual
  # column-0 table shape. Leading whitespace is allowed and stripped by `cut`
  # matching on `|`, which does not care about what precedes the first `|`.
  [[ "$row" =~ ^[[:space:]]*\|[^\|]+\|[^\|]+\|[[:space:]]*$ ]] || continue
  name="$(cut -d'|' -f2 <<< "$row" | sed -E 's/^[[:space:]]+|[[:space:]]+$//g')"
  [[ "$name" == "Path" || "$name" =~ ^-+$ ]] && continue
  [[ -n "$name" ]] || continue
  TABLE_PATHS["$name"]=1
done <<< "$section"

if (( ${#TABLE_PATHS[@]} == 0 )); then
  fail "$PERFORMANCE_STD's Hot-path benchmarks table lists no paths"
  finish
fi

# Every `// hot-path: <name>` marker in a tracked .rs file. ⚠️ `|| true`:
# no marker exists yet in the bootstrap state this gate was written in, and
# an empty grep match must not kill the script under `set -e`.
mapfile -t marker_lines < <(git grep -nE '// *hot-path: *' -- '*.rs' 2>/dev/null || true)

# ⚠️ All three checks below run unconditionally, and `problems` accumulates
# across every one of them, rather than each ending in its own `finish` --
# the same "accumulate, then one terminal report" shape `check-file-size.sh`,
# `check-requirements-trace.sh`, `check-readmes.sh`, and `check-portability.sh`
# already use, and the one `lib.sh`'s own `fail()` docstring asks for ("the
# caller keeps going so one run reports every violation rather than only the
# first"). An earlier version of this script called `finish` after each
# check, so a commit with two kinds of defect at once only ever saw the
# first -- found by review, reproduced directly (a marker naming an unknown
# path *and* a stale `NOT_YET_BUILT` entry in one tree reported only the
# unknown-marker failure until that was fixed and the gate re-run).
problems=0

declare -A COVERED=()
for line in "${marker_lines[@]}"; do
  [[ -n "$line" ]] || continue
  name="$(sed -E 's/^[^:]+:[0-9]+:.*hot-path: *//' <<< "$line" | sed -E 's/[[:space:]]+$//')"
  if [[ -z "${TABLE_PATHS[$name]:-}" ]]; then
    fail "$line"
    note "hot-path marker names '$name', which $PERFORMANCE_STD's table does not list"
    problems=$((problems + 1))
  else
    COVERED["$name"]=1
  fi
done

# Every row in `NOT_YET_BUILT` is also checked against the table itself --
# an entry naming a path rule 18 no longer lists is exactly the same drift
# this whole gate exists to catch, just on the allowlist side rather than a
# marker. ⚠️ And a row that now has a real marker (`COVERED`) but still has
# an entry is the loophole that made the per-row guarantee this script's own
# header promises false: without this check, a covered row keeps its "not a
# failure" protection forever, so a later regression -- the benchmark
# deleted again -- stayed silent instead of becoming the individual hard
# failure the header claims. Found by review reproducing exactly that
# sequence against a scratch fixture, not by inspection.
for name in "${!NOT_YET_BUILT[@]}"; do
  if [[ -z "${TABLE_PATHS[$name]:-}" ]]; then
    fail "NOT_YET_BUILT names '$name', which $PERFORMANCE_STD's table does not list"
    problems=$((problems + 1))
  elif [[ -n "${COVERED[$name]:-}" ]]; then
    fail "NOT_YET_BUILT still lists '$name', which now has a hot-path: marker"
    note "remove this entry -- the row is covered, so it no longer needs the allowlist"
    problems=$((problems + 1))
  fi
done

uncovered=0
for name in "${!TABLE_PATHS[@]}"; do
  [[ -n "${COVERED[$name]:-}" ]] && continue
  if [[ -n "${NOT_YET_BUILT[$name]:-}" ]]; then
    uncovered=$((uncovered + 1))
  else
    fail "hot path '$name' has no benchmark and is not in NOT_YET_BUILT"
    note "add a hot-path: marker for it, or, if it genuinely isn't built yet, add it to NOT_YET_BUILT with a reason"
    problems=$((problems + 1))
  fi
done

if (( problems == 0 )); then
  ok "hot-path markers (${#marker_lines[@]} found, all name a real row)"
  if (( uncovered > 0 )); then
    note "$uncovered of ${#TABLE_PATHS[@]} hot path(s) in rule 18's table have no benchmark yet, allowlisted in NOT_YET_BUILT"
  fi
fi
finish
