#!/usr/bin/env bash
# Which standards is this change judged against? `M-1.34`.
#
#   scripts/which-standards.sh              the staged change
#   scripts/which-standards.sh <path>...    those paths
#   scripts/which-standards.sh --why        also say which path matched
#
# Paths go to **stdout**, one per line, so this pipes. Everything else goes to
# stderr.
#
# ## Why this exists
#
# `AGENTS.md` lists eight standards with a "read when" line and trusts whoever
# is reading to pick the right ones. That is a hope, not a mechanism. This turns
# it into a lookup keyed on evidence — the paths actually being changed — which
# is something a script can do and a reader reliably will not.
#
# Its first consumer is `review.sh`: doc 21 §4 says the reviewer receives "the
# relevant standards", and without this that phrase has no referent. Handing a
# reviewer all eight dilutes attention across seven that do not apply, which is
# the failure mode §4 is trying to avoid.
#
# ## Where the mapping lives, and why not here
#
# In each standard's own front matter, as `applies_to:`. A table in this script
# would be a second place to edit, and the M-1.33 lesson was that two places
# holding the same fact is a promise to keep them in sync forever. A standard
# that does not say when it applies is a defect this script reports.
#
# ## Standards do not select themselves — decided, `M2.6` (closing `M1.41`)
#
# `M1.31`'s review noticed that the packet judging an edit *to* `security.md`
# did not include `security.md`: no standard's `applies_to` names its own
# path (`git.md`/`review.md` self-select only via their `["*"]`, and `sdd.md`
# names `docs/internal/standards/*`, which is why editing any standard
# selects `sdd.md`). ⚠️ **Self-selection is declined.** `applies_to` means
# "this standard governs files matching these globs", and a standard does
# not govern itself — adding its own path to every standard would trade that
# meaning for noise, and the reviewer already *has* the edited standard: it
# is in the diff, which is the packet's largest section. What was actually
# missing is an instruction, not a route, and `review.md` rule 5 now carries
# it: a diff that edits a standard obliges the reviewer to read that
# standard in full, edited and unedited parts alike.
#
# ## The glob dialect
#
# Shell patterns matched against repository-relative paths, where ⚠️ **`*`
# matches across `/`** — this is bash `[[ == ]]` pattern matching, not gitignore
# and not shell globbing. So `*.rs` matches `crates/a/src/b.rs`, and a crate is
# named `*oqueue-codec/*` rather than by a directory layout that does not exist
# yet.
#
# ⚠️ Consequence worth knowing: most patterns here name Rust that has not been
# written. They are live the moment a crate lands, and until then they are
# unexercised — see the report at the bottom of a run.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

STANDARDS_DIR="docs/internal/standards"

WHY=0
declare -a PATHS=()
for arg in "$@"; do
  case "$arg" in
    --why) WHY=1 ;;
    -h|--help)
      sed -n '2,6p' "${BASH_SOURCE[0]}" >&2
      exit 0
      ;;
    -*) printf 'unknown option: %s\n' "$arg" >&2; exit 2 ;;
    *) PATHS+=("$arg") ;;
  esac
done

