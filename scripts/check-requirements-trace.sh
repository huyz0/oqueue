#!/usr/bin/env bash
# Every milestone plan names the requirements it serves, and every ID it
# names is real. `M-1.24`.
#
#   scripts/check-requirements-trace.sh
#
# ## What this makes structural
#
# `docs/internal/product/roadmap.md` links each milestone to its plan under
# `docs/internal/product/milestones/`, and every plan written so far (M-1.41
# through M-1.43) already carries a `**Serves:** FR-N, NFR-N` line naming
# what it serves. That line is this project's one place to *author* the
# fact — but `roadmap.md`'s own "Requirement coverage" table already
# restates it, one row per milestone, as an at-a-glance summary the plan
# files do not offer on their own. Two places holding one fact is exactly
# the hazard `build-index.sh`'s header names for the standards and skills
# tables, so this gate does not treat the roadmap table as a second
# authored source — it is checked *against* the plans, never trusted on its
# own.
#
# Three failures, all real:
#
#   1. a milestone plan with no `**Serves:**` line, or one naming no FR/NFR
#      ID — a milestone that serves nothing has no reason to exist, and one
#      that forgot to say what it serves is indistinguishable from that
#   2. a `**Serves:**` line citing an ID `requirements.md` does not list —
#      a typo or a stale citation after a requirement was renumbered, which
#      reads as traceable and is not
#   3. `roadmap.md`'s "Requirement coverage" table disagreeing with a plan's
#      `**Serves:**` line — either a different set of ids for a milestone
#      both name, a plan with no row in the table, or a table row naming a
#      milestone with no plan file. Found by review: this table already
#      existed, already forward-referenced this gate by name in its own
#      prose, and nothing checked it against the thing it summarizes.
#
# ## What this cannot check
#
# That the citation is *right* — that a milestone naming NFR-50 actually
# advances NFR-50 rather than merely mentioning it. That is a judgement call
# for whoever reads the plan next to the requirement, the same limit
# `check-commit-msg.sh` already accepts for whether a commit's subject
# describes what it actually did.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

MILESTONES_DIR="docs/internal/product/milestones"
REQUIREMENTS_FILE="docs/internal/product/requirements.md"
ROADMAP_FILE="docs/internal/product/roadmap.md"

if [[ ! -d "$MILESTONES_DIR" ]]; then
  skip "requirements trace (no $MILESTONES_DIR yet)"
  finish
fi

if [[ ! -f "$REQUIREMENTS_FILE" ]]; then
  fail "$REQUIREMENTS_FILE not found; milestone citations cannot be checked against it"
  finish
fi

if [[ ! -f "$ROADMAP_FILE" ]]; then
  fail "$ROADMAP_FILE not found; the Requirement coverage table cannot be checked against the plans"
  finish
fi

# Every FR-N / NFR-N id requirements.md actually lists, so a plan citing one
# that does not exist is caught the same way a review finding citing an
# unknown backlog task is. ⚠️ `|| true`: a requirements file with no table
# rows yet would otherwise make `grep` exit 1 under `set -e` and kill this
# script before the "no requirements listed" case below can report it —
# the same empty-match hazard lib.sh's own known_task_ids() already names.
known_ids="$(grep -oE '\| (FR|NFR)-[0-9]+ \|' "$REQUIREMENTS_FILE" \
  | tr -d '|' | tr -d ' ' | sort -u || true)"

if [[ -z "$known_ids" ]]; then
  fail "$REQUIREMENTS_FILE lists no FR/NFR ids; nothing for a milestone to cite"
  finish
fi

mapfile -t plans < <(find "$MILESTONES_DIR" -maxdepth 1 -name 'M*.md' -type f | sort)

