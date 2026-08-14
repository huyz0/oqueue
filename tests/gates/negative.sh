#!/usr/bin/env bash
# Every gate is invoked against a deliberately broken artifact and observed
# to fail. `M-1.15`.
#
#   tests/gates/negative.sh
#
# ## Why this exists
#
# "A gate nobody has watched fail is a gate nobody has tested" — this
# project's own retrospective, first stated for `check-commit-msg.sh` and
# repeated at every gate since. Every gate in `scripts/` has, in fact, been
# run against a contrived negative case at least once while it was being
# written; the finding this task closes is that each of those runs was
# manual, one-off, and lives only in a commit message. This makes it
# permanent, checked in, and re-run on demand instead of trusted from memory.
#
# ⚠️ **This is a test suite, not a gate.** It reports the same `ok`/`fail`
# vocabulary every gate uses for consistency, but its pass condition is
# inverted: a case *passes* when the gate under test *fails*. A case whose
# gate reports `ok` on a broken artifact is this suite's failure, and the
# one thing it exists to catch.
#
# ## How each case is built
#
# Every case gets its own disposable git repository under `mktemp -d`,
# never the real tree. `lib.sh` computes `REPO_ROOT` from its own file's
# location on disk (`$(dirname "${BASH_SOURCE[0]}")/..`), not the caller's
# working directory, so a gate script only resolves against the scratch
# repo if `lib.sh` and the gate itself are physically copied into it — that
# is why every case copies both rather than invoking the real `scripts/`
# files against a different `cwd`. Each gate's only cross-file dependency is
# `lib.sh`; none of them source one another.
#
# ## Setup versus verdict
#
# Each case is split into a `setup_*` function (builds the broken scratch
# repo, echoes its path) and an `invoke_*` function (runs the real gate
# against it as its own last command). `run_case` calls `setup_*` plainly,
# under this script's own `set -e` in full force, so a mistake in the setup
# itself — a typo'd path, a `cp` of a file that does not exist — aborts the
# whole suite immediately and visibly, the same way a bug in any other
# script here would. Only `invoke_*`'s exit status is caught and interpreted
# as the case's verdict. Found by review: an earlier version ran setup and
# the gate invocation as one function, judged by that function's single
# exit code — a setup failure and the gate's own deliberate failure are both
# non-zero, and once one function's worth of commands can fail for either
# reason, "exit code is non-zero" stops meaning "the gate caught the
# defect." Splitting the two means a case can only ever pass because the
# real gate script rejected the real broken artifact, never because
# something upstream of it broke instead.
# `lib.sh` sets `set -e`, but bash does not propagate `errexit` into a
# command substitution's subshell by default -- every `setup_*` function
# below is invoked as `dir="$(setup_fn)"`, and without this, a failing
# command inside `setup_fn` (a bad `cp`, a typo'd path) would not abort
# that subshell; execution would fall through to `setup_fn`'s final
# `printf` and the substitution would report success regardless. Found
# empirically while fixing the setup/invoke split above: splitting the two
# functions was not, on its own, sufficient -- `dir="$(setup_fn)"` silently
# swallowed a deliberately injected setup failure until this was added.
shopt -s inherit_errexit

source "$(dirname "${BASH_SOURCE[0]}")/../../scripts/lib.sh"

cd "$REPO_ROOT"

SCRATCH_ROOT="$(mktemp -d)"
trap 'rm -rf "$SCRATCH_ROOT"' EXIT

# new_scratch <name>: a fresh, isolated git repo under SCRATCH_ROOT with
# lib.sh already in place. Echoes its path.
new_scratch() {
  local dir="$SCRATCH_ROOT/$1"
  mkdir -p "$dir/scripts"
  cp "$REPO_ROOT/scripts/lib.sh" "$dir/scripts/lib.sh"
  git -C "$dir" init -q
  git -C "$dir" config user.email test@test.com
  git -C "$dir" config user.name test
  printf '%s\n' "$dir"
}

# copy_gate <dir> <script-name>: brings one real gate script into a scratch
# repo, unmodified, so this suite tests the actual file rather than a copy
# of its logic.
copy_gate() {
  cp "$REPO_ROOT/scripts/$2" "$1/scripts/$2"
  chmod +x "$1/scripts/$2"
}

