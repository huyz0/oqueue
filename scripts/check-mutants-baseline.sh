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

# The argued list, comments and blanks stripped -- the staged copy. ⚠️ **The
# two reads are now the same question asked the same way**, which is what this
# comment claimed while they differed twice over: this one fell back to the
# worktree and that one guarded on `[[ -f ]]`. `M4.69` removed both.
#
# ⚠️ **The index only, and `M4.69` is why that sentence is now true.** It had a
# `|| cat "$BASELINE"` fallback while `check-mutants.sh` read the index alone,
# so the comment claiming the two reads matched was false in the direction that
# matters: an *unstaged* baseline line argued away a survivor here and was
# ignored there. `lib.sh`'s own note gives the rule — a line that counts while
# unstaged rewards the path that leaves no trace in history. ⚠️ Both copies also
# guarded the read on `[[ -f "$BASELINE" ]]`, a worktree test in front of an
# index read; that was a shared bug rather than a divergence, and both are gone.
#
# ⚠️ **And the two format guards, which this copy did not have at all.** They
# are `check-mutants.sh`'s, for its reasons: an entry with no reason suppresses
# a survivor while recording nothing about why, and an entry whose location is
# not `<file>.rs:<line>:<col>:` argues by prefix — review demonstrated a
# baseline of the single character `c` marking five unargued survivors as
# argued, and this script matched with `[[ "$s" == "$loc"* ]]` exactly as that
# one did.
declare -a argued=()
malformed=0
while IFS= read -r line; do
  [[ -z "$line" || "$line" == \#* ]] && continue
  if [[ ! "$line" =~ ^([^[:space:]]+([[:space:]][^[:space:]]+)*)[[:space:]][[:space:]]+[^[:space:]] ]]; then
    fail "baseline entry has no reason: $line"
    note "format: <file>:<line>:<col>: <mutation>  <why this survivor is acceptable>"
    malformed=1
    continue
  fi
  if [[ ! "${line%%  *}" =~ ^[^[:space:]]+\.rs:[0-9]+:[0-9]+: ]]; then
    fail "baseline entry does not start with a <file>.rs:<line>:<col>: location: $line"
    malformed=1
    continue
  fi
  argued+=("$line")
done < <(git show ":$BASELINE" 2>/dev/null || true)

if (( malformed > 0 )); then
  note "the same two guards check-mutants.sh applies, and for its reasons"
  finish
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
  # ⚠️ The same four lines `check-mutants.sh` prints for the same condition.
  # An operator who hits this on a nightly and then again locally must not be
  # told two different things about one defect — `M4.69`, which found the two
  # copies' wordings had already drifted. ⚠️ The `--list` line was only here and
  # the other three only there; both print all four now.
  note "the mutant was killed, moved, or stopped being generated -- delete the"
  note "entry, re-key it, or say 'unviable: <why it no longer compiles>'"
  note "⚠️ a suppression for something that is not there is one nobody can see"
  note "⚠️ cargo mutants --list re-derives a key in seconds, without running a test"
  finish
fi

ok "every argued baseline entry still argues a live survivor (${#argued[@]} entr(y|ies), ${#survivors[@]} survivor(s) across $expected shard(s))"
finish
