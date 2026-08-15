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

# ⚠️ **A separate target directory, and this is not hygiene — it is the only
# way to keep `target/release` from being replaced by a benchmark build.**
# Cargo gives the built-in `bench` profile no directory of its own: `dist`
# writes `target/dist` and `release-checked` writes `target/release-checked`,
# but `bench` writes **`target/release`**, the same path `--profile release`
# writes. Since `M0.3` those two are different programs — `bench` inherits
# `dist`, so fat LTO, `codegen-units = 1` and full debuginfo — so without this
# line one `scripts/bench.sh` run leaves a fat-LTO binary sitting where a
# release artifact is expected, and any later step packaging "whatever is in
# `target/release`" ships it. ⚠️ And the swap is silent: both profiles' objects
# coexist in `target/release/deps` under different metadata hashes, so
# alternating rebuilds nothing and only the uplifted artifact changes. A
# collision that cost a rebuild would at least announce itself.
# ADR-0001 commitment 4; `build.md` rule 18 sets coverage aside for the same
# kind of reason.
# ⚠️ A **subdirectory of** any outer `CARGO_TARGET_DIR`, not a fallback for one.
# Written as `${CARGO_TARGET_DIR:-...target/bench}` an outer setting — a CI
# cache, a worktree, rust-analyzer — would put benchmarks straight back into the
# same tree as that setting's `release` directory, which is the collision this
# line exists to avoid. Honouring the override and then always descending keeps
# both properties.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$REPO_ROOT/target}/bench"

ok "running $SUITE via cargo bench --bench $TARGET --workspace"
note "target dir: $CARGO_TARGET_DIR  (keeps target/release intact)"
exec cargo bench --bench "$TARGET" --workspace "${EXTRA_ARGS[@]}"
