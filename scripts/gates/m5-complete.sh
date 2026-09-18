#!/usr/bin/env bash
# The M5 completion condition, as `M5.md` states it: read amplification after
# compaction is within bound (FR-34); an idle partition's data is deleted on
# schedule with no write to trigger it (FR-33); and the GC safety inequality
# holds as an invariant test under M10's simulation, with a stale reader racing
# a deleter (FR-35). Plus leg 0b, the scoped handoff check `M4.79` wrote.
#
# ## ⚠️ Why this file existing matters more than it passing
#
# `milestone/SKILL.md` writes the driving loop as `until completion condition
# exits 0`. For the whole of `M5` that condition was **this file, which did not
# exist** — it was `M5.27`, a `todo` row inside the milestone it was supposed
# to terminate. A loop whose exit test cannot be evaluated does not stop; it
# falls back to the only other reading available, "keep going until nothing is
# `todo`", which that same skill names as a loop with no exit and which `M4`
# had already demonstrated with three boundary rounds opening 8, 10 and 7 rows.
#
# ⚠️ **Measured**: `M5` was decomposed into **38** rows, `M5.0`-`M5.37`, by
# commit `232073c`, and at commit `7c84d39` its backlog held **84** — 26 of the additions filed by
# reviews of the commits closing the other rows, and the count never once fell.
# ⚠️ **A commit is named because the number moves**; re-derive it with
# `grep -c '^| M5\.' docs/internal/product/backlog.md` rather than trusting
# this sentence, which is the discipline `M5.64` and `M5.74` exist to teach. That is not a milestone being thorough, it is a milestone
# with no exit test. ⚠️ **`M5.84` is the row that would stop the next milestone
# opening the same way, and it is `todo`** — written here in the conditional it
# deserves, because `AGENTS.md` says a script in neither the backlog nor
# `roadmap.md`'s deferral table is unscheduled, never "already done".
#
# ## A red leg is this gate working
#
# ⚠️ **This is not a pre-commit hook** — no `m*-complete.sh` is, and `NFR-56`
# gives the whole pre-commit suite 10 s. It is allowed to be red for as long as
# the milestone is open, and while it is red it is the *statement of what M5
# has left*: three assertions, not thirty-four rows. Read the red legs as the
# remaining scope and the backlog as the working notes, never the other way
# round.
#
# ## Each leg is falsifiable, or says it is not
#
# `M3.16`/`M4`/`M10.15`/`M11.12`'s precedent. ⚠️ **A leg with no mechanism
# behind it fails; it does not skip.** A skip means "this could not be run
# here" — a missing client, an absent toolchain — and reads as "nothing is
# wrong with the tree". A requirement nothing implements is something wrong
# with the tree, and `M5.55` found the packet making exactly this substitution
# in the other direction.

source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT" || exit 1

# ── 0. Every M5 commit read as a whole ──────────────────────────────────────
#
# ⚠️ **Above every skip and every tool requirement** — `M1.43`'s finding and
# `m3-complete.sh`'s precedent, in terms: this needs no cargo, so nothing below
# may gate it. ⚠️ **A first draft of this file put `require_tool cargo ||
# finish` above both legs**, which made the whole gate exit **0** after one
# `skip` line on any machine without the Rust toolchain — and since
# `milestone/SKILL.md` drives `until completion condition exits 0`, that is the
# milestone declaring itself complete over 35 undispositioned rows on the
# strength of a missing compiler. A gate reporting success while checking
# nothing, in the file written to stop a milestone ending wrongly. Found by
# `M5.27`'s third round.
mr_rc=0
bash "$REPO_ROOT/scripts/check-milestone-review.sh" --milestone M5 || mr_rc=$?
if (( mr_rc == 0 )); then
  ok "leg 0: every M5 commit is covered by a milestone review"
elif (( mr_rc == 127 )); then
  fail "leg 0: scripts/check-milestone-review.sh could not be run (exit 127)"
  note "the gate is absent, not the review -- M5's coverage is unknown, not failing"
else
  fail "leg 0: M5's commits have not been read as a whole (exit $mr_rc)"
  note "run: scripts/milestone-review.sh context --milestone M5 to build the packet"
fi