TOTAL=0
FAILED_CASES=0

# run_case <gate-label> <setup-fn> <invoke-fn>: calls setup-fn plainly (a
# failure there aborts this whole suite, under the script-wide `set -e` --
# see the header), then calls invoke-fn with the resulting scratch dir as
# $1 and checks *that* command's exit code, and only that one, is non-zero.
run_case() {
  local label="$1" setup_fn="$2" invoke_fn="$3" rc=0
  TOTAL=$((TOTAL + 1))
  local dir
  dir="$("$setup_fn")"
  # `|| rc=$?`, not a bare call: invoke-fn's whole point is to end in a
  # failing command (the gate under test, against the broken artifact
  # setup-fn just built), and a bare call here would abort the suite at the
  # first case under this file's own `set -e` rather than letting it report
  # and continue. Unlike setup-fn above, invoke-fn is *just* the one gate
  # invocation, so this suppression is scoped to exactly the command whose
  # exit code is the thing being tested, not to any setup step upstream of
  # it.
  "$invoke_fn" "$dir" >/tmp/negative-gate-output.$$ 2>&1 || rc=$?
  if (( rc != 0 )); then
    ok "$label fails on a broken artifact (exit $rc)"
  else
    fail "$label reported ok on a broken artifact -- this is what the suite exists to catch"
    note "captured output:"
    sed 's/^/     /' /tmp/negative-gate-output.$$ >&2
    FAILED_CASES=$((FAILED_CASES + 1))
  fi
  rm -f /tmp/negative-gate-output.$$
}

# --- check-commit-msg.sh: a subject with no backlog task ID -----------------
setup_commit_msg() {
  local dir; dir="$(new_scratch commit-msg)"
  copy_gate "$dir" check-commit-msg.sh
  mkdir -p "$dir/docs/internal/product"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | todo |
EOF
  echo "not a valid subject at all" > "$dir/msg.txt"
  printf '%s\n' "$dir"
}
invoke_commit_msg() {
  bash "$1/scripts/check-commit-msg.sh" "$1/msg.txt"
}

# --- known_task_ids: a task id only in the working tree, unstaged ----------
setup_commit_msg_unstaged_row() {
  local dir; dir="$(new_scratch commit-msg-unstaged-row)"
  copy_gate "$dir" check-commit-msg.sh
  mkdir -p "$dir/docs/internal/product"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | todo |
EOF
  (cd "$dir" && git add -A)
  # Added to the working tree *after* staging, so the index still holds only
  # M-1.1 -- `known_task_ids` (`M-1.39`) must read the index, not this file
  # on disk, or a commit subject naming this row would wrongly pass locally
  # and fail the identical gate on CI, which only ever sees the index.
  cat >> "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-9.9 | an unstaged task | some criterion | todo |
EOF
  echo "M-9.9: a change whose task only exists in the working tree" > "$dir/msg.txt"
  printf '%s\n' "$dir"
}
invoke_commit_msg_unstaged_row() {
  bash "$1/scripts/check-commit-msg.sh" "$1/msg.txt"
}

