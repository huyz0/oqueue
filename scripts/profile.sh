#!/usr/bin/env bash
# Every profiling mode is one command. `M-1.28`, `performance.md` rules 20-22.
#
#   scripts/profile.sh <mode> <bench> [-- <extra args to cargo>]
#
#   mode: instructions | flame | heap | massif | cache | alloc
#
# ⚠️ **This is a tool, not a gate — see `performance.md`'s "Profiling: on
# demand, never gated".** It is not wired into `.pre-commit-config.yaml`, has
# no entry in `tests/gates/negative.sh`, and calling `finish` here means "the
# command ran or gave a clear reason it didn't", never "the profile looked
# good" — nothing here judges output. Runnable at any moment without
# ceremony is the whole point; gating it would either slow the loop to a
# crawl or produce alerts nobody trusts.
#
# ## The table this implements
#
# | mode | tool | performance.md row |
# |---|---|---|
# | `instructions` | gungraun (Callgrind) | "Instructions retired, per function" |
# | `flame` | `perf` + `cargo flamegraph` | "Where wall time goes" |
# | `heap` | gungraun (DHAT) | "Heap: what allocated, where, how long" |
# | `massif` | gungraun (Massif) | "Peak memory over time" |
# | `cache` | gungraun (Cachegrind) | "Cache misses and branch misprediction" |
# | `alloc` | allocator stats | "Allocation count and size distribution" |
#
# ## `instructions`/`heap`/`massif`/`cache`: one invocation, four tools
#
# Confirmed against gungraun's own docs before writing this (not assumed):
# gungraun's Callgrind/Cachegrind/DHAT/Massif backends are selected in the
# *benchmark harness's own Rust configuration* — `LibraryBenchmarkConfig`'s
# tool setting — not by a CLI flag or environment variable this script could
# pass. `cargo bench --bench <bench>` is therefore the same command for all
# four; which Valgrind sub-tool actually runs is decided by which harness
# file `<bench>` names and how *that file* configures gungraun, not by this
# script. Naming a bench target per mode (`bench-micro-instructions`,
# `bench-micro-heap`, …) is the natural way to make that concrete once real
# benchmarks exist — a decision for whoever adds the first one, not this
# task's to make.
#
# ## `alloc`: honestly unimplemented
#
# "Allocator stats" names no external tool `instructions`/`heap`/`massif`/
# `cache` don't already cover, and no crate exists to wire a custom global
# allocator into. Reporting a plausible-sounding command here would be
# exactly the thing non-negotiable 3 exists to refuse for test claims,
# extended to tooling: this mode prints what it needs and exits, rather than
# guessing.
#
# ## What this cannot verify
#
# ⚠️ No crate exists yet, and none of `valgrind`, `perf`, or `cargo-flamegraph`
# are installed in the environment this was written in. Every mode's
# `require_tool` skip path is exercised and observed to fail cleanly; the
# actual `cargo bench`/`cargo flamegraph` invocations are not, because there
# is nothing yet to invoke them against. That is the same limit M-1.12
# names for `check-budget.sh`'s constant — measured once a workspace exists,
# not guessed now.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

MODE="${1:-}"
BENCH="${2:-}"
shift 2 2>/dev/null || true
EXTRA_ARGS=("$@")
# `--` between <bench> and pass-through args is conventional but optional;
# strip a leading one so `profile.sh flame my-bench -- --foo` and
# `profile.sh flame my-bench --foo` both work.
if [[ "${EXTRA_ARGS[0]:-}" == "--" ]]; then
  EXTRA_ARGS=("${EXTRA_ARGS[@]:1}")
fi

usage() {
  note "usage: scripts/profile.sh <mode> <bench> [-- <extra cargo args>]"
  note "modes: instructions | flame | heap | massif | cache | alloc"
}

if [[ -z "$MODE" || -z "$BENCH" ]]; then
  fail "profile.sh needs a mode and a bench target"
  usage
  finish
fi

case "$MODE" in
  instructions|heap|massif|cache|flame|alloc) ;;
  *)
    fail "unknown profiling mode: $MODE"
    usage
    finish
    ;;
esac

if ! has_rust; then
  skip "profile $MODE (no Cargo.toml yet)"
  finish
fi

case "$MODE" in
  instructions|heap|massif|cache)
    require_tool valgrind "apt-get install valgrind, or the equivalent for this system" || finish
    require_tool cargo "https://rustup.rs" || finish
    # gungraun's own benchmark harness needs a separate runner binary on
    # PATH (or GUNGRAUN_RUNNER pointing at one) -- not bundled with the
    # `gungraun` library dependency, so `cargo`+`valgrind` alone are not
    # enough. Found by review: without this check, a developer with
    # everything else installed got a raw gungraun error instead of a
    # clean, remedy-bearing skip -- exactly what require_tool exists to
    # give every other missing-tool path in this file.
    require_tool gungraun-runner "cargo install gungraun-runner" || finish
    ok "running $MODE via gungraun (cargo bench selects the tool through the bench target's own config)"
    exec cargo bench --bench "$BENCH" "${EXTRA_ARGS[@]}"
    ;;
  flame)
    require_tool perf "apt-get install linux-tools-common linux-tools-generic, or the equivalent for this system" || finish
    if ! cargo flamegraph --version >/dev/null 2>&1; then
      skip "cargo-flamegraph not installed  (install: cargo install flamegraph)"
      finish
    fi
    ok "running flame via perf + cargo-flamegraph"
    exec cargo flamegraph --bench "$BENCH" "${EXTRA_ARGS[@]}"
    ;;
  alloc)
    skip "alloc (no instrumented allocator wired into any crate yet)"
    note "allocation count and size distribution needs a project-specific"
    note "global allocator (a feature flag on the crate under test), which"
    note "does not exist until a crate does -- see this script's header"
    finish
    ;;
esac
