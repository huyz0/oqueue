#!/usr/bin/env bash
# Replays every seed in the corpus, and says when a pin has drifted. `M10.11`.
#
#   scripts/seed-corpus.sh              replay every entry
#   scripts/seed-corpus.sh --self-test  check the machinery with no entries
#
# ⚠️ **A drifted pin is a warning and not a failure**, and that is a decision
# rather than laziness: the entry still runs and still passes or fails, it just
# no longer reaches the schedule it was recorded for. Failing the build on a
# toolchain bump would make `build.md` rule 1 impossible; saying nothing would
# let the corpus quietly become a list of arbitrary numbers. `ADR-0028`'s
# Consequences are the argument.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# shellcheck source=lib.sh
. "$ROOT/scripts/lib.sh"

# ⚠️ Overridable so `--self-test` can point the reader at a probe file. A path
# is not a threshold; non-negotiable 2 is about values a gate compares against.
CORPUS="${CORPUS:-$ROOT/tests/seeds/corpus.tsv}"
SELF_TEST=0
[[ "${1:-}" == "--self-test" ]] && SELF_TEST=1

have_corpus() {
  [[ -f "$CORPUS" ]] || { fail "no corpus at tests/seeds/corpus.tsv"; return 1; }
}

# The versions an entry recorded today would carry.
current_toolchain() { rustc --version 2>/dev/null | awk '{print $2}'; }
current_tokio() {
  awk -F'"' '/^name = "tokio"$/{want=1} want && /^version = /{print $2; exit}' \
    "$ROOT/Cargo.lock" 2>/dev/null
}

# Every non-comment, non-blank row, as `seed<TAB>toolchain<TAB>tokio<TAB>found`.
#
# ⚠️ **`awk`, not `read -r` with `IFS=$'\t'`.** A tab is whitespace to bash's
# field splitting, so consecutive tabs collapse into one and a row with an
# *empty* pin — the exact malformation this reader exists to refuse — arrives
# with its later columns shifted left and passes. `tests/gates/negative.sh`
# caught that, which is what its rule-20a case is for.
entries() {
  awk -F'\t' '
    /^[[:space:]]*(#|$)/ { next }
    { printf "%s\037%s\037%s\037%s\037%d\n", $1, $2, $3, $4, NF }
  ' "$CORPUS"
}

replay_one() {
  local seed="$1" toolchain="$2" tokio="$3" found="$4"
  local now_toolchain now_tokio
  now_toolchain="$(current_toolchain)"
  now_tokio="$(current_tokio)"
  if [[ "$toolchain" != "$now_toolchain" || "$tokio" != "$now_tokio" ]]; then
    warn "seed $seed was recorded under rustc $toolchain / tokio $tokio, and this is rustc $now_toolchain / tokio $now_tokio — it still runs and no longer reaches the schedule it was recorded for ($found)"
  fi
  # ⚠️ **`--exact`, and the count is checked.** `cargo test` exits 0 when a
  # filter selects nothing, so a renamed or moved test turns this whole gate
  # into a green run of zero tests — over a corpus that then protects nothing.
  local out
  out="$(OQUEUE_SEED="$seed" cargo test -p oqueue-broker --test it -- --exact "invariants::the_three_invariants_hold_at_every_step_of_a_faulted_run" 2>&1)" || {
    printf '%s\n' "$out" >&2
    return 1
  }
  if ! grep -q '1 passed' <<<"$out"; then
    printf '%s\n' "$out" >&2
    fail "the seeded run matched no test — has it been renamed?"
    return 1
  fi
}

main() {
  have_corpus || finish
  local count=0
  # ⚠️ **`\037`, the ASCII unit separator, not `|`.** A pipe is plausible in
  # the free-text `found` column, and a row carrying one shifted the fields so
  # that `[[ "b|5" -lt 5 ]]` evaluated `b` arithmetically and killed the script
  # under `set -u` — fail-closed, but naming no row, no rule and no remedy, and
  # skipping every entry after it.
  while IFS=$'\037' read -r seed toolchain tokio found fields; do
    [[ -n "${seed:-}" ]] || continue
    if [[ ! "$seed" =~ ^[0-9]+$ ]]; then
      fail "malformed corpus row: seed '$seed' is not a number"
      continue
    fi
    # ⚠️ **Bounded to a `u64`, because the harness's own parser is.** A longer
    # digit string passed the regex, failed to parse in the test, and — before
    # that fallback was removed — ran the *default* schedule while this script
    # reported the row as replayed.
    if (( ${#seed} > 20 )) || [[ ${#seed} -eq 20 && "$seed" > "18446744073709551615" ]]; then
      fail "seed $seed does not fit a u64"
      continue
    fi
    if [[ "$fields" -lt 5 ]]; then
      fail "seed $seed has $fields column(s); five are required"
      continue
    fi
    if [[ -z "$toolchain" || -z "$tokio" ]]; then
      fail "seed $seed records no toolchain or tokio version — see ADR-0028"
      continue
    fi
    replay_one "$seed" "$toolchain" "$tokio" "$found" || fail "seed $seed no longer passes"
    count=$((count + 1))
  done < <(entries)

  if (( SELF_TEST )); then
    # ⚠️ The machinery, exercised where the corpus cannot exercise it: an
    # empty corpus is the honest state and asserts nothing on its own.
    local probe; probe="$(mktemp)"
    printf '#seed\ttoolchain\ttokio\tfound\tfixed_by\nnotanumber\ta\tb\tc\td\n' > "$probe"
    if CORPUS="$probe" bash "$ROOT/scripts/seed-corpus.sh" >/dev/null 2>&1; then
      fail "the corpus reader accepts a malformed seed"
    else
      ok "a malformed seed is refused"
    fi
    rm -f "$probe"
  fi

  ok "$count corpus seed(s) replayed"
  finish
}

main "$@"
