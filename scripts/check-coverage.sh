#!/usr/bin/env bash
# Line coverage, with a **per-crate** floor. `M0.15`, `testing.md` rule 19.
#
#   scripts/check-coverage.sh
#
# ## Why per-crate and not the workspace aggregate
#
# ⚠️ NFR-55 and `testing.md` rule 19 both say **per-crate floor**, and the
# distinction is the whole point: a workspace aggregate satisfies every other
# word of the requirement while letting one crate sit at zero behind the
# average. This gate fails on the *lowest* crate.
#
# ## ⚠️ Coverage is necessary and nowhere near sufficient
#
# `testing.md` rule 15 is explicit that mutation testing is the primary quality
# gate, because coverage says a line *ran*, not that anything *constrained* it.
# A crate can sit at 100% here and have tests that assert nothing.
#
# ⚠️ **And it counts inline `#[cfg(test)]` modules as covered lines.**
# `cargo-llvm-cov` drops `tests/` files but not test modules inside `src/`, so a
# crate that keeps its tests inline is partly measuring itself: `bin/oqueue`'s
# 96.77% is majority test code by line count. That is inert today because the
# two crates this floor actually guards keep their tests in `tests/it/`, and it
# stops being inert the moment a guarded crate adopts the inline idiom — a 3:1
# inline-test ratio can sit above the floor with half the production code
# unexecuted. Found by review.
#
# So: a green run is **not** evidence that no production code is unvisited, and
# it is not evidence of test quality either. It is evidence that the crate is
# not obviously untested. `M0.17`'s mutation testing is what carries the weight.
#
# ## The threshold
#
# A literal, below, and ⚠️ **no environment variable moves it.** `check-drift.sh`
# is what enforces that — and it matches on names containing `threshold`,
# `_limit`, `_budget`, `_ceiling` or `_floor`, which is why the constant is
# called `COVERAGE_FLOOR` and not something outside that set. ⚠️ Review measured
# the first version: named `MIN_CRATE_COVERAGE`, it could be made
# environment-settable and `check-drift.sh` passed anyway. Non-negotiable 2 is
# why any of this matters: raising it is fine, lowering it is the thing this
# project does not do.
#
# ## The exclusion list
#
# ⚠️ **Named, with a reason per entry, never silent.** Two kinds are excluded
# and both would otherwise drag the per-crate minimum to zero:
#
#   * crates that are still empty skeletons — they have no executable lines at
#     all, so `llvm-cov` reports no rows for them and a naive minimum would be
#     either 0 or undefined;
#   * `bin/oqueue`, the composition root, whose job is choosing concrete types.
#     `check-layering.sh` already treats it as a composer; a coverage floor on
#     a wiring file measures nothing worth measuring.
#
# ⚠️ **An entry must be removed when its crate acquires logic.** That is the
# whole risk of this list: an exclusion outlives its reason silently. The
# skeleton entries are removed by the milestone that fills each crate.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ── The constant ────────────────────────────────────────────────────────────
#
# Measured on 2026-08-16 at commit `f79f422` with
# `cargo llvm-cov --workspace --summary-only --locked`. Per crate, line
# coverage: `oqueue-core` 91.63% (197/215), `oqueue-crypto` 100% (19/19),
# `bin/oqueue` 96.77% (30/31). Lowest crate carrying logic: **91.63%**.
#
# ⚠️ The floor is set **below** the measurement on purpose. A floor equal to
# what was measured fails on the next commit that adds a line before its test,
# which trains people to lower it — and non-negotiable 2 forbids that, so the
# pressure has to go somewhere else. 85% leaves ~6.6 points of room while still
# refusing a crate that is meaningfully untested.
COVERAGE_FLOOR=85

