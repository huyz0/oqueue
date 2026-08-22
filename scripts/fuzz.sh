#!/usr/bin/env bash
# Every decoder's fuzz target, run to its corpus — security.md rule 5 and
# testing.md rule 24, deferred into M2 by M0's checkpoint review and
# discharged by M2.26.
#
# ## What a run means
#
# Each target under crates/oqueue-codec/fuzz/fuzz_targets/ runs for a
# bounded number of executions, seeded from the checked-in corpus — which
# includes the real librdkafka frames the golden corpus captured, so the
# fuzzer starts from bytes a real client actually sent rather than from
# nothing. A crash, a sanitizer report, or a leak fails the run and leaves
# the reproducing input under fuzz/artifacts/<target>/ for replay with
# `cargo +nightly fuzz run <target> <artifact>`.
#
# ⚠️ Bounded, not exhaustive: RUNS_PER_TARGET below is a smoke depth chosen
# so the whole script stays in the nightly tier's minutes, not a claim of
# coverage. Raising it is always safe; the direction that would weaken this
# gate is lowering it (non-negotiable 2).
#
# ## Tooling
#
# Needs a nightly toolchain and cargo-fuzz (libFuzzer). Both missing-tool
# cases are skips with remedies, lib.sh's contract — but a skip here means
# NO fuzzing ran, and the nightly tier should treat that as its own alarm.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

FUZZ_DIR="crates/oqueue-codec/fuzz"
# ⚠️ A constant, deliberately not an environment override: this is a gate
# threshold, and a threshold an environment can move is non-negotiable 2's
# canonical violation (check-drift.sh's own worked example is exactly the
# `${VAR:-default}` shape). Raising it is an edit under review like any
# other; lowering it is the weakening direction.
RUNS_PER_TARGET=100000

if ! has_rust; then
  skip "fuzzing (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish
if ! command -v cargo-fuzz >/dev/null 2>&1; then
  skip "fuzzing (cargo-fuzz not installed; cargo install cargo-fuzz)"
  finish
fi
if ! rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
  skip "fuzzing (no nightly toolchain; rustup toolchain install nightly)"
  finish
fi

# Every target the fuzz crate declares, discovered rather than listed here
# — a hardcoded list is how a sixth decoder ships without a target while
# this script stays green. The check below closes the other direction.
mapfile -t targets < <(find "$FUZZ_DIR/fuzz_targets" -name '*.rs' -exec basename {} .rs \; | sort)
if (( ${#targets[@]} == 0 )); then
  fail "no fuzz targets found under $FUZZ_DIR/fuzz_targets"
  finish
fi

# ⚠️ Every module in oqueue-codec must have a target or a recorded reason
# not to — derived from the source tree, so the check FAILS CLOSED: a new
# module added without a target (rule 24 already names the next one, index
# search over corrupt blobs) turns this red rather than staying invisible.
# The allowlist names the modules that parse no untrusted byte stream of
# their own, each with its reason.
declare -A NOT_A_PARSER=(
  [lib]="the module root; declares, parses nothing"
  [wire]="Cursor primitives -- every target drives them transitively"
  [versions]="a static advertised-versions table; parses nothing"
  [attributes]="a bitfield over an i16 the batch decoder already produced"
)
for src in "$REPO_ROOT"/crates/oqueue-codec/src/*.rs; do
  module="$(basename "$src" .rs)"
  [[ -n "${NOT_A_PARSER[$module]:-}" ]] && continue
  found=0
  for t in "${targets[@]}"; do [[ "$t" == "$module" ]] && found=1; done
  if (( ! found )); then
    fail "decoder module '$module' has no fuzz target and no allowlist reason"
  fi
done

# Build first, separately: a target that does not compile is a build
# failure, not a crash, and the two need different operators. This is also
# the compile coverage the root workspace cannot give a crate deliberately
# outside it -- an oqueue-codec or oqueue-broker rename that breaks the
# targets turns THIS red rather than nothing.
mkdir -p target
if ! (cd "$FUZZ_DIR" && cargo +nightly fuzz build) > target/fuzz-build.log 2>&1; then
  fail "the fuzz targets do not build"
  note "$(tail -5 target/fuzz-build.log 2>/dev/null || true)"
  finish
fi
ok "all ${#targets[@]} fuzz targets build"

for target in "${targets[@]}"; do
  # Two corpus dirs: the first (scratch, gitignored) receives what the
  # fuzzer discovers; the second is the committed seed set -- the real
  # librdkafka frames among them -- which stays read-only and reviewed.
  mkdir -p "$FUZZ_DIR/corpus/$target"
  if (cd "$FUZZ_DIR" && cargo +nightly fuzz run "$target" \
      "corpus/$target" "seeds/$target" -- \
      -runs="$RUNS_PER_TARGET" -max_len=65536 -print_final_stats=0) \
      > "target/fuzz-$target.log" 2>&1; then
    ok "fuzz target '$target' survived $RUNS_PER_TARGET runs from its corpus"
  else
    fail "fuzz target '$target' crashed -- reproducer under $FUZZ_DIR/artifacts/$target/"
    note "$(tail -5 "target/fuzz-$target.log" 2>/dev/null || true)"
  fi
done

finish
