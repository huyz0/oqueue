#!/usr/bin/env bash
# Format, lint, and test — scoped to one crate, or the whole workspace. `M0.2`.
#
#   scripts/check-crate.sh                 # the whole workspace
#   scripts/check-crate.sh oqueue-core     # one crate
#
# ## Why this exists and why it is scoped
#
# `.agents/skills/tdd/SKILL.md` names this script in its definition of done and
# `.agents/skills/milestone/SKILL.md` names it in the loop, and until `M0.2` it
# did not exist — so the first Rust in the repository would have arrived with
# nothing in `.pre-commit-config.yaml` running a single cargo command against
# it.
#
# The crate argument is the point rather than a convenience. Doc 19 §5: clippy
# over the whole workspace on every edit is too slow to be useful as in-session
# feedback, so an agent runs it scoped, and **CI runs the workspace-wide version
# regardless** — `.github/workflows/gates.yml` invokes this with no argument, so
# nothing goes unchecked by being outside the crate somebody happened to name.
#
# ## What it runs, and what each one is for
#
#   cargo fmt --check       `rust-style.md` rules 1-2: rustfmt's output is
#                           correct by definition, so formatting is never a
#                           review finding.
#   cargo clippy -D warnings  `rust-style.md` rules 3-7 and `clippy.toml`'s
#                           thresholds, which are what make `code-structure.md`
#                           enforceable rather than aspirational.
#   cargo test              `testing.md`. A tree that does not pass its own
#                           tests is not green, and non-negotiable 1 is about
#                           green trees.
#
# ⚠️ **`--all-targets` on clippy.** Without it, clippy lints the library and
# silently ignores tests, benches and examples — which is where the
# `unwrap_used` allowance lives, so it is exactly the code most worth linting.
# `cargo test` deliberately does *not* take it; see the comment at that call.
#
# ⚠️ **The format check is workspace-wide even when a crate is named**, and that
# is a choice rather than a limitation — `cargo fmt -p` does scope correctly.
# rustfmt reads no dependency graph and is fast enough that scoping it buys
# nothing, while a formatting failure in a crate the caller did not name is
# still a formatting failure that will block the commit.
#
# ## What this does not do
#
# It does not check coverage (`check-coverage.sh`, `M0.15`), the time budget
# (`check-budget.sh`, `M0.16`), or mutants (`M0.17`). Those are separate gates
# with separate constants, and folding them in here would make the in-session
# feedback loop this script exists to keep fast into the slowest thing an agent
# runs.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

CRATE="${1:-}"

if ! has_rust; then
  skip "crate checks (no Cargo.toml yet)"
  finish
fi

require_tool cargo "install Rust via https://rustup.rs" || finish

# `-p <crate>` for the two commands that read the dependency graph. ⚠️ Not for
# `cargo fmt`: with `edition.workspace = true` a bare `cargo fmt -p foo` still
# has to resolve the workspace, and rustfmt over every file costs less than the
# resolution does — so the scope argument buys nothing and complicates the
# failure message.
# ⚠️ The manifest is parsed **before** anything else and regardless of whether a
# crate was named. A root `Cargo.toml` that does not parse otherwise surfaces as
# whatever the first cargo command happens to say — review found it reported as
# "rustfmt: files are not formatted", with `cargo fmt --all` as the remedy.
require_python || finish
meta=""
meta_err=""
meta_rc=0
# ⚠️ stderr captured **separately**, not folded in with `2>&1`. cargo warns on
# stdout's sibling stream and still exits 0 — a member-crate `[profile]` section
# does exactly that, which is the mistake `M0.3` exists to warn about — and
# folding the two makes the JSON unparseable, so the gate died with a traceback
# and then claimed the crate did not exist. Found by review.
#
# ⚠️ The stderr file lives under `target/tmp/`, not `/tmp`. `build.md` rule 19
# puts all scratch there, and the same commit's negative-test fixture cites that
# rule -- a gate breaking it two files away would be the first thing to point at
# next time somebody argues the rule is optional.
mkdir -p "$REPO_ROOT/target/tmp"
meta_err_file="$REPO_ROOT/target/tmp/check-crate-metadata-stderr.$$"
meta="$(cargo metadata --no-deps --format-version 1 2>"$meta_err_file")" || meta_rc=$?
meta_err="$(cat "$meta_err_file" 2>/dev/null || true)"
rm -f "$meta_err_file"
if (( meta_rc != 0 )); then
  fail "cargo metadata failed: the workspace manifest does not parse"
  printf '%s\n' "$meta_err" | tail -20 >&2
  finish
