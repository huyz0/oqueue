#!/usr/bin/env bash
# The nightly random-seed sweep: run the harness under seeds nobody chose, and
# file the first one that fails as an artifact. `M10.11`.
#
#   scripts/seed-sweep.sh          32 seeds
#   SWEEP_SEEDS=200 scripts/seed-sweep.sh
#
# ⚠️ **The artifact is the whole point.** A sweep that goes red and says
# "something failed" costs a night and buys nothing; what a corpus entry needs
# is the seed *and* the versions it was recorded under (`ADR-0028`), which is
# what `target/seeds/failing-*.tsv` holds — in the corpus's own column order,
# so filing it is a copy rather than a transcription.
#
# ⚠️ **Not on the commit path.** A sweep is unbounded work by construction and
# `check-budget.sh` holds the pre-commit suite to ten seconds; this is the
# nightly tier, beside `fuzz.yml`, for the same reason.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib.sh
. "$ROOT/scripts/lib.sh"

# ⚠️ A count, not a threshold: nothing passes or fails because of it, it only
# says how long the night's search runs.
SEEDS="${SWEEP_SEEDS:-32}"
OUT="$ROOT/target/seeds"

current_toolchain() { rustc --version 2>/dev/null | awk '{print $2}'; }
current_tokio() {
  awk -F'"' '/^name = "tokio"$/{want=1} want && /^version = /{print $2; exit}' \
    "$ROOT/Cargo.lock" 2>/dev/null
}

main() {
  mkdir -p "$OUT"
  local toolchain tokio failed=0
  toolchain="$(current_toolchain)"
  tokio="$(current_tokio)"

  # ⚠️ **Build once, and say so if that is what broke.** Without this the first
  # iteration of a tree that does not compile files a random number as a
  # failing schedule, and with the run's output discarded there is nothing in
  # the night's log to say otherwise — `fuzz.yml`'s header calls this the
  # crash-vs-build distinction and gives it as the reason its own script owns
  # the loop.
  if ! cargo test -p oqueue-broker --test it --no-run > "$OUT/build.log" 2>&1; then
    fail "the tree does not build — see target/seeds/build.log; no seed is implicated"
    finish
  fi

  for ((i = 0; i < SEEDS; i++)); do
    # ⚠️ `$RANDOM` twice: bash gives 15 bits, and a sweep over 32768 schedules
    # would revisit them long before it found anything new.
    local seed=$(( (RANDOM << 15 | RANDOM) + 1 ))
    local log="$OUT/seed-$seed.log"
    if OQUEUE_SEED="$seed" cargo test -p oqueue-broker --test it -- --exact "invariants::the_three_invariants_hold_at_every_step_of_a_faulted_run" > "$log" 2>&1; then
      # ⚠️ **A filter that matched nothing is not a pass.** `cargo test` exits 0
      # when it selects no tests, so a renamed run turns the sweep into N green
      # runs of nothing.
      if ! grep -q '1 passed' "$log"; then
        fail "the seeded run matched no test — has it been renamed? see ${log#"$ROOT"/}"
        finish
      fi
      rm -f "$log"
      continue
    fi
    # ⚠️ **The diagnostic is kept, and an earlier version threw it away.**
    # `Broken`'s own `Display` says which invariant broke, at what step, with
    # what offsets — which is the only place the corpus's `found` column can
    # be filled from, and without it "filing an entry is a copy" was false.
    local found
    found="$(grep -om1 'a consumer can see[^"]*\|offsets are not gap-free[^"]*\|the high watermark is[^"]*' "$log" \
      || echo "see seed-$seed.log")"
    local artifact="$OUT/failing-$seed.tsv"
    printf '%s\t%s\t%s\t%s\t%s\n' \
      "$seed" "$toolchain" "$tokio" "$found" "unfixed" > "$artifact"
    fail "seed $seed failed — filed at ${artifact#"$ROOT"/} beside ${log#"$ROOT"/}, add it to tests/seeds/corpus.tsv"
    failed=1
    break
  done

  (( failed )) || ok "$SEEDS seed(s) swept, none failed"
  finish
}

main "$@"