# ── The exclusion list ──────────────────────────────────────────────────────
#
# ⚠️ **Two kinds, and they expire differently.** Collapsing them into one list
# is what made the first version of this gate wrong in both directions.
#
# `UNTIL_FILLED` — excluded *because the crate is empty*. The entry is a
# temporary fact, and the gate **fails** if the crate acquires executable lines
# without the entry being deleted. That is the hazard this list carries and the
# one `check-file-size.sh`'s allowlist does not: a stale entry there is inert
# until a file grows, a stale entry here switches a floor off exactly when the
# crate starts to matter.
declare -A UNTIL_FILLED=(
  [oqueue-buf]="empty skeleton — M2 fills it"
  [oqueue-codec]="empty skeleton — M2 fills it"
  [oqueue-checksum]="empty skeleton — M2 fills it"
  [oqueue-index]="empty skeleton — M3 fills it"
  [oqueue-store]="empty skeleton — M1 fills it"
  [oqueue-coordinator]="empty skeleton — M3 fills it"
  [oqueue-compact]="empty skeleton — M5 fills it"
  [oqueue-broker]="empty skeleton — M2 fills it"
  [oqueue-testkit]="empty skeleton — harness and generators, M1 onward"
)

# `ALWAYS` — excluded *for what the crate is*, permanently, and expected to have
# code. ⚠️ Adding to this list is the way to defeat this gate, so an entry needs
# a reason a reviewer would accept and there is exactly one today.
declare -A ALWAYS=(
  [oqueue]="composition root — wiring, not logic; check-layering.sh treats it as a composer, and a coverage floor on a wiring file measures nothing worth measuring"
)

if ! has_rust; then
  skip "coverage (no Cargo.toml yet)"
  finish
fi
require_tool cargo "install Rust via https://rustup.rs" || finish
require_python || finish

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  skip "coverage (cargo-llvm-cov not installed)"
  note "install: cargo install cargo-llvm-cov"
  note "⚠️ a missing tool is a skip, never a pass — this check did not run"
  finish
fi

# ⚠️ **No `CARGO_TARGET_DIR` is set here.** `build.md` rule 18 wants coverage in
# its own directory because `-C instrument-coverage` changes the fingerprint —
# and `cargo-llvm-cov` already does that on its own, building into
# `<target>/llvm-cov-target`. Setting it here additionally *overrode* the
# `target-dir` that `tests/gates/negative.sh`'s scratch fixtures configure, so
# the negative suite built into the system temp directory against rule 19.
# Found by review.

cov_json="$REPO_ROOT/target/tmp/coverage.$$.json"
mkdir -p "$REPO_ROOT/target/tmp"
if ! cargo llvm-cov --workspace --locked --json --output-path "$cov_json" >/dev/null 2>&1; then
  fail "cargo llvm-cov failed"
  note "run it directly to see why: cargo llvm-cov --workspace --locked"
  rm -f "$cov_json"
  finish
fi

report="$(python3 - "$cov_json" "$REPO_ROOT" <<'PY'
import json, sys, os, collections

root = sys.argv[2].rstrip(os.sep)
data = json.load(open(sys.argv[1]))
# llvm-cov's json: data[0].files[].filename / .summary.lines.{count,covered}
per_crate = collections.defaultdict(lambda: [0, 0])
for f in data["data"][0]["files"]:
    path = f["filename"]
    # ⚠️ **Relative to the repository root first.** Parsing `crates`/`bin` out of
    # an absolute path takes the first such component *anywhere* in it, so a
    # clone at `~/bin/oqueue` resolved every file to crate `oqueue` and the gate
    # failed every commit. Found by review.
    if path.startswith(root):
        path = path[len(root):].lstrip(os.sep)
    parts = path.split(os.sep)
    crate = None
    for i, p in enumerate(parts):
        if p in ("crates", "bin") and i + 1 < len(parts):
            crate = parts[i + 1]
            break
    if crate is None:
        continue
    lines = f["summary"]["lines"]
    per_crate[crate][0] += lines["count"]
    per_crate[crate][1] += lines["covered"]

