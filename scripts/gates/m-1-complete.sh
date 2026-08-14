#!/usr/bin/env bash
# M-1's own completion condition. `M-1.16`.
#
#   scripts/gates/m-1-complete.sh
#
# ## What "complete" means, and why this is not a summary of the backlog
#
# `docs/internal/product/milestones/M-1.md` states the condition this script
# is: every non-negotiable in `AGENTS.md` names a script that exists and
# passes (except rule 3, which cannot be enforced by a script and says so),
# `tests/gates/negative.sh` proves each of those gates can actually fail, and
# `check-milestone-review.sh` confirms every commit in the milestone has been
# read as a whole, not just per-commit. It does **not** check that every
# backlog row is `done` — `docs/internal/product/backlog.md` is the
# authoritative task list and this script is not a second copy of it. A
# milestone can satisfy this condition while backlog rows remain open if
# those rows do not bear on any non-negotiable; whether that is actually true
# right now is a judgement call for whoever reads this script's output next
# to the backlog, not something a gate can decide.
#
# ## Why AGENTS.md is parsed rather than hand-listed
#
# A hard-coded `rule 1 -> check-commit-msg.sh` table here would be a second
# place non-negotiable 1's own script name lives, and the two would drift the
# first time either changed without the other -- the same two-places-one-fact
# hazard `build-index.sh`'s header already names for the standards and skills
# tables. Reading AGENTS.md's own "## Non-negotiables" section and following
# whatever `scripts/check-*.sh` names it cites keeps this script and the
# document it is checking unable to disagree about which script enforces
# which rule.
#
# ## What this cannot check
#
# The same limit every gate here already accepts for rule 3: that a script
# which exists and exits 0 is actually testing the right thing. This asserts
# structure -- a script is named, it runs, it passes -- not correctness of
# what it checks. That is what review is for.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

require_python || finish

AGENTS_FILE="AGENTS.md"
if [[ ! -f "$AGENTS_FILE" ]]; then
  fail "AGENTS.md not found at the repository root"
  finish
fi

# Parses AGENTS.md's "## Non-negotiables" section into one `RULE <n> <script>`
# line per script a rule names, `RULE <n> NONE` for a rule that names none and
# is explicitly marked as the permanent exception, or `PROBLEM <why>` for
# anything that does not fit that shape. Python, not grep: a rule's text can
# wrap across several lines before the next numbered item, and finding "the
# next numbered rule" reliably needs the same multi-line view build-index.sh
# already uses Python for, rather than bash reimplementing a small parser.
rc=0
parsed="$(python3 - "$AGENTS_FILE" 2>&1 <<'PYEOF'
import re, sys

def build():
    path = sys.argv[1]
    text = open(path, encoding="utf-8").read()

    m = re.search(r'^## Non-negotiables\n(.*?)(?=^## )', text, re.S | re.M)
    if not m:
        print("PROBLEM AGENTS.md has no ## Non-negotiables section")
        sys.exit(2)
    section = m.group(1)

    starts = list(re.finditer(r'^(\d+)\.\s', section, re.M))
    if not starts:
        print("PROBLEM no numbered rule found under ## Non-negotiables")
        sys.exit(2)

    problems = []
    seen_numbers = []
    for i, s in enumerate(starts):
        num = int(s.group(1))
        seen_numbers.append(num)
        end = starts[i + 1].start() if i + 1 < len(starts) else len(section)
        body = section[s.start():end]

        scripts = sorted(set(re.findall(r'`(scripts/check-[A-Za-z0-9_.-]+\.sh)`', body)))
        if scripts:
            for sc in scripts:
                print(f"RULE {num} {sc}")
            continue

        # A rule naming no check-*.sh script must say, in its own text, that
        # no script can enforce it -- the wording AGENTS.md's rule 3 already
        # uses. Anything else naming no script is the state this file's own
        # banner calls a preference, and the completion condition exists to
        # refuse exactly that. Whitespace is collapsed before the search: the
        # phrase itself wraps across a line break in the source (Markdown
        # reflows prose at ~80 columns), so a literal substring search against
        # the raw text missed it and misreported rule 3 itself as unmarked.
        normalized = " ".join(body.split())
        if "no script enforces this" in normalized.lower():
            print(f"RULE {num} NONE")
        else:
            problems.append(
                f"rule {num} names no scripts/check-*.sh script and is not "
                "marked as the permanent exception"
            )

    if sorted(seen_numbers) != list(range(1, len(seen_numbers) + 1)):
        problems.append(
            f"non-negotiable numbering is not a contiguous 1..N sequence: {seen_numbers}"
        )

    for p in problems:
        print(f"PROBLEM {p}")
    sys.exit(2 if problems else 0)

