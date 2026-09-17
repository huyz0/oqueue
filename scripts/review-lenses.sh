#!/usr/bin/env bash
# What is this reviewer being asked to look at? `M5.52`.
#
#   scripts/review-lenses.sh              the staged change
#   scripts/review-lenses.sh <path>...    those paths
#
# Lenses go to **stdout**; everything else to stderr.
#
# ## Why this exists
#
# `review.md` rule 12a says what a change is *judged against* is derived from
# its paths (`which-standards.sh`). What the reviewer is asked to *look at* was
# whatever the spawning prompt happened to say that turn — which differs per
# task, is written by the author, and dies with the session. A brief improvised
# per review is how attention goes to whatever is most recently on someone's
# mind rather than to where this repository's defects have actually been.
#
# ⚠️ **This routes attention and never enforcement.** Every deterministic gate
# runs on every commit whatever this prints; a lens cannot skip one, and a path
# matching no lens gets no lens rather than all of them. What is expensive is
# the agent's reading, and this decides what that reading is spent on.
#
# ## Where the mapping lives, and why here rather than beside each thing
#
# ⚠️ **Here, and this is the opposite call from `which-standards.sh`'s**, which
# keeps its mapping in each standard's own `applies_to` front matter to avoid a
# second place to edit. A lens is not a property of a file the way a standard's
# scope is: it is a property of *this repository's review history* — the classes
# of defect that got through here, with the instances named. There is no other
# place holding that, so there is nothing for this to drift from. Each lens
# below cites the tasks that earned it, and a lens nobody can cite an instance
# for does not belong here.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if (( $# > 0 )); then
  PATHS=$(printf '%s\n' "$@")
else
  # ⚠️ `ACMRD`, with `D`: a commit that deletes a test is exactly what
  # non-negotiable 2 is about, and a deletion-only change would otherwise
  # select no lens at all.
  PATHS=$(git diff --cached --name-only --diff-filter=ACMRD 2>/dev/null)
fi

if [[ -z "$PATHS" ]]; then
  printf 'no paths, so no lenses\n' >&2
  exit 0
fi

SEEN=""

# Prints a lens once, however many paths select it.
lens() {
  local name="$1" body="$2"
  case "$SEEN" in *"[$name]"*) return ;; esac
  SEEN="$SEEN[$name]"
  printf '\n### Lens: %s\n\n%s\n' "$name" "$body"
}

# True when any changed path matches the pattern. ⚠️ bash `[[ == ]]` pattern
# matching, the same dialect `which-standards.sh` documents: `*` crosses `/`.
matches() {
  local pattern="$1" path
  while IFS= read -r path; do
    [[ -n "$path" ]] || continue
    # shellcheck disable=SC2053
    [[ "$path" == $pattern ]] && return 0
  done <<< "$PATHS"
  return 1
}

if matches 'scripts/*' || matches 'tests/gates/*' || matches 'tests/harness/*' \
   || matches '.pre-commit-config.yaml' || matches '.githooks/*' \
   || matches '.github/workflows/*'; then
  lens "a gate that reports success while checking nothing" \