fi

# ⚠️ The lockfile is checked here too, for the same reason: `--locked` on clippy
# and on the tests reports a stale lock as "clippy: warnings denied" and "tests:
# failing", which sends the reader to look for a lint. `cargo metadata` compiles
# nothing, so this costs nothing.
# ⚠️ **No `--no-deps` here**, unlike the read above. `--no-deps` skips dependency
# resolution entirely, so `--locked` has nothing to refuse and the check passes
# on a stale lockfile — verified: it did, and the staleness surfaced two steps
# later as "clippy: warnings denied", which is the reporting this block exists
# to prevent.
lock_err_file="$REPO_ROOT/target/tmp/check-crate-lock-stderr.$$"
if ! cargo metadata --locked --format-version 1 >/dev/null 2>"$lock_err_file"; then
  lock_err="$(cat "$lock_err_file" 2>/dev/null || true)"
  rm -f "$lock_err_file"
  # ⚠️ cargo's own message is shown rather than swallowed, and the diagnosis is
  # made from it rather than assumed. Resolution can fail for reasons that have
  # nothing to do with staleness -- an offline cold registry, a checksum
  # mismatch, a `[patch]` that does not apply -- and now that `M0.5` has added the
  # first external dependency, printing "Cargo.lock is stale" with a remedy that
  # cannot work would send the reader somewhere there is nothing to find. Found
  # by review, which noted this is the same misreporting the metadata read above
  # was already fixed for.
  if grep -q -- '--locked' <<< "$lock_err"; then
    fail "Cargo.lock is stale or missing"
    note "a dependency changed without the lockfile being regenerated"
    note "run: cargo check   then stage Cargo.lock  (build.md rule 2)"
  else
    fail "dependency resolution failed, and not because the lockfile is stale"
  fi
  printf '%s\n' "$lock_err" | tail -20 >&2
  finish
fi
rm -f "$lock_err_file"

scope=(--workspace)
label="workspace"
if [[ -n "$CRATE" ]]; then
  # ⚠️ Checked against the workspace's actual package names rather than passed
  # through: `cargo clippy -p nosuch` fails with cargo's own error, which reads
  # like a broken gate rather than a typo in the argument.
  #
  # ⚠️ Parsed, not grepped. `cargo metadata`'s JSON carries *target* names as
  # well as package names, so a grep for `"name":"..."` accepts `oqueue_core` --
  # the lib target, with an underscore -- and hands it to `-p`, where cargo
  # rejects it and this gate reports "clippy: warnings denied" for what is a
  # typo. Found by review.
  if ! printf '%s' "$meta" | python3 -c '
import json, sys
names = [p["name"] for p in json.load(sys.stdin)["packages"]]
sys.exit(0 if sys.argv[1] in names else 1)
' "$CRATE"; then
    fail "no crate named $CRATE in this workspace"
    note "members: $(printf '%s' "$meta" | python3 -c '
import json, sys
print(", ".join(sorted(p["name"] for p in json.load(sys.stdin)["packages"])))
')"
    note "run with no argument to check every crate"
    finish
  fi
  scope=(-p "$CRATE")
  label="$CRATE"
fi

# ⚠️ Each of the three runs even when an earlier one fails, and the exit codes
# are collected rather than short-circuited. `lib.sh`'s own contract: "the
# caller keeps going so one run reports every violation rather than only the
# first". A clippy failure hidden behind a formatting failure costs a whole
# extra edit-run cycle, which is the loop this script is meant to shorten.

if cargo fmt --all --check >/dev/null 2>&1; then
  ok "rustfmt (workspace)"
else
  fail "rustfmt: files are not formatted"
  note "run: cargo fmt --all"
  # Shown rather than summarised — the diff is the whole of the information.
  cargo fmt --all --check 2>&1 | head -40 >&2 || true
fi