if (( ${#plans[@]} == 0 )); then
  skip "requirements trace (no milestone plan exists yet)"
  finish
fi

# roadmap.md's "## Requirement coverage" table, read into one associative
# array per milestone id: its "Serves" cell's ids, sorted and space-joined.
# Table rows only — the header row ("Milestone") and separator ("---") are
# excluded by requiring the second column to hold at least one FR/NFR id.
declare -A ROADMAP_IDS=()
section="$(sed -n '/^## Requirement coverage$/,/^## /p' "$ROADMAP_FILE" || true)"
while IFS= read -r row; do
  # ⚠️ Only lines shaped like `| X | Y |` — exactly two columns — are table
  # rows. Without this, `cut -d'|' -fN` on a prose line with no `|` at all
  # returns that whole line unchanged (GNU cut's documented default, not a
  # bug in cut), and this section's own prose contains a real "FR-15" — the
  # deferred-requirements paragraph a few lines below the table — which a
  # looser filter picked up as a bogus row and then failed as an orphan
  # naming a milestone that does not exist. Found before this ever reached
  # review, by watching this loop misparse the real roadmap.md.
  [[ "$row" =~ ^\|[^\|]+\|[^\|]+\|$ ]] || continue
  mid="$(cut -d'|' -f2 <<< "$row" | tr -d '[:space:]')"
  [[ "$mid" == "Milestone" || "$mid" =~ ^-+$ ]] && continue
  cell="$(cut -d'|' -f3 <<< "$row")"
  cell_ids="$(grep -oE '(FR|NFR)-[0-9]+' <<< "$cell" | sort -u | tr '\n' ' ' || true)"
  [[ -n "$cell_ids" ]] || continue
  ROADMAP_IDS["$mid"]="${cell_ids% }"
done <<< "$section"

untraced=0
unknown=0
mismatched=0
declare -A ROADMAP_MATCHED=()
for plan in "${plans[@]}"; do
  mid="$(basename "$plan" .md)"

  # ⚠️ `|| true`, not a bare call: a plan with no **Serves:** line at all is
  # the failure this loop exists to catch, not a reason for grep's exit 1 to
  # kill the script under `set -e` before that failure can be reported.
  serves_line="$(grep -m1 -E '^\*\*Serves:\*\*' "$plan" || true)"

  if [[ -z "$serves_line" ]]; then
    fail "$plan has no **Serves:** line naming what it serves"
    untraced=$((untraced + 1))
    continue
  fi

  ids="$(grep -oE '(FR|NFR)-[0-9]+' <<< "$serves_line" | sort -u | tr '\n' ' ' || true)"
  ids="${ids% }"

  if [[ -z "$ids" ]]; then
    fail "$plan's **Serves:** line names no FR/NFR requirement"
    note "$serves_line"
    untraced=$((untraced + 1))
    continue
  fi

  for id in $ids; do
    if ! grep -qxF "$id" <<< "$known_ids"; then
      fail "$plan cites $id, which $REQUIREMENTS_FILE does not list"
      unknown=$((unknown + 1))
    fi
  done

  roadmap_ids="${ROADMAP_IDS[$mid]:-}"
  if [[ -z "$roadmap_ids" ]]; then
    fail "$plan has no row in $ROADMAP_FILE's Requirement coverage table"
    mismatched=$((mismatched + 1))
  elif [[ "$roadmap_ids" != "$ids" ]]; then
    fail "$mid: $plan says '$ids', but $ROADMAP_FILE's Requirement coverage table says '$roadmap_ids' -- they have drifted"
    mismatched=$((mismatched + 1))
  else
    ROADMAP_MATCHED["$mid"]=1
  fi
done

# A table row naming a milestone with no plan file: the same drift, the
# other direction.
for mid in "${!ROADMAP_IDS[@]}"; do
  if [[ -z "${ROADMAP_MATCHED[$mid]:-}" && ! -f "$MILESTONES_DIR/$mid.md" ]]; then
    fail "$ROADMAP_FILE's Requirement coverage table names $mid, which has no $MILESTONES_DIR/$mid.md"
    mismatched=$((mismatched + 1))
  fi
done

if (( untraced > 0 || unknown > 0 || mismatched > 0 )); then
  finish
fi

ok "${#plans[@]} milestone plan(s) each name a requirement they serve, every id exists, and roadmap.md's Requirement coverage table agrees"
finish