# ── 0b. And every row M5 leaves open has been handed on ─────────────────────
#
# `M4.79` wrote `--milestone`, and `M4.84` is the row saying nothing makes the
# next gate carry it; until `M5.35` makes it a rule it is carried by hand.
#
# ⚠️ **This is the leg that would have stopped M5 growing**, and nothing ever
# ran it, because it runs from this file and this file did not exist until
# `M5.27`. It refuses a close over rows no deferral row names — the disposition
# the milestone skill requires — and unrun, it left "keep going until nothing
# is `todo`" as the only available reading of done.
#
# ⚠️ **Its output is let through, not discarded**: the sub-check prints the ids
# it is refusing over, and that list is the actionable half.
#
# ⚠️ **Exit 3 is a skip, not a refusal.** Once M5's roadmap cell reads
# `complete` the check declines to answer, because the rows it handed on have
# been dissolved by then — so a red answer would be permanent, which is
# `M4.77`'s subject: a gate red by design makes a real regression
# indistinguishable from work in progress. `m4-complete.sh` has this branch and
# the first two drafts of this file omitted it.
ho_rc=0
bash "$REPO_ROOT/scripts/check-milestone-handoff.sh" --milestone M5 || ho_rc=$?
if (( ho_rc == 0 )); then
  ok "leg 0b: every row M5 leaves open is handed on by a roadmap.md deferral row"
elif (( ho_rc == 3 )); then
  skip "leg 0b: M5's handoff (asked after M5's roadmap cell flipped to \`complete\`)"
  note "the rows M5 handed on have been dissolved, so this leg proves nothing now;"
  note "it is the boundary run, before the next milestone's opening commit, that counts"
elif (( ho_rc == 127 )); then
  fail "leg 0b: scripts/check-milestone-handoff.sh could not be run (exit 127)"
  note "the gate is absent, not the handoff -- M5's dispositions are unknown"
else
  fail "leg 0b: M5's handoff check refused (exit $ho_rc) -- its output above says why"
fi

# ⚠️ **No `require_tool … || finish` here, and that is deliberate.** Each FR
# leg below is in two halves: "is there a test of this name", which is a
# `git grep` and needs no toolchain, and "does it run and pass", which does.
# Requiring cargo up front skipped *both*, so a machine without the toolchain
# reached `finish` with nothing failed and exited **0** — and this file is what
# `milestone/SKILL.md` drives `until … exits 0`, so that is the milestone
# declaring itself complete with its three requirements never evaluated. Round
# three of `M5.27` found that as a blocking defect and it was fixed by moving
# legs 0 and 0b above the requirement; round four found the same shape still
# covering the three existence checks, which are cargo-free and are the whole
# of this gate's signal today.
#
# ⚠️ **So a missing toolchain is a failure here, not a skip.** Everywhere else
# in this repository a skip means "this could not be run here" and is the
# honest answer. For the one file that is a milestone's exit condition, "could
# not be evaluated" must never be reachable from a green exit.
have_cargo=1
command -v cargo >/dev/null 2>&1 || have_cargo=0

# ⚠️ **A named test must exist *and* run.** `cargo test <filter>` exits **0**
# when the filter selects nothing, so "the grep matched and cargo was happy" is
# green for a test that is in another crate's binary, or is `#[ignore]`d, or
# was renamed. Both halves are checked: the name is found under a *tests*
# directory, and the run reports at least one test passing.
#
# ⚠️ **The pathspec is `crates/*/tests/*`, and the missing `/*` is not a
# typo-class mistake.** A git pathspec wildcard is matched against the whole
# path without anchoring at `/`, so `crates/*/tests` matches only a path that
# *ends* in `tests` — of which this repository has none. Written that way,
# every leg below fails whatever the tree contains: a leg that cannot pass,
# which is the mirror of a leg that cannot fail, and it would have made this
# whole file unfalsifiable while looking like it was working. Found by
# `M5.27`'s first round.
names_a_test() {
  local name="$1" found
  # ⚠️ **`src` as well as `tests`.** An invariant test under M10's simulation
  # may live in a `#[cfg(test)]` module beside the code it constrains — which
  # is where `m3-complete.sh` finds several of its own legs — and searching
  # only `tests/` would prescribe a location no task asks for.
  found="$(git grep -ln -- "fn ${name}" 'crates/*/tests/*' 'crates/*/src/*' 2>/dev/null)" || return 1
  [[ -n "$found" ]] || return 1
  printf '%s' "$found"
}