try:
    build()
except SystemExit:
    raise                      # the deliberate verdict above, not a crash
except Exception as exc:
    import traceback
    traceback.print_exc()
    print(f"PROBLEM the AGENTS.md parser raised {type(exc).__name__}: {exc}")
    sys.exit(3)
PYEOF
)" || rc=$?

if (( rc != 0 )); then
  # ⚠️ Fails closed, not open. Any exit this parser did not choose on purpose
  # (a traceback aside) means AGENTS.md was not actually read, and the failure
  # below says so rather than silently skipping to "no rules, nothing to run".
  fail "AGENTS.md's non-negotiables could not be read (exit $rc)"
  note "captured output:"
  sed 's/^/     /' <<< "$parsed" >&2
  finish
fi

declare -A RAN=()
non_negotiables_ok=1

while IFS= read -r line; do
  [[ -n "$line" ]] || continue
  case "$line" in
    "PROBLEM "*)
      fail "${line#PROBLEM }"
      non_negotiables_ok=0
      ;;
    "RULE "*)
      rest="${line#RULE }"
      num="${rest%% *}"
      script="${rest#* }"
      if [[ "$script" == "NONE" ]]; then
        note "rule $num: no script, and marked as the permanent exception"
        continue
      fi
      # Rule 2 and rule 4 each name more than one script; run each exactly
      # once even though it is cited by only one rule.
      [[ -n "${RAN[$script]:-}" ]] && continue
      RAN["$script"]=1
      if [[ ! -f "$REPO_ROOT/$script" ]]; then
        fail "rule $num names $script, which does not exist"
        non_negotiables_ok=0
        continue
      fi
      grc=0
      # `|| grc=$?`, not a bare call: this line's whole point is to observe a
      # possibly-nonzero exit code, not propagate it -- the same discipline
      # tests/gates/negative.sh's own run_case() needed (M-1.15).
      "$REPO_ROOT/$script" >/tmp/m1-complete-gate.$$ 2>&1 || grc=$?
      if (( grc == 0 )); then
        ok "rule $num: $script passes"
      else
        fail "rule $num: $script fails (exit $grc)"
        note "captured output:"
        sed 's/^/     /' /tmp/m1-complete-gate.$$ >&2
        non_negotiables_ok=0
      fi
      rm -f /tmp/m1-complete-gate.$$
      ;;
  esac
done <<< "$parsed"

if (( non_negotiables_ok == 0 )); then
  finish
fi

# tests/gates/negative.sh: every gate named above has been watched to fail on
# a broken artifact, not just to pass on this one.
nrc=0
bash "$REPO_ROOT/tests/gates/negative.sh" >/tmp/m1-complete-negative.$$ 2>&1 || nrc=$?
if (( nrc == 0 )); then
  ok "tests/gates/negative.sh: every non-negotiable's gate fails on a broken artifact"
else
  fail "tests/gates/negative.sh failed (exit $nrc)"
  note "captured output:"
  sed 's/^/     /' /tmp/m1-complete-negative.$$ >&2
fi
rm -f /tmp/m1-complete-negative.$$

# check-milestone-review.sh: the outer loop, not just the inner one -- every
# M-1 commit has been read as a whole, not only individually. `M-1.37`.
mrc=0
"$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M-1 \
  >/tmp/m1-complete-review.$$ 2>&1 || mrc=$?
if (( mrc == 0 )); then
  ok "check-milestone-review.sh: every M-1 commit is covered by a milestone review"
else
  fail "check-milestone-review.sh failed (exit $mrc)"
  note "captured output:"
  sed 's/^/     /' /tmp/m1-complete-review.$$ >&2
fi
rm -f /tmp/m1-complete-review.$$

finish
