#!/usr/bin/env bash
# Does every argued baseline entry still argue a live survivor? `M4.60`.
#
#   scripts/check-mutants-baseline.sh <survivors-dir> <expected-shards>
#
# ## Why this is its own script
#
# `check-mutants.sh` asks two questions of a mutation run. The first —
# **is every survivor killed or argued?** — is answerable from one shard,
# because an unargued survivor in shard 3 is a failure whatever shards 1 and
# 2 found. The second — **does every argued entry still argue something?**,
# which `M4.32` added and which is the reason a stale suppression cannot
# hide — is answerable only from the *union*: a shard tests 1/k of the
# mutants, so every entry arguing a survivor in another shard has no match
# in this one.
#
# ⚠️ **That is not a detail of how the nightly is wired; it is why sharding
# is a task and not a flag.** `M4.51` moved the full pass to a nightly
# schedule and `M4.32` measured that pass at 2294 mutants over roughly six
# hours, past the bound the job declares — and rule 16's word for the answer
# is "sharded". Running the converse loop per shard would fail a correct
# baseline on seven runs out of eight.
#
# ## The count is the point
#
# ⚠️ **A missing shard is refused, not treated as an empty one.** The whole
# failure this script exists to avoid is judging the baseline against a
# partial union. A shard that crashed, timed out, or never uploaded its file
# leaves every entry arguing one of its survivors looking dead — the exact
# false report, arriving through absence rather than through logic. So the
# number of files is compared against what the caller says it launched, and
# a mismatch is a refusal to answer at all.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

BASELINE="baselines/mutants.txt"
dir="${1:-}"
expected="${2:-}"

if [[ -z "$dir" || -z "$expected" ]]; then
  fail "usage: scripts/check-mutants-baseline.sh <survivors-dir> <expected-shards>"
  finish
fi
if [[ ! "$expected" =~ ^[0-9]+$ ]] || (( expected < 1 )); then
  fail "expected-shards must be a positive integer, not '$expected'"
  finish
fi
if [[ ! -d "$dir" ]]; then
  fail "no survivors directory at $dir -- no shard reported"
  note "the union cannot be judged from nothing; this is not an empty union"
  finish
fi

shopt -s nullglob
files=("$dir"/*.txt)
shopt -u nullglob
if (( ${#files[@]} != expected )); then
  fail "${#files[@]} shard file(s) in $dir, expected $expected"
  for f in "${files[@]}"; do
    note "have: $(basename "$f")"
  done
  note "a missing shard makes every entry arguing one of its survivors look dead"
  note "-- which is the false report this check exists to avoid, through absence"
  finish
fi

survivors=()
for f in "${files[@]}"; do
  while IFS= read -r line; do
    [[ -n "$line" ]] && survivors+=("$line")
  done < "$f"
done

# The argued list, comments and blanks stripped -- the staged copy, matching
# `check-mutants.sh`'s own read so the two cannot disagree about which
# baseline they judged.
declare -a argued=()
if [[ -f "$BASELINE" ]]; then
  while IFS= read -r line; do
    [[ -z "$line" || "$line" == \#* ]] && continue
    argued+=("$line")
  done < <(git show ":$BASELINE" 2>/dev/null || cat "$BASELINE")
fi

stale=0
for a in "${argued[@]}"; do
  loc="${a%%  *}"
  reason="${a#*  }"
  matched=0
  for s in "${survivors[@]}"; do
    [[ "$s" == "$loc"* ]] && { matched=1; break; }
  done
  # ⚠️ **`unviable:` is the exemption**, for the reason `check-mutants.sh`'s
  # own copy of this loop gives: a mutant can stop being *generated* as well
  # as stop surviving, and `cargo mutants` never lists one that no longer
  # compiles, which is indistinguishable from one that was killed.
  if [[ "${reason#"${reason%%[![:space:]]*}"}" == unviable:* ]]; then
    if (( matched )); then
      fail "baseline entry claims 'unviable:' for a mutant that survived: $loc"
      stale=$((stale + 1))
    fi
    continue
  fi
  if (( matched == 0 )); then
    fail "baseline entry argues no surviving mutant: $loc"
    stale=$((stale + 1))
  fi
done

if (( stale > 0 )); then
  note "re-key it against the run, or delete it -- testing.md rule 17"
  note "⚠️ cargo mutants --list re-derives a key in seconds, without running a test"
  finish
fi

ok "every argued baseline entry still argues a live survivor (${#argued[@]} entr(y|ies), ${#survivors[@]} survivor(s) across $expected shard(s))"
finish