for crate, (total, covered) in sorted(per_crate.items()):
    pct = 100.0 * covered / total if total else -1.0
    print(f"{crate}\t{covered}\t{total}\t{pct:.2f}")
PY
)" || { fail "could not parse the coverage report"; rm -f "$cov_json"; finish; }
rm -f "$cov_json"

# ⚠️ **The crate list comes from the workspace, not from the report.** A crate
# with no executable lines produces no `llvm-cov` rows at all — so iterating the
# report would silently ignore exactly the crates the exclusion list exists to
# make deliberate, and the "not in the exclusion list" branch below could never
# fire. Found while testing this script: it reported "1 excluded" when ten
# entries were listed.
mapfile -t workspace_crates < <(
  cargo metadata --no-deps --format-version 1 2>/dev/null |
    python3 -c 'import json,sys; [print(p["name"]) for p in json.load(sys.stdin)["packages"]]' |
    sort
)
if (( ${#workspace_crates[@]} == 0 )); then
  fail "could not list workspace crates"
  finish
fi

declare -A covered_lines total_lines pct_of
while IFS=$'\t' read -r crate covered total pct; do
  [[ -z "$crate" ]] && continue
  covered_lines[$crate]="$covered"
  total_lines[$crate]="$total"
  pct_of[$crate]="$pct"
done <<< "$report"

lowest_name=""
lowest_pct=""
checked=0
excluded=0

for crate in "${workspace_crates[@]}"; do
  total="${total_lines[$crate]:-0}"
  if [[ -n "${ALWAYS[$crate]:-}" ]]; then
    excluded=$((excluded + 1))
    continue
  fi
  if [[ -n "${UNTIL_FILLED[$crate]:-}" ]]; then
    # ⚠️ The entry says the crate is empty. If it is not, the entry has outlived
    # its reason and is silently holding a coverage floor off real code.
    if (( total > 0 )); then
      fail "$crate is excluded as an empty skeleton but has $total executable lines"
      note "reason on file: ${UNTIL_FILLED[$crate]}"
      note "the crate has acquired logic — delete its entry from UNTIL_FILLED"
    fi
    excluded=$((excluded + 1))
    continue
  fi
  covered="${covered_lines[$crate]:-0}"
  pct="${pct_of[$crate]:-0.00}"
  if (( total == 0 )); then
    # ⚠️ Not silently skipped: a crate with no executable lines and no exclusion
    # entry is a crate somebody added without deciding which it is.
    fail "$crate has no executable lines and is not in the exclusion list"
    note "add it to UNTIL_FILLED with a reason, or give it code and tests"
    continue
  fi
  checked=$((checked + 1))
  if [[ -z "$lowest_pct" ]] || python3 -c "import sys; sys.exit(0 if float('$pct') < float('$lowest_pct') else 1)"; then
    lowest_pct="$pct"
    lowest_name="$crate"
  fi
  if python3 -c "import sys; sys.exit(0 if float('$pct') < $COVERAGE_FLOOR else 1)"; then
    fail "$crate line coverage ${pct}% is below the ${COVERAGE_FLOOR}% floor ($covered/$total lines)"
  fi
done

if (( checked == 0 )); then
  # ⚠️ Every crate excluded means the gate proved nothing while exiting 0 —
  # the vacuous-green shape this project keeps finding.
  fail "no crate was checked: all $excluded were excluded"
  note "an exclusion list that covers everything is not a coverage gate"
  finish
fi

# ⚠️ `check-file-size.sh` carries this same guard and the comment explaining it:
# without it a run prints a FAIL line and an ok line for the same check.
if (( _FAILURES > 0 )); then
  finish
fi

ok "line coverage (${checked} crate(s) checked, lowest ${lowest_name} at ${lowest_pct}%, floor ${COVERAGE_FLOOR}%, ${excluded} excluded by name)"
finish