# No paths named means the staged change, because that is what is about to be
# committed and therefore what is about to be reviewed.
if (( ${#PATHS[@]} == 0 )); then
  if ! git rev-parse --git-dir >/dev/null 2>&1; then
    printf 'not a git repository, and no paths given\n' >&2
    exit 2
  fi
  # ⚠️ No --diff-filter. A deletion is part of what is being reviewed, and
  # filtering deletions out made a deletion-only change report "nothing staged"
  # and route to no standards at all — including git.md, whose `*` is meant to
  # be unconditional. Deleting a test or a standard is precisely the commit
  # class non-negotiable 2 exists for.
  mapfile -t PATHS < <(git diff --cached --name-only)
  if (( ${#PATHS[@]} == 0 )); then
    printf 'nothing staged, so nothing to judge\n' >&2
    exit 0
  fi
fi

# applies_to -> one pattern per line.
#
# Read from front matter only: parsing stops at the closing `---`, so an
# `applies_to:` line inside a fenced example later in the document cannot be
# mistaken for metadata.
#
# Both YAML spellings are accepted, because rejecting the second produced a
# message naming a cause that was false — "has no applies_to" when the key was
# right there, which sends the author to re-add something already present:
#
#   applies_to: ["*.rs", "tests/*"]        inline flow, must close on one line
#   applies_to:                            block sequence
#     - "*.rs"
#     - "tests/*"
#
# Quoting is optional; single and double quotes are both stripped.
#
# ⚠️ **This parser is more permissive than build-index.sh's**, which reads only
# the flow form for its list keys — so a `tags:` written as a block sequence
# fails the index gate with a message about a missing family tag. The two
# readers disagreeing about what valid front matter is remains a wart; it is
# recorded in M-1.34's backlog note rather than fixed here, because
# build-index.sh belongs to another task.
#
# A flow list that does not close its `]` on the same line is rejected rather
# than truncated. ⚠️ Failing open here is the worse bug: it hands the reviewer a
# shorter standards list and says nothing.
patterns_of() {
  awk '
    function clean(s,   q) {
      q = sprintf("%c", 39)          # a literal single quote, without fighting
      gsub(/^[ \t]+|[ \t]+$/, "", s) # bash quoting inside this awk program
      gsub(/^"|"$/, "", s)
      sub("^" q, "", s); sub(q "$", "", s)
      return s
    }
    NR == 1 { if ($0 != "---") exit; next }
    /^---[ \t]*$/ { exit }
    /^applies_to:/ {
      rest = $0
      sub(/^applies_to:[ \t]*/, "", rest)
      if (rest ~ /^\[/) {
        if (rest !~ /\]/) { print "!MALFORMED"; exit }
        gsub(/^\[[ \t]*|[ \t]*\][ \t]*$/, "", rest)
        n = split(rest, a, ",")
        for (i = 1; i <= n; i++) { p = clean(a[i]); if (p != "") print p }
      } else {
        block = 1
      }
      next
    }
    block && /^[ \t]+-[ \t]*/ {
      rest = $0
      sub(/^[ \t]+-[ \t]*/, "", rest)
      p = clean(rest); if (p != "") print p
      next
    }
    block { block = 0 }
  ' "$1"
}

problems=0
matched_any=0
declare -a MATCHED=()

for std in "$STANDARDS_DIR"/*.md; do
  [[ -f "$std" ]] || continue

  mapfile -t pats < <(patterns_of "$std")

  if [[ " ${pats[*]-} " == *" !MALFORMED "* ]]; then
    printf 'PROBLEM %s has a malformed applies_to: a flow list must close its "]" on the same line\n' "$std" >&2
    problems=$((problems + 1))
    continue
  fi

  if (( ${#pats[@]} == 0 )); then
    # A standard nobody can route to is a standard nobody reads. This is the
    # same class of defect as a skill with no `description`.
    printf 'PROBLEM %s has no applies_to, so no change can ever be routed to it\n' "$std" >&2
    problems=$((problems + 1))
    continue
  fi

  declare -a hits=()
  for p in "${PATHS[@]}"; do
    for pat in "${pats[@]}"; do
      # shellcheck disable=SC2053  # the right-hand side is a pattern on purpose
      if [[ "$p" == $pat ]]; then
        hits+=("$p")
        break
      fi
    done
  done

  if (( ${#hits[@]} > 0 )); then
    matched_any=1
    MATCHED+=("$std")
    printf '%s\n' "$std"
    if (( WHY )); then
      printf '  matched %d of %d path(s), first: %s\n' \
        "${#hits[@]}" "${#PATHS[@]}" "${hits[0]}" >&2
    fi
  fi
done

if (( problems > 0 )); then
  printf '\n%d standard(s) cannot be routed to; see the PROBLEM line(s) above\n' "$problems" >&2
  exit 1
fi

if (( matched_any == 0 )); then
  # git.md matches "*", so reaching here means the standards are misconfigured.
  # Worth saying out loud rather than returning a silence that reads like "no
  # standards apply".
  printf 'no standard claims any of the %d path(s) given\n' "${#PATHS[@]}" >&2
fi

exit 0