clippy_out=""
clippy_rc=0
# ⚠️ `--locked`. `build.md` rule 2: `Cargo.lock` is committed and a stale one
# must fail loudly rather than resolve silently. Doc 21 §9 makes the hook and CI
# run the same script, so the flag cannot live only on the CI side -- which
# means a dependency added without regenerating the lockfile fails here, at the
# commit, and the remedy is to run a plain `cargo check` and stage `Cargo.lock`.
# ⚠️ **No `--all-features` here.** The shipped/default configuration is the
# portable gate configuration; the optional FIPS configuration requires CMake
# and Go by design and is compiled, tested, and linted in
# `docker/release-fips.Dockerfile`. The non-FIPS optional configurations are
# checked explicitly below, so removing `--all-features` does not leave
# `gzip`, `snappy`, or `heap-profiling` uncompiled.
clippy_out="$(cargo clippy --locked "${scope[@]}" --all-targets \
  -- -D warnings 2>&1)" || clippy_rc=$?
if (( clippy_rc == 0 )); then
  ok "clippy ($label)"
else
  fail "clippy ($label): warnings denied"
  printf '%s\n' "$clippy_out" | tail -60 >&2
fi

test_out=""
test_rc=0
# ⚠️ **No `--all-targets` here, unlike clippy above, and the asymmetry is the
# point.** `--all-targets` *excludes* doc tests -- a failing `///` example
# reports "running 0 tests ... ok" -- and this is the only `cargo test` the hook
# and CI run, so a broken doc example would stay green forever from the moment
# `oqueue-core` grows its first documented type. Cargo's default target set is
# lib + bins + tests + doc tests, which is what should gate a commit. Benches
# and examples are still compiled: clippy runs `--all-targets` immediately
# above, so nothing goes uncompiled by being left out here.
# Clippy and tests both cover the configuration that ships. The optional FIPS
# configuration is covered by the isolated release builder above. The other
# optional configurations are checked below when this is the workspace gate or
# when their owning crate is requested directly.
#
# ⚠️ **An earlier version of this comment justified the split by jemalloc's ARM
# page-size trap, and that reason was wrong -- twice, in opposite directions.**
# The truth: jemalloc bakes in the **build host's** page size, natively as well
# as when cross-compiled (this workspace's own `config.log` probes and writes
# `#define LG_PAGE 12`), and a binary aborts when the kernel it *runs on* has a
# larger page than the one baked in. A gate that builds and runs on one machine
# therefore never mismatches itself, which is why the trap is not this script's
# problem -- it is M13's, where a build host and a deployment host can differ.
# ADR-0007, doc 18 §3.5.
#
# ⚠️ Feature-specific code is covered by its owning configuration: the default
# branch is linted here, and the FIPS branch is linted in the isolated builder.
# That split keeps the portable gate honest without leaving the optional branch
# unchecked.
test_out="$(cargo test --locked "${scope[@]}" 2>&1)" || test_rc=$?
if (( test_rc == 0 )); then
  ok "tests ($label)"
else
  fail "tests ($label): failing"
  printf '%s\n' "$test_out" | tail -60 >&2
fi

run_optional_config() {
  local label="$1"
  shift
  local output=""
  local rc=0
  output="$(cargo "$@" 2>&1)" || rc=$?
  if (( rc == 0 )); then
    ok "$label"
  else
    fail "$label"
    printf '%s\n' "$output" | tail -60 >&2
  fi
}

if [[ -z "$CRATE" || "$CRATE" == "oqueue-codec" ]]; then
  run_optional_config \
    "clippy (oqueue-codec gzip+snappy)" \
    clippy --locked -p oqueue-codec --features gzip,snappy --all-targets -- -D warnings
  run_optional_config \
    "tests (oqueue-codec gzip+snappy)" \
    test --locked -p oqueue-codec --features gzip,snappy
fi

if [[ -z "$CRATE" || "$CRATE" == "oqueue" ]]; then
  run_optional_config \
    "clippy (oqueue heap-profiling)" \
    clippy --locked -p oqueue --features heap-profiling --all-targets -- -D warnings
  run_optional_config \
    "tests (oqueue heap-profiling)" \
    test --locked -p oqueue --features heap-profiling
fi

finish