# --- check-tests-kept.sh: a test removed with no Removes-test: trailer -----
setup_tests_kept() {
  local dir; dir="$(new_scratch tests-kept)"
  copy_gate "$dir" check-tests-kept.sh
  mkdir -p "$dir/src"
  cat > "$dir/src/lib.rs" <<'EOF'
pub fn add(a: i32, b: i32) -> i32 { a + b }

#[test]
fn add_works() {
    assert_eq!(add(1, 2), 3);
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: add a function with a test")
  cat > "$dir/src/lib.rs" <<'EOF'
pub fn add(a: i32, b: i32) -> i32 { a + b }
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: quietly drop the test, no trailer")
  printf '%s\n' "$dir"
}
invoke_tests_kept() {
  bash "$1/scripts/check-tests-kept.sh"
}

# --- check-drift.sh: a threshold read from the environment ------------------
setup_drift() {
  local dir; dir="$(new_scratch drift)"
  copy_gate "$dir" check-drift.sh
  mkdir -p "$dir/scripts"
  # Built from fragments, not written as one literal line: the combined text
  # is exactly the pattern check-drift.sh looks for, and check-drift.sh scans
  # every tracked *.sh file in the real repo -- this one included. A literal
  # copy here would trip the real gate on its own fixture the same way
  # check-drift.sh once tripped on its own header's worked examples (M-1.7's
  # retrospective). `dollar` breaks the `${...:-...}` syntax across two
  # source lines so no single line here contains both signals check-drift.sh
  # looks for; the two fragments still concatenate to the exact original
  # string at runtime, which is what lands in the scratch repo's budget.sh
  # and is what check-drift.sh, run against *that* file, is meant to catch.
  local dollar='$'
  local budget_expr="${dollar}{OQUEUE_BUDGET_SECONDS:-120}"
  {
    echo '#!/usr/bin/env bash'
    printf 'BUDGET_SECONDS="%s"\n' "$budget_expr"
  } > "$dir/scripts/budget.sh"
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a settable budget")
  printf '%s\n' "$dir"
}
invoke_drift() {
  bash "$1/scripts/check-drift.sh"
}

# --- check-layering.sh: a leaf crate depending on a sibling leaf crate ------
setup_layering() {
  local dir; dir="$(new_scratch layering)"
  copy_gate "$dir" check-layering.sh
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf", "crates/oqueue-codec"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-buf/src" "$dir/crates/oqueue-codec/src"
  cat > "$dir/crates/oqueue-buf/Cargo.toml" <<'EOF'
[package]
name = "oqueue-buf"
version = "0.1.0"
edition = "2021"

[dependencies]
oqueue-codec = { path = "../oqueue-codec" }
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  cat > "$dir/crates/oqueue-codec/Cargo.toml" <<'EOF'
[package]
name = "oqueue-codec"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn g() {}' > "$dir/crates/oqueue-codec/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a leaf crate depending on a sibling leaf")
  printf '%s\n' "$dir"
}
invoke_layering() {
  bash "$1/scripts/check-layering.sh"
}

# --- check-sans-io.sh: a concrete socket type in a library crate -----------
setup_sans_io() {
  local dir; dir="$(new_scratch sans-io)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-buf/src"
  cat > "$dir/crates/oqueue-buf/src/lib.rs" <<'EOF'
use std::net::TcpStream;

pub fn connect() -> std::io::Result<TcpStream> {
    TcpStream::connect("127.0.0.1:9092")
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a library crate that names TcpStream directly")
  printf '%s\n' "$dir"
}
invoke_sans_io() {
  bash "$1/scripts/check-sans-io.sh"
}

# --- check-core-contract.sh: a trait's method set changes, no ADR ----------
setup_core_contract() {
  local dir; dir="$(new_scratch core-contract)"
  copy_gate "$dir" check-core-contract.sh
  # has_rust() gates on a root Cargo.toml existing; without one this gate
  # skips cleanly rather than examining anything.
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub trait Store {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: define the Store trait")
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub trait Store {
    fn get(&self, key: &str, epoch: u64) -> Option<Vec<u8>>;
}
EOF
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_core_contract() {
  bash "$1/scripts/check-core-contract.sh"
}

# --- check-unsafe.sh: unsafe outside the three named crates -----------------
setup_unsafe() {
  local dir; dir="$(new_scratch unsafe)"
  copy_gate "$dir" check-unsafe.sh
  mkdir -p "$dir/baselines" "$dir/crates/oqueue-broker/src"
  cat > "$dir/baselines/unsafe.txt" <<'EOF'
# empty
EOF
  cat > "$dir/crates/oqueue-broker/src/lib.rs" <<'EOF'
fn evil() {
    unsafe { std::hint::unreachable_unchecked(); }
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: unsafe outside the three allowed crates")
  printf '%s\n' "$dir"
}
invoke_unsafe() {
  bash "$1/scripts/check-unsafe.sh"
}

# --- check-reviewed.sh: a staged change with no recorded verdict -----------
setup_reviewed() {
  local dir; dir="$(new_scratch reviewed)"
  copy_gate "$dir" check-reviewed.sh
  echo "some content" > "$dir/file.txt"
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_reviewed() {
  bash "$1/scripts/check-reviewed.sh"
}

# --- check-reviewed.sh: a review artifact whose task_id is a regex ---------
setup_reviewed_regex_task_id() {
  local dir; dir="$(new_scratch reviewed-regex-task-id)"
  copy_gate "$dir" check-reviewed.sh
  mkdir -p "$dir/docs/internal/product" "$dir/target/review"
  # A real backlog with one real task id -- `.*` is not one of them, so a
  # `task_id` of `.*` must be rejected. Before `M-1.38`'s fix, `grep -qx`
  # (no `-F`) read it as a regex instead of a literal string and matched
  # every row, including this one.
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | done |
EOF
  echo "some content" > "$dir/file.txt"
  (cd "$dir" && git add -A)
  # Portable sha256, the same fallback lib.sh's own sha256_stdin uses --
  # this file deliberately does not source lib.sh (see the header), so it
  # cannot call that helper directly.
  local h
  if command -v sha256sum >/dev/null 2>&1; then
    h="$(cd "$dir" && git diff --cached | sha256sum | cut -d' ' -f1)"
  else
    h="$(cd "$dir" && git diff --cached | shasum -a 256 | cut -d' ' -f1)"
  fi
  cat > "$dir/target/review/$h.json" <<EOF
{"task_id": ".*", "diff_sha256": "$h", "reviewer": "fixture", "verdict": "pass", "findings": []}
EOF
  printf '%s\n' "$dir"
}
invoke_reviewed_regex_task_id() {
  bash "$1/scripts/check-reviewed.sh"
}

# --- check-milestone-review.sh: a commit no review artifact covers ---------
setup_milestone_review() {
  local dir; dir="$(new_scratch milestone-review)"
  copy_gate "$dir" check-milestone-review.sh
  mkdir -p "$dir/docs/internal/product"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | done |
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a change with no covering review artifact")
  printf '%s\n' "$dir"
}
invoke_milestone_review() {
  bash "$1/scripts/check-milestone-review.sh" --milestone M-1
}

# --- build-index.sh --check: a generated region that does not match its
# source -------------------------------------------------------------------
setup_build_index() {
  local dir; dir="$(new_scratch build-index)"
  copy_gate "$dir" build-index.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/researches"
  cat > "$dir/docs/internal/standards/example.md" <<'EOF'
---
title: "example"
description: "an example standard"
tags: [process]
---

Body.
EOF
  cat > "$dir/AGENTS.md" <<'EOF'
# example

<!-- index:standards:start -->
this table is stale and does not name example.md
<!-- index:standards:end -->

<!-- index:skills:start -->
<!-- index:skills:end -->
EOF
  cat > "$dir/docs/researches/README.md" <<'EOF'
# research index

<!-- index:tags:start -->
<!-- index:tags:end -->

<!-- index:count:start -->
<!-- index:count:end -->
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a standard added, index never regenerated")
  printf '%s\n' "$dir"
}
invoke_build_index() {
  bash "$1/scripts/build-index.sh" --check
}

# --- check-requirements-trace.sh: a milestone plan citing a requirement that
# does not exist -------------------------------------------------------------
setup_requirements_trace() {
  local dir; dir="$(new_scratch requirements-trace)"
  copy_gate "$dir" check-requirements-trace.sh
  mkdir -p "$dir/docs/internal/product/milestones"
  cat > "$dir/docs/internal/product/requirements.md" <<'EOF'
| ID | Requirement | Verification | Status |
| --- | --- | --- | --- |
| FR-1 | does a thing | a test | agreed |
EOF
  cat > "$dir/docs/internal/product/milestones/M1.md" <<'EOF'
**Serves:** FR-999
EOF
  # A roadmap.md whose Requirement coverage row *agrees* with the plan above
  # (both say FR-999) -- so the roadmap-agreement check the gate also runs
  # passes cleanly, and this case exercises only the failure it is named
  # for: an id the plan cites that requirements.md does not list. Without
  # this file at all, the gate's own "roadmap.md not found" check fires
  # first and this case would pass for an unrelated reason -- found by
  # review, which built a mutant of the gate with the unknown-id check
  # deleted and confirmed it still failed identically against the fixture
  # as it stood before this file was added.
  cat > "$dir/docs/internal/product/roadmap.md" <<'EOF'
## Requirement coverage

| Milestone | Serves |
|---|---|
| M1 | FR-999 |
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a plan cites a requirement that does not exist")
  printf '%s\n' "$dir"
}
invoke_requirements_trace() {
  bash "$1/scripts/check-requirements-trace.sh"
}

# --- check-file-size.sh: a file over the 500-line limit ---------------------
setup_file_size() {
  local dir; dir="$(new_scratch file-size)"
  copy_gate "$dir" check-file-size.sh
  mkdir -p "$dir/crates/oqueue-x/src"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-x"]
resolver = "2"
EOF
  cat > "$dir/crates/oqueue-x/Cargo.toml" <<'EOF'
[package]
name = "oqueue-x"
version = "0.1.0"
edition = "2021"
EOF
  # `printf`'s format-string cycling, not `yes | head`: a pipe whose reader
  # closes early (`head`) sends the writer (`yes`) SIGPIPE, and this file's
  # own `pipefail` (inherited from lib.sh) would treat the writer's death as
  # the pipeline's failure -- aborting this setup function under `set -e`
  # before it ever got to commit a fixture, the exact idiom `M-1.44` exists
  # to document. `printf` with a `%.0s` filler argument repeats the format
  # string once per argument and involves no pipe at all.
  printf 'pub fn f() {}\n%.0s' {1..501} > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a file over the 500-line limit")
  printf '%s\n' "$dir"
}
invoke_file_size() {
  bash "$1/scripts/check-file-size.sh"
}

# --- check-readmes.sh: a crate whose README Upstream section is stale ------
setup_readmes() {
  local dir; dir="$(new_scratch readmes)"
  copy_gate "$dir" check-readmes.sh
  mkdir -p "$dir/crates/oqueue-x/src"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-x"]
resolver = "2"
EOF
  cat > "$dir/crates/oqueue-x/Cargo.toml" <<'EOF'
[package]
name = "oqueue-x"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1" }
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  cat > "$dir/crates/oqueue-x/README.md" <<'EOF'
# oqueue-x

## Upstream

Nothing documented here, and Cargo.toml depends on tokio.

## Downstream

Nobody yet.
EOF
  cat > "$dir/crates/oqueue-x/AGENTS.md" <<'EOF'
notes
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: Upstream section does not mention a real dependency")
  printf '%s\n' "$dir"
}
invoke_readmes() {
  bash "$1/scripts/check-readmes.sh"
}

# --- check-hot-path-bench.sh: a marker names a path not in the table -------
setup_hot_path_bench() {
  local dir; dir="$(new_scratch hot-path-bench)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/crates/oqueue-x/benches"
  # A minimal table, not the real one -- this gate reads whatever
  # performance.md the tree it runs against has, so the fixture only needs
  # the shape (indented GFM rows inside a numbered list item, per the real
  # file) and one real row for the marker below to *not* match.
  cat > "$dir/docs/internal/standards/performance.md" <<'EOF'
## Hot-path benchmarks

18. **Every hot path has a benchmark**, added with the code rather than after
    it. The hot paths, and each one's benchmark obligation:

    | Path | Benchmark |
    |---|---|
    | RecordBatch encode / decode | `bench-micro`, gated |

19. **A hot path without a benchmark is an unmeasured claim.**
EOF
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-x"]
resolver = "2"
EOF
  cat > "$dir/crates/oqueue-x/Cargo.toml" <<'EOF'
[package]
name = "oqueue-x"
version = "0.1.0"
edition = "2021"
EOF
  # Close, but not the table's exact wording -- the drift this gate exists
  # to catch: a marker whose name fell out of sync with the table it cites.
  cat > "$dir/crates/oqueue-x/benches/bench_micro.rs" <<'EOF'
// hot-path: RecordBatch encode/decode
fn bench_recordbatch() {}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.29: a hot-path marker names no real row")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a row is required (not in NOT_YET_BUILT) and
# has no marker anywhere ------------------------------------------------
setup_hot_path_bench_required() {
  local dir; dir="$(new_scratch hot-path-bench-required)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/crates/oqueue-x/src"
  # All eight of `NOT_YET_BUILT`'s real names, verbatim, plus one row it
  # does not name. Without all eight present, every real entry becomes
  # "stale" against this fixture's table and the gate fails on *that*
  # instead -- found by running this fixture against the real script before
  # trusting it: the first draft, with only the ninth row, failed for the
  # stale-allowlist reason, not the required-row reason this case is named
  # for, the exact "fixture fails for the wrong reason" bug M-1.24's round 2
  # review found in a different gate's fixture.
  cat > "$dir/docs/internal/standards/performance.md" <<'EOF'
## Hot-path benchmarks

18. **Every hot path has a benchmark**, added with the code rather than after
    it. The hot paths, and each one's benchmark obligation:

    | Path | Benchmark |
    |---|---|
    | RecordBatch encode / decode | `bench-micro`, gated |
    | CRC-32C over representative sizes | `bench-micro`, gated + known-answer test |
    | Varint decode (and the paths that avoid it) | `bench-micro`, gated |
    | Offset→object index lookup | `bench-micro`, gated |
    | Buffer allocation and pooling | `bench-micro` + heap profile |
    | Produce path end to end | `bench-macro`, report only |
    | Fetch: tail (cached) and cold (ranged GET) | `bench-macro`, report only |
    | Compaction throughput | `bench-macro`, report only |
    | A ninth path NOT_YET_BUILT does not name | `bench-micro`, gated |

19. **A hot path without a benchmark is an unmeasured claim.**
EOF
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-x"]
resolver = "2"
EOF
  cat > "$dir/crates/oqueue-x/Cargo.toml" <<'EOF'
[package]
name = "oqueue-x"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.29: a required hot path has no benchmark and no allowlist entry")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_required() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a row is covered but NOT_YET_BUILT still
# lists it -- the loophole that let a later regression go unnoticed --------
setup_hot_path_bench_leftover() {
  local dir; dir="$(new_scratch hot-path-bench-leftover)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/crates/oqueue-x/benches"
  # A marker for a row the real, unmodified script's `NOT_YET_BUILT` already
  # names -- exactly what landing that row's milestone and adding its
  # benchmark produces, if the entry is not also deleted in the same
  # commit. Without this check, this state passes silently and a later
  # regression (the marker removed again) would too, since the leftover
  # entry keeps protecting the row forever -- found by round 3 review
  # reproducing the sequence, not by inspection.
  cat > "$dir/docs/internal/standards/performance.md" <<'EOF'
## Hot-path benchmarks

18. **Every hot path has a benchmark**, added with the code rather than after
    it. The hot paths, and each one's benchmark obligation:

    | Path | Benchmark |
    |---|---|
    | RecordBatch encode / decode | `bench-micro`, gated |
    | CRC-32C over representative sizes | `bench-micro`, gated + known-answer test |
    | Varint decode (and the paths that avoid it) | `bench-micro`, gated |
    | Offset→object index lookup | `bench-micro`, gated |
    | Buffer allocation and pooling | `bench-micro` + heap profile |
    | Produce path end to end | `bench-macro`, report only |
    | Fetch: tail (cached) and cold (ranged GET) | `bench-macro`, report only |
    | Compaction throughput | `bench-macro`, report only |

19. **A hot path without a benchmark is an unmeasured claim.**
EOF
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-x"]
resolver = "2"
EOF
  cat > "$dir/crates/oqueue-x/Cargo.toml" <<'EOF'
[package]
name = "oqueue-x"
version = "0.1.0"
edition = "2021"
EOF
  cat > "$dir/crates/oqueue-x/benches/bench_micro.rs" <<'EOF'
// hot-path: RecordBatch encode / decode
fn bench_recordbatch() {}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.29: a covered row still has a NOT_YET_BUILT entry")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_leftover() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-portability.sh: a vendor-syntax line in AGENTS.md ---------------
setup_portability() {
  local dir; dir="$(new_scratch portability)"
  copy_gate "$dir" check-portability.sh
  mkdir -p "$dir/.agents/skills/foo" "$dir/.claude/commands"
  # `@import` (Claude Code's own syntax) in AGENTS.md, the one concrete
  # vendor-syntax pattern the skill this gate enforces names by example.
  # Everything else in this fixture is deliberately clean, so this case
  # exercises only the failure it is named for.
  cat > "$dir/AGENTS.md" <<'EOF'
# Test repo

@some-other-file.md

Some prose.
EOF
  cat > "$dir/.agents/skills/foo/SKILL.md" <<'EOF'
---
name: foo
description: Do the foo thing.
---

# Foo

Some procedure.
EOF
  cat > "$dir/.claude/commands/foo.md" <<'EOF'
---
description: Do the foo thing.
---

Follow the procedure in `.agents/skills/foo/SKILL.md`. Read that file now
and do what it says.

This file is an adapter.
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.30: AGENTS.md uses vendor-specific @import syntax")
  printf '%s\n' "$dir"
}
invoke_portability() {
  bash "$1/scripts/check-portability.sh"
}

# --- check-portability.sh: an unterminated ``` fence ------------------------
setup_portability_unterminated_fence() {
  local dir; dir="$(new_scratch portability-unterminated-fence)"
  copy_gate "$dir" check-portability.sh
  mkdir -p "$dir/.agents/skills/foo" "$dir/.claude/commands"
  # A ``` marker with no closing partner. Without the fix this fixture
  # exists to hold in place, `strip_fenced_lines` never re-closes `in_fence`
  # and silently drops every line from the unclosed marker to end of file --
  # including the real `@` violation below -- from every scan, with no
  # diagnostic. Found by review reproducing it against a real file; this
  # fixture is the same construction, minimized.
  cat > "$dir/AGENTS.md" <<'EOF'
# Test repo

```
an unterminated fence

@some-other-file.md
EOF
  cat > "$dir/.agents/skills/foo/SKILL.md" <<'EOF'
---
name: foo
description: Do the foo thing.
---

# Foo
EOF
  cat > "$dir/.claude/commands/foo.md" <<'EOF'
---
description: Do the foo thing.
---

Follow the procedure in `.agents/skills/foo/SKILL.md`. Read that file now
and do what it says.
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.30: AGENTS.md has an unterminated fence hiding a real violation")
  printf '%s\n' "$dir"
}
invoke_portability_unterminated_fence() {
  bash "$1/scripts/check-portability.sh"
}

run_case "check-commit-msg.sh"          setup_commit_msg          invoke_commit_msg
run_case "check-commit-msg.sh (unstaged backlog row)" setup_commit_msg_unstaged_row invoke_commit_msg_unstaged_row
run_case "check-tests-kept.sh"          setup_tests_kept          invoke_tests_kept
run_case "check-drift.sh"               setup_drift               invoke_drift
run_case "check-layering.sh"            setup_layering            invoke_layering
run_case "check-sans-io.sh"             setup_sans_io             invoke_sans_io
run_case "check-core-contract.sh"       setup_core_contract       invoke_core_contract
run_case "check-unsafe.sh"              setup_unsafe              invoke_unsafe
run_case "check-reviewed.sh"            setup_reviewed            invoke_reviewed
run_case "check-reviewed.sh (regex task_id)" setup_reviewed_regex_task_id invoke_reviewed_regex_task_id
run_case "check-milestone-review.sh"    setup_milestone_review    invoke_milestone_review
run_case "build-index.sh --check"       setup_build_index         invoke_build_index
run_case "check-requirements-trace.sh"  setup_requirements_trace  invoke_requirements_trace
run_case "check-file-size.sh"           setup_file_size           invoke_file_size
run_case "check-readmes.sh"             setup_readmes             invoke_readmes
run_case "check-hot-path-bench.sh"      setup_hot_path_bench      invoke_hot_path_bench
run_case "check-hot-path-bench.sh (required row)" setup_hot_path_bench_required invoke_hot_path_bench_required
run_case "check-hot-path-bench.sh (leftover entry)" setup_hot_path_bench_leftover invoke_hot_path_bench_leftover
run_case "check-portability.sh"         setup_portability         invoke_portability
run_case "check-portability.sh (unterminated fence)" setup_portability_unterminated_fence invoke_portability_unterminated_fence

note "$TOTAL gate(s) exercised, $FAILED_CASES failed to fail as expected"

finish