"This change touches enforcement, where every defect found here so far looked
green. \`M5.38\`'s leg matched \`ObjectStore\` with a word boundary and so never
saw \`FakeObjectStore\`; \`M5.39\` replaced that boundary with a consuming
character class and stopped matching a line that starts with the name;
\`M10.24\` found a hand-written hook running 12 of the config's 17 gates while
the tree claimed the suite; \`M0.17\` found a packet asserting that gates had
passed without running them.

- Construct the input the check must REJECT, run it, and confirm it is rejected.
- Confirm the check is wired into something that runs it. A gate nothing invokes
  is a preference.
- Confirm it cannot pass vacuously: no matching files, an absent report, a tool
  that is not installed, an empty input set.
- Confirm the exit status propagates, and that a \`FAIL\` line is not printed
  beside an exit of 0."
fi

# ⚠️ **Named files, not `*commit*`.** That pattern was here and matched eight
# tracked paths with nothing to do with this lens — `.pre-commit-config.yaml`,
# `check-commit-msg.sh`, the OffsetCommit codec — so a reviewer of a wire-format
# change was told to check that a retry cannot overwrite bytes an index entry
# names, about a path that has no such bytes. A lens pointed at the wrong change
# costs more than no lens: it is the reviewer's whole attention, spent on the
# author's behalf, somewhere the defect is not.
if matches '*oqueue-compact/*' || matches '*oqueue-core/src/bundle*' \
   || matches '*oqueue-core/src/index_state*'; then
  lens "acknowledged data, lost silently" \
"This is the write path, and \`AGENTS.md\` names it the one operation that can
lose acknowledged data without anything failing (NFR-20). The instances are all
in this milestone: \`M5.4\` shipped five such paths in one draft — caller-ordered
inputs, no offset range, an unconditional PUT, discarded spans, and gaps
accepted in silence; \`M5.5\` wrote one range twice in a round with no error;
\`M5.46\` produced plans no set of objects tiles.

- For each way the inputs could be wrong — out of order, missing one, naming one
  twice, overlapping, short — say what the code does. \"Refused\" is an answer;
  \"produces a valid object with the records absent\" is the finding.
- Check the count that is compared against what. A record count nobody compares
  is a number, not a check.
- Check that a retry cannot overwrite bytes an index entry already names."
fi

if matches '*oqueue-core/src/*'; then
  lens "a seam whose fakes no longer say what the backends do" \
"\`AGENTS.md\` non-negotiable 6: a trait change moves the trait, every fake,
every implementation and an ADR in one commit. The failure this catches is
narrower and quieter — a fake and a backend that both compile and disagree.
\`M5.6\` sealed on \"any part written\" in the fake and on bytes in S3 and GCS,
so every test passed and the backends refused an object the fake accepted.

- For each behaviour the trait documents, check the fake and the real
  implementations agree on it, including the error cases.
- Check the fake is not more permissive. A fake that accepts what a backend
  refuses makes the suite green and the deployment red."
fi

if matches '*.rs'; then
  lens "a test that passes without constraining anything" \
"\`testing.md\` rule 15 calls this the characteristic failure of generated
tests, and the mutants gate is what usually finds it here rather than review.

- For each new test, name the mutation of the production code that would survive
  it. If you cannot, say so — that is the finding.
- Check an assertion that would hold for any input (a length, a \`is_ok()\`, a
  count the fixture chose) against one that pins the behaviour.
- Check a threshold or constant was not moved in the direction that weakens its
  gate (non-negotiable 2)."
fi

if matches 'docs/*' || matches '*.md' || matches 'baselines/*'; then
  lens "a sentence that will be false before anyone re-reads it" \
"Prose here is load-bearing and this repository's own history is the argument:
\`AGENTS.md\` records a hardcoded list of outstanding work going stale four
separate times before the fix turned out to be deleting it, \`M5.43\` items (6)
and (12) each corrected a count in one paragraph and left the next one wrong,
and \`M5.50\` spent five review rounds on one such paragraph.

- Check every count, list and \"every X does Y\" claim against the tree, now.
- Prefer a sentence that names where the answer lives to one that copies the
  answer. A copy is a promise to keep two places in sync forever.
- A claim about what a file or a gate does is checkable: check it rather than
  reading it as intent.
- ⚠️ A prose defect is a \`minor\` unless it is false and something acts on it.
  \`review.md\` rule 15: a minor is recorded, not re-reviewed, and opening a
  round for one is how a documentation commit costs five."
fi

if [[ -z "$SEEN" ]]; then
  printf 'No lens matched these paths. That is an answer, not a gap: review the\n'
  printf 'change against its task and the standards above.\n'
fi
