#!/usr/bin/env bash
# Every benchmark suite is one command. `M-1.28`, `performance.md` rule 2.
#
#   scripts/bench.sh <suite> [-- <extra args to cargo>]
#
#   suite: micro | macro | simd
#
# ⚠️ **A tool, not a gate** — `micro` happens to be the one suite
# `performance.md` rule 2 says gates merges ("bench-micro … ✅ every
# commit"), but the gating decision belongs to whatever CI step or
# pre-commit hook runs the instruction-count regression check against a
# stored baseline, once that exists — this script only runs the suite and
# reports what `cargo bench` reports. Not wired into
# `.pre-commit-config.yaml`, no entry in `tests/gates/negative.sh`, for the
# same reason `profile.sh` has neither.
#
# ## The table this implements
#
# | suite | harness | performance.md says |
# |---|---|---|
# | `micro` | gungraun (instruction counts) | pure CPU leaves: codec, varint, CRC, index lookup |
# | `macro` | criterion, quiet hardware | end-to-end produce/fetch, batching, multi-threaded |
# | `simd`  | criterion, real ISA | anything with runtime ISA dispatch |
#
# Each suite name is also the conventional Cargo bench target name
# (`[[bench]] name = "bench-micro"` etc.) — `cargo bench --bench bench-<suite>
# --workspace` runs every workspace crate that defines a bench target with
# that name and skips the rest, which is what lets this stay one command
# without this script knowing which crates have which benchmarks.
#
# ## What this cannot verify
#
# No crate or bench target exists yet, so the actual `cargo bench`
# invocation has not been run against real code — only argument validation
# and the missing-workspace skip path have been. Same limit `profile.sh`'s
# header names for the same reason.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

SUITE="${1:-}"
shift 1 2>/dev/null || true
EXTRA_ARGS=("$@")
if [[ "${EXTRA_ARGS[0]:-}" == "--" ]]; then
  EXTRA_ARGS=("${EXTRA_ARGS[@]:1}")
fi

usage() {
  note "usage: scripts/bench.sh <suite> [-- <extra cargo args>]"
  note "suites: micro | macro | simd"
}

if [[ -z "$SUITE" ]]; then
  fail "bench.sh needs a suite"
  usage
  finish
fi

case "$SUITE" in
  micro|macro|simd) ;;
  *)
    fail "unknown benchmark suite: $SUITE"
    usage
    finish
    ;;
esac

if ! has_rust; then
  skip "bench $SUITE (no Cargo.toml yet)"
  finish
fi

require_tool cargo "https://rustup.rs" || finish

TARGET="bench-$SUITE"
ok "running $SUITE via cargo bench --bench $TARGET --workspace"
exec cargo bench --bench "$TARGET" --workspace "${EXTRA_ARGS[@]}"