# run_counted <label> <min-tests> <cargo test args...>
#
# ⚠️ **`m2-complete.sh`'s helper, and reusing its shape is the point.** The
# *check* is theirs unchanged — the status, then the pass count — and the two
# differ only in what they print, which `diff` is the honest way to see. This
# comment deliberately does not enumerate those differences: two earlier
# attempts to describe them were each wrong about one, which is a smaller
# version of what the count sentence above tells the reader to do instead of
# trusting prose. A
# first draft of this file grew its own runner that shelled out to
# `scripts/docker-test.sh` — which is how `AGENTS.md` says to run *cargo*, and
# exactly wrong here: `AGENTS.md`'s own example runs a completion gate **as**
# `scripts/docker-test.sh scripts/gates/m3-complete.sh`, so the inner call is a
# container inside a container, `docker` is not on the inner PATH, and the leg
# reports exit 127 as "the test did not pass". A met requirement would read as
# unmet on the documented path and on every CI runner. `require_tool` below is
# how the other completion gates say "this machine cannot run it" instead.
# Found by `M5.27`'s second round.
run_counted() {
  local label="$1" min="$2"; shift 2
  local out passed
  if ! out="$(cargo test "$@" --quiet 2>&1)"; then
    fail "$label: the run itself failed"
    note "run: cargo test $*"
    printf '%s\n' "$out" | tail -20
    return 1
  fi
  # ⚠️ A filter matching nothing exits 0 ("0 passed; N filtered out"), so the
  # count is the check and the status is not.
  passed="$(printf '%s\n' "$out" | grep -oE '[0-9]+ passed' | awk '{s+=$1} END{print s+0}')"
  if (( passed < min )); then
    fail "$label matched only $passed test(s), floor $min"
    note "a renamed, moved or #[ignore]d test leaves a filter green"
    return 1
  fi
  ok "$label ($passed test(s))"
}

# ── FR-34: read amplification after compaction is within bound ──────────────
#
# ⚠️ **The subject is the ratio after a round, not the planner's threshold.**
# `COMPACTION_READ_AMP_THRESHOLD` is what makes a partition a *candidate*;
# FR-34 is about what the round then achieves, which is a different number and
# needs a before-and-after over one partition.
if where="$(names_a_test a_compacted_partition_reads_within_the_bound)"; then
  note "FR-34: asserted by $where"
  if (( ! have_cargo )); then
    fail "FR-34: a test of this name exists and no toolchain here can run it"
    note "install the Rust toolchain (rustup.rs) -- this gate is the milestone's"
    note "exit condition, so 'could not evaluate' may not read as 'complete'"
  elif run_counted "FR-34" 1 --workspace a_compacted_partition_reads_within_the_bound; then
    :
  else
    note "FR-34 is read amplification after a round, and it is not met"
  fi
else
  fail "FR-34: no test measures read amplification after a compaction round"
  note "the tree has read_amp() and a threshold that selects candidates, and"
  note "asserts neither that a round brings the ratio under the bound nor by"
  note "how much -- 'a_fetch_over_the_compacted_range_resolves_to_one_reference'"
  note "is the index-side criterion (M5.5), not the requirement"
fi

# ── FR-33: an idle partition's data is deleted on schedule ─────────────────
#
# ⚠️ **"With no write to trigger it" is the whole requirement.** Retention
# driven by the next produce is retention that never runs on the partitions
# that need it most.
if where="$(names_a_test an_idle_partition_is_reaped_with_no_write)"; then
  note "FR-33: asserted by $where"
  if (( ! have_cargo )); then
    fail "FR-33: a test of this name exists and no toolchain here can run it"
    note "install the Rust toolchain (rustup.rs) -- this gate is the milestone's"
    note "exit condition, so 'could not evaluate' may not read as 'complete'"
  elif run_counted "FR-33" 1 --workspace an_idle_partition_is_reaped_with_no_write; then
    :
  else
    note "FR-33 is idle-partition retention, and it is not met"
  fi
else
  fail "FR-33: nothing deletes an idle partition's data"
  note "no retention mechanism is in the tree: time-based (M5.16), size-based"
  note "(M5.17), the idle-partition expiry heap (M5.18) and trim() (M5.19) are"
  note "all todo, and FR-33 is not met by any subset of them"
fi

# ── FR-35: the GC safety inequality, under simulation ──────────────────────
#
# ⚠️ **An invariant test under M10's simulation, with a stale reader racing a
# deleter** — `M5.md`'s own words, and the race is the point: a deleter that is
# safe when nothing is reading proves nothing about the hazard.
if where="$(names_a_test a_stale_reader_racing_a_deleter)"; then
  note "FR-35: asserted by $where"
  if (( ! have_cargo )); then
    fail "FR-35: a test of this name exists and no toolchain here can run it"
    note "install the Rust toolchain (rustup.rs) -- this gate is the milestone's"
    note "exit condition, so 'could not evaluate' may not read as 'complete'"
  elif run_counted "FR-35" 1 --workspace a_stale_reader_racing_a_deleter; then
    :
  else
    note "FR-35 is the GC safety inequality under simulation, and it is not met"
  fi
else
  fail "FR-35: the GC safety inequality is asserted by nothing"
  note "M5.22 is the row, and it names M10's simulation as the harness --"
  note "there is no deletion path yet for a reader to race (M5.20, M5.21)"
fi

finish
