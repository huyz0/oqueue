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

# ⚠️ **The cases that did not run, named by the gate they cover.** `M0.26`.
# Two cases are registered only where their tool is installed, which is right —
# a missing tool is a skip, never a failure — but until now the *fact* left no
# trace a caller could read. `m0-complete.sh` printed "every gate fails on a
# broken artifact" from this suite's exit code, so on a machine without
# `cargo-llvm-cov` and `cargo-mutants` it declared half of M0's new gates
# complete having never watched either fail. That is the skip-is-a-pass shape
# the same script refuses for `cargo`, with a note saying why.
SKIPPED_GATES=()

# skip_case <gate.sh> <reason> [remedy]: report a case that could not run, and
# record which gate is thereby unproven. ⚠️ The gate name is the machine-
# readable part: a caller asking "was check-mutants.sh watched to fail?" needs
# a name, not prose.
skip_case() {
  local gate="$1" reason="$2" remedy="${3:-}"
  SKIPPED_GATES+=("$gate")
  skip "$gate -- $reason"
  [[ -z "$remedy" ]] || note "$remedy"
  note "⚠️ this case did not run; $gate is unproven on this machine"
}

# run_case <gate-label> <setup-fn> <invoke-fn> [expected-substring]: calls
# setup-fn plainly (a failure there aborts this whole suite, under the
# script-wide `set -e` -- see the header), then calls invoke-fn with the
# resulting scratch dir as $1 and checks *that* command's exit code, and only
# that one, is non-zero.
#
# ⚠️ **The fourth argument is what stops a case going vacuously green**, and
# `M0.21` is where that stopped being hypothetical. A gate that checks four
# properties fails a fixture planting any one of them, so "exit non-zero"
# stopped distinguishing the defect the fixture plants from an unrelated one
# it happens to also have -- `check-layering.sh`'s two existing fixtures
# tripped the new `[lints]` assertion, and would have passed with the layering
# check deleted outright. Where it is given, the case additionally requires
# the gate to have *said* what it caught. It is optional because most gates
# check one thing, and the day one of those grows a second property is the day
# its case needs this too.
run_case() {
  local label="$1" setup_fn="$2" invoke_fn="$3" expect="${4:-}" rc=0
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
  if (( rc != 0 )) && [[ -n "$expect" ]] && ! grep -qF -- "$expect" /tmp/negative-gate-output.$$; then
    fail "$label failed, but not for the reason the fixture plants"
    note "expected the output to contain: $expect"
    note "captured output:"
    sed 's/^/     /' /tmp/negative-gate-output.$$ >&2
    FAILED_CASES=$((FAILED_CASES + 1))
  elif (( rc != 0 )); then
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
  # ⚠️ Otherwise valid in every respect this gate checks *except* the one the
  # case is named for -- see `run_case`'s fourth argument.
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf", "crates/oqueue-codec"]
resolver = "2"

[profile.release]
overflow-checks = true
EOF
  mkdir -p "$dir/crates/oqueue-buf/src" "$dir/crates/oqueue-codec/src"
  cat > "$dir/crates/oqueue-buf/Cargo.toml" <<'EOF'
[package]
name = "oqueue-buf"
version = "0.1.0"
edition = "2021"

[dependencies]
oqueue-codec = { path = "../oqueue-codec" }

[lints]
workspace = true
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  cat > "$dir/crates/oqueue-codec/Cargo.toml" <<'EOF'
[package]
name = "oqueue-codec"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true
EOF
  echo 'pub fn g() {}' > "$dir/crates/oqueue-codec/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: a leaf crate depending on a sibling leaf")
  printf '%s\n' "$dir"
}
invoke_layering() {
  bash "$1/scripts/check-layering.sh"
}

# --- check-layering.sh: a non-UTF-8 Cargo.toml, the crash path -------------
#
# `M-1.51`. Distinct from the violation case above: `package_name()`'s
# `read_text()` raises `UnicodeDecodeError` before any manifest is even
# parsed for dependencies, exercising the crash-safety wrapper `M-1.45`
# added rather than the ordinary "stray dependency" path.
setup_layering_non_utf8() {
  local dir; dir="$(new_scratch layering-non-utf8)"
  copy_gate "$dir" check-layering.sh
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf"]
resolver = "2"

[profile.release]
overflow-checks = true
EOF
  mkdir -p "$dir/crates/oqueue-buf/src"
  printf '[package]\nname = "oqueue-buf"\nversion = "0.1.0"\nedition = "2021"\n# \xff\xfe bad byte\n' \
    > "$dir/crates/oqueue-buf/Cargo.toml"
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.51: a non-UTF-8 byte in a Cargo.toml")
  printf '%s\n' "$dir"
}
invoke_layering_non_utf8() {
  bash "$1/scripts/check-layering.sh"
}

# --- check-layering.sh: a member manifest with no [lints] workspace --------
#
# `M0.21`. `rust-style.md` rule 3's whole enforcement is one line per manifest;
# omitting it opts the crate out of pedantic, nursery and the `unwrap_used` ban
# with nothing else changing and every other gate still green.
setup_layering_no_lints() {
  local dir; dir="$(new_scratch layering-no-lints)"
  copy_gate "$dir" check-layering.sh
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf"]
resolver = "2"

[profile.release]
overflow-checks = true
EOF
  mkdir -p "$dir/crates/oqueue-buf/src"
  cat > "$dir/crates/oqueue-buf/Cargo.toml" <<'EOF'
[package]
name = "oqueue-buf"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M0.21: a manifest that opts out of the workspace lints")
  printf '%s\n' "$dir"
}

# --- check-layering.sh: a [profile] section in a member manifest -----------
#
# `M0.21`. ⚠️ Cargo **ignores** this and exits 0 with a warning on stderr, so
# the manifest says one thing and the build does another -- the failure mode is
# a profile setting that looks configured and is not.
setup_layering_member_profile() {
  local dir; dir="$(new_scratch layering-member-profile)"
  copy_gate "$dir" check-layering.sh
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf"]
resolver = "2"

[profile.release]
overflow-checks = true
EOF
  mkdir -p "$dir/crates/oqueue-buf/src"
  cat > "$dir/crates/oqueue-buf/Cargo.toml" <<'EOF'
[package]
name = "oqueue-buf"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true

[profile.release]
overflow-checks = true
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M0.21: a member manifest carrying a profile cargo ignores")
  printf '%s\n' "$dir"
}

# --- check-layering.sh: a release profile without overflow-checks ----------
#
# `M0.21`, and ⚠️ this is `security.md` rule 4's named gate, which did not
# exist until here. The fixture is the exact shape rule 4 cites: the release
# build wraps where the debug build panics, so every test passes and the
# shipping binary is the one with the wrapping length offset.
setup_layering_no_overflow_checks() {
  local dir; dir="$(new_scratch layering-no-overflow)"
  copy_gate "$dir" check-layering.sh
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-buf"]
resolver = "2"

[profile.release]
lto = "fat"
EOF
  mkdir -p "$dir/crates/oqueue-buf/src"
  cat > "$dir/crates/oqueue-buf/Cargo.toml" <<'EOF'
[package]
name = "oqueue-buf"
version = "0.1.0"
edition = "2021"

[lints]
workspace = true
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-buf/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M0.21: a release profile with no overflow-checks")
  printf '%s\n' "$dir"
}

# --- check-readmes.sh: bin/oqueue, the crate that was excluded ------------
#
# `M0.21` widened the glob to `bin/*/`. ⚠️ **This case is what makes the
# widening load-bearing** rather than a comment: with the glob back at
# `crates/*/` the fixture below produces no manifests at all and the gate
# reports its own SKIP, which is exit 0 and this suite's failure.
setup_readmes_bin() {
  local dir; dir="$(new_scratch readmes-bin)"
  copy_gate "$dir" check-readmes.sh
  mkdir -p "$dir/bin/oqueue/src"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["bin/oqueue"]
resolver = "2"
EOF
  cat > "$dir/bin/oqueue/Cargo.toml" <<'EOF'
[package]
name = "oqueue"
version = "0.1.0"
edition = "2021"

[dependencies]
mimalloc = { version = "0.1" }
EOF
  echo 'fn main() {}' > "$dir/bin/oqueue/src/main.rs"
  cat > "$dir/bin/oqueue/README.md" <<'EOF'
# oqueue

## Upstream

Nothing documented here, and Cargo.toml depends on mimalloc.

## Downstream

Nobody yet.
EOF
  echo notes > "$dir/bin/oqueue/AGENTS.md"
  (cd "$dir" && git add -A && git commit -q -m "M0.21: bin/oqueue Upstream does not mention a real dependency")
  printf '%s\n' "$dir"
}
invoke_readmes_bin() {
  bash "$1/scripts/check-readmes.sh"
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

# --- check-core-contract.sh: a non-UTF-8 tracked .rs file, the crash path --
#
# `M-1.51`. `extract_impl_files_by_trait()` scans every `*.rs` file
# `git ls-files` returns via `git show`, not only files that mention the
# changed trait -- so the crash comes from a file with nothing to do with
# `Store`, exercising the crash-safety wrapper `M-1.45` added rather than
# the "no ADR" path the case above already covers.
setup_core_contract_non_utf8() {
  local dir; dir="$(new_scratch core-contract-non-utf8)"
  copy_gate "$dir" check-core-contract.sh
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
  printf 'pub struct Other;\n// \xff\xfe bad byte\n' > "$dir/crates/oqueue-core/src/other.rs"
  (cd "$dir" && git add -A && git commit -q -m "M-1.51: define Store, and a non-UTF-8 byte in a tracked .rs file")
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub trait Store {
    fn get(&self, key: &str, epoch: u64) -> Option<Vec<u8>>;
}
EOF
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_core_contract_non_utf8() {
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

# --- check-unsafe.sh: a non-UTF-8 file does not suppress a real violation --
#
# `M-1.53`. ⚠️ **Not a crash-path case, unlike its siblings for
# `check-layering.sh`/`check-core-contract.sh` (`M-1.51`) — the acceptance
# criterion that asked for "the same shape" turned out not to be
# satisfiable, and this is the deviation, made explicitly rather than
# forced.** Probed directly before writing this fixture: a non-UTF-8 byte in
# a tracked `.rs` file's *content*, in its *filename*, and in
# `baselines/unsafe.txt` itself, none of them crash `check-unsafe.sh` — each
# is already caught by the per-file `except (OSError, UnicodeDecodeError)`
# guard (content, and a mis-decoded filename lands on `FileNotFoundError`,
# also `OSError`) or by `os.environ`'s own `surrogateescape` decoding
# (baseline text), and reported as a `warn`, not a crash. That guard did not
# exist in `check-layering.sh`/`check-core-contract.sh` before `M-1.45`,
# which is exactly why a bare non-UTF-8 byte crashed *them* and does not
# crash this script — `check-unsafe.sh` was already more defensive, not
# less. No small, realistic fixture was found that reaches this script's
# outer `try`/`except Exception` wrapper at all; every path a non-UTF-8 byte
# can take is already handled before reaching it.
#
# What *is* real and untested: whether an unreadable file silently stops the
# scan before it reaches a real violation elsewhere in the tree, or is
# skipped without disturbing it. This fixture combines both in one tree, and
# — checked against `git ls-files`' own return order, not assumed — the
# unreadable file's path (`crates/aaa-bad/...`) sorts *before* the real
# violation's (`crates/oqueue-broker/...`), so the per-file loop reaches the
# `warn`-and-`continue` before it ever reaches the real violation. A path
# ordering where the violation came first would let this case pass by
# accident regardless of whether the scan actually continues past a skip.
setup_unsafe_non_utf8() {
  local dir; dir="$(new_scratch unsafe-non-utf8)"
  copy_gate "$dir" check-unsafe.sh
  mkdir -p "$dir/baselines" "$dir/crates/aaa-bad/src" "$dir/crates/oqueue-broker/src"
  cat > "$dir/baselines/unsafe.txt" <<'EOF'
# empty
EOF
  printf 'pub fn f() {}\n// \xff\xfe bad byte\n' > "$dir/crates/aaa-bad/src/bad.rs"
  cat > "$dir/crates/oqueue-broker/src/lib.rs" <<'EOF'
fn evil() {
    unsafe { std::hint::unreachable_unchecked(); }
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.53: a non-UTF-8 file scanned before a real violation")
  printf '%s\n' "$dir"
}
invoke_unsafe_non_utf8() {
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

# --- check-portability.sh: an index file claiming a present script is missing
#
# `M0.24`. ⚠️ `M0.1` was a whole task written to correct exactly this in these
# two files, and both were false again by the end of `M0` — while `M0.2` and
# `M0.17` were writing the scripts the README went on calling unwritten. A
# prose claim about what is on disk is one a script can hold; the reason it
# needs to is that nobody re-reads an index file they have read once.
setup_portability_stale_claim() {
  local dir; dir="$(new_scratch portability-stale)"
  copy_gate "$dir" check-portability.sh
  mkdir -p "$dir/.agents/skills/tdd" "$dir/scripts" "$dir/docs/internal/standards"
  cat > "$dir/.agents/skills/tdd/SKILL.md" <<'EOF'
---
name: tdd
description: Use when implementing a task test-first.
---

# TDD

Run `scripts/check-crate.sh` when the test is green.
EOF
  printf '#!/usr/bin/env bash
true
' > "$dir/scripts/check-crate.sh"
  chmod +x "$dir/scripts/check-crate.sh"
  cat > "$dir/.agents/skills/README.md" <<'EOF'
# Skills

A few scripts these skills invoke are still unwritten, so a skill that says
"run the gate" describes an intended step rather than an available one.
EOF
  echo "# oqueue" > "$dir/AGENTS.md"
  (cd "$dir" && git add -A && git commit -q -m "M0.24: an index file calling a present script missing")
  printf '%s\n' "$dir"
}
invoke_portability_stale_claim() {
  bash "$1/scripts/check-portability.sh"
}

# --- m-1-complete.sh: AGENTS.md missing its ## Non-negotiables section ------
#
# `M-1.46`. `copy_gate`'s `<script-name>` argument doubles as the path under
# `scripts/`, so `gates/m-1-complete.sh` copies the real nested script --
# the directory it needs, `$dir/scripts/gates`, is made first since
# `new_scratch` only creates `$dir/scripts`.
setup_m1_complete_missing_section() {
  local dir; dir="$(new_scratch m1-complete-missing-section)"
  mkdir -p "$dir/scripts/gates"
  copy_gate "$dir" gates/m-1-complete.sh
  cat > "$dir/AGENTS.md" <<'EOF'
# Test repo

## Start here

Nothing here.

## Never

Never do bad things.
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.46: AGENTS.md has no Non-negotiables section")
  printf '%s\n' "$dir"
}
invoke_m1_complete_missing_section() {
  bash "$1/scripts/gates/m-1-complete.sh"
}

# --- m-1-complete.sh: non-UTF-8 AGENTS.md bytes, the crash path -------------
#
# Distinct from the case above: that one exercises the ordinary "problems
# found" path (`sys.exit(2)`, a `PROBLEM` line naming what's wrong with the
# section). This one exercises the crash-safety wrapper `M-1.16` gave this
# script from the start -- a non-UTF-8 byte makes Python's own
# `open(path, encoding="utf-8").read()` raise `UnicodeDecodeError`, uncaught
# inside `build()`, caught by the `try`/`except Exception` around it, and
# mapped to exit 3 and a `PROBLEM the AGENTS.md parser raised ...` line
# rather than a bare traceback the caller can't distinguish from any other
# non-zero exit.
setup_m1_complete_non_utf8() {
  local dir; dir="$(new_scratch m1-complete-non-utf8)"
  mkdir -p "$dir/scripts/gates"
  copy_gate "$dir" gates/m-1-complete.sh
  printf '# Test repo\n\n## Non-negotiables\n\n1. A rule with a bad byte: \xff\xfe.\n' > "$dir/AGENTS.md"
  (cd "$dir" && git add -A && git commit -q -m "M-1.46: AGENTS.md has a non-UTF-8 byte")
  printf '%s\n' "$dir"
}
invoke_m1_complete_non_utf8() {
  bash "$1/scripts/gates/m-1-complete.sh"
}

# `M0.2`. `scripts/check-crate.sh` is the first gate in this suite that shells
# out to `cargo`, which makes it the first one whose scratch fixture has to be a
# *compilable* workspace rather than a plausible-looking tree of text. Three
# cases, one per check the script runs, because the script's own contract is
# that all three run even when an earlier one fails — a single fixture that
# breaks all three would pass while proving only that the first one works.
#
# ⚠️ Each fixture pins `edition = "2021"` and `resolver = "2"` rather than
# inheriting this repository's own choices. The point of a scratch fixture is
# that it exercises the gate, not that it mirrors the workspace; tying it to the
# real root manifest would make an unrelated edition bump fail these tests.
_crate_scratch() {
  local dir; dir="$(new_scratch "$1")"
  copy_gate "$dir" check-crate.sh

  # ⚠️ **Both of the following are written as files, not exported.** Every
  # `setup_*` here is called through `dir="$(setup_fn)"`, so its body runs in a
  # command-substitution subshell and any `export` is gone before `invoke_*`
  # runs the gate. A file in the fixture is read by cargo at invoke time, which
  # is the only moment that matters.

  # ⚠️ **The pinned toolchain, explicitly.** `SCRATCH_ROOT` is under `mktemp -d`,
  # outside the repository, so this repository's own `rust-toolchain.toml` does
  # not reach it and cargo runs whatever `rustup` last defaulted to. A default
  # without the `clippy` component makes all four cases below fail on a missing
  # subcommand — which is still a failure, so the suite still reports green
  # while testing nothing. That is the third distinct way this one fixture found
  # to be vacuously green; the other two are in the comment below. Found by
  # review.
  cp "$REPO_ROOT/rust-toolchain.toml" "$dir/rust-toolchain.toml"

  # ⚠️ **Build output under the repository's own `target/`, never the system
  # temp directory** — `build.md` rule 19. Four fixtures at ~7 MB each is not
  # much, but the rule exists because the system temp directory is a tmpfs on
  # some developer machines, and a suite that quietly fills it is a suite people
  # stop running.
  mkdir -p "$dir/.cargo"
  cat > "$dir/.cargo/config.toml" <<EOF
[build]
target-dir = "$REPO_ROOT/target/tmp/negative-$1"
EOF

  mkdir -p "$dir/crates/k/src"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/k"]
resolver = "2"
EOF
  cat > "$dir/crates/k/Cargo.toml" <<'EOF'
[package]
name = "k"
version = "0.0.0"
edition = "2021"
EOF
  # ⚠️ A placeholder source *before* the lockfile, then the lockfile. Both
  # matter, and both were got wrong once:
  #
  #   - `check-crate.sh` passes `--locked` (build.md rule 2), so a fixture with
  #     no `Cargo.lock` fails on the missing lock rather than on the thing it
  #     was written to break. It still goes red, so the suite still reports
  #     green while the case tests nothing.
  #   - `cargo generate-lockfile` needs the package to be valid, and a package
  #     whose `src/lib.rs` does not exist yet is not. Called before this line
  #     it fails, `|| true` swallows it, and no lockfile appears -- which is
  #     the first failure again, wearing a different hat.
  #
  # Both were found by mutant-testing rather than by reading: disabling the fmt
  # check left the unformatted fixture still failing, which is exactly the
  # signal a fixture that has stopped constraining anything gives off.
  echo 'pub fn placeholder() {}' > "$dir/crates/k/src/lib.rs"
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1 || true)
  if [[ ! -f "$dir/Cargo.lock" ]]; then
    printf 'negative.sh: could not create a lockfile in %s\n' "$dir" >&2
    return 1
  fi
  printf '%s\n' "$dir"
}

setup_crate_stale_lock() {
  local dir; dir="$(_crate_scratch crate_stale_lock)"
  echo 'pub fn f() {}' > "$dir/crates/k/src/lib.rs"
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  # A dependency the committed lockfile knows nothing about. `--locked` must
  # refuse to resolve it rather than silently rewriting Cargo.lock -- build.md
  # rule 2's whole point, and the reason the flag is on the hook and not only in
  # CI.
  #
  # ⚠️ **A path dependency inside the fixture, not a crates.io one.** The first
  # version named `serde = "1"` and claimed `--locked` would fail before
  # resolution; it does not. With a network cargo hits the index first, and
  # *without* one the case fails identically with and without `--locked` --
  # which means on any machine that cannot reach crates.io it passed while
  # constraining nothing, the same vacuous-green shape the lockfile bug above
  # already produced once in this file. A path dependency needs no index, so
  # the only thing that can fail is the lock check. Found by review.
  mkdir -p "$dir/crates/dep/src"
  cat > "$dir/crates/dep/Cargo.toml" <<'EOF'
[package]
name = "dep"
version = "0.0.0"
edition = "2021"
EOF
  echo 'pub fn g() {}' > "$dir/crates/dep/src/lib.rs"
  cat >> "$dir/crates/k/Cargo.toml" <<'EOF'

[dependencies]
dep = { path = "../dep" }
EOF
  (cd "$dir" && git add -A && git commit -q -m "M0.2: dependency added without regenerating the lockfile")
  printf '%s\n' "$dir"
}
invoke_crate_stale_lock() {
  bash "$1/scripts/check-crate.sh"
}

setup_crate_fmt() {
  local dir; dir="$(_crate_scratch crate_fmt)"
  # Deliberately misformatted and otherwise clean: no clippy lint fires on this
  # and it has no tests, so a run that fails here fails *only* on rustfmt.
  printf 'pub fn f( ) ->u8{1}\n' > "$dir/crates/k/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M0.2: unformatted source")
  printf '%s\n' "$dir"
}
invoke_crate_fmt() {
  bash "$1/scripts/check-crate.sh"
}

setup_crate_clippy() {
  local dir; dir="$(_crate_scratch crate_clippy)"
  # `let_and_return` is a default-level clippy lint, so this fires without the
  # fixture having to reproduce the workspace's pedantic/nursery configuration
  # — which it deliberately does not carry.
  cat > "$dir/crates/k/src/lib.rs" <<'EOF'
pub fn f() -> u8 {
    let x = 1;
    x
}
EOF
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  (cd "$dir" && git add -A && git commit -q -m "M0.2: clippy warning")
  printf '%s\n' "$dir"
}
invoke_crate_clippy() {
  bash "$1/scripts/check-crate.sh"
}

setup_crate_test() {
  local dir; dir="$(_crate_scratch crate_test)"
  cat > "$dir/crates/k/src/lib.rs" <<'EOF'
pub fn f() -> u8 {
    1
}

#[test]
fn f_is_two() {
    assert_eq!(f(), 2);
}
EOF
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  (cd "$dir" && git add -A && git commit -q -m "M0.2: failing test")
  printf '%s\n' "$dir"
}
invoke_crate_test() {
  bash "$1/scripts/check-crate.sh"
}

setup_coverage_below_floor() {
  local dir; dir="$(_crate_scratch coverage_floor)"
  # ⚠️ A crate with executable lines and a test that exercises almost none of
  # them. `f` is covered; the other three arms are not, which puts the crate
  # well under any sane floor while still leaving the suite green -- so a run
  # that fails here fails *only* on coverage.
  cat > "$dir/crates/k/src/lib.rs" <<'EOF'
pub fn f(n: u8) -> u8 {
    if n == 0 {
        return 1;
    }
    if n == 1 {
        return 2;
    }
    if n == 2 {
        return 3;
    }
    if n == 3 {
        return 4;
    }
    if n == 4 {
        return 5;
    }
    if n == 5 {
        return 6;
    }
    0
}

#[test]
fn only_the_first_arm() {
    assert_eq!(f(0), 1);
}
EOF
  # ⚠️ The gate itself, unmodified. The fixture's crate is named `k`, which
  # appears in neither `UNTIL_FILLED` nor `ALWAYS`, so it is checked rather than
  # excluded and the case fails on coverage. ⚠️ An earlier version rewrote an
  # `EXCLUDED=(...)` array to be safe -- that array had already been split in
  # two, so the rewrite matched nothing and silently did nothing. A no-op
  # safeguard reads exactly like a working one, which is why the assertion
  # below names what it depends on instead.
  copy_gate "$dir" check-coverage.sh
  # ⚠️ `[[:space:]]`, not `\s` -- the latter is a GNU extension and this repo
  # runs on macOS too. A pattern that silently never matches would make this
  # guard useless in exactly the case it exists for.
  if grep -qE '^[[:space:]]*\[k\]=' "$dir/scripts/check-coverage.sh"; then
    printf 'FIXTURE BROKEN: crate `k` is in an exclusion list; this case would pass vacuously\n' >&2
    exit 1
  fi
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  (cd "$dir" && git add -A && git commit -q -m "M0.15: a crate below the coverage floor")
  printf '%s\n' "$dir"
}
invoke_coverage_below_floor() {
  bash "$1/scripts/check-coverage.sh"
}
# --- m0-complete.sh: an M0 gate whose negative case did not run -------------
#
# `M0.26`. ⚠️ **A case that did not run is not a case that passed.** The suite
# exits 0 with a case skipped for a missing tool — correct for the suite, and
# wrong for "every gate fails on a broken artifact". The fixture plants a
# stand-in suite that exits 0 and names one of the four M0 gates as skipped,
# which is exactly what the real suite prints on a machine without
# `cargo-mutants`.
setup_m0_complete_skipped_case() {
  local dir; dir="$(new_scratch m0-complete-skipped)"
  _m0_scaffold "$dir" m0-complete-skipped
  mkdir -p "$dir/scripts/gates" "$dir/tests/gates"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  cat > "$dir/tests/gates/negative.sh" <<'EOF'
#!/usr/bin/env bash
# A suite that passes while one M0 gate went unproven -- the shape M0.26 is for.
echo "  ok  everything that ran, ran"
echo "SKIPPED_CASES check-mutants.sh"
exit 0
EOF
  chmod +x "$dir/tests/gates/negative.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/Cargo.toml" <<'EOF'
[package]
name = "oqueue-core"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-core/src/lib.rs"
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.26: a gate declared complete unwatched")
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: a stated hook count the config contradicts -------------
#
# `M0.25`. ⚠️ NFR-56 is "2.27 s across N hooks", so N is part of the claim, and
# three documents held three different values. Alone among the ten stale claims
# that row corrected, this one is a number a script can count — which is why it
# is the one that got a gate rather than a repair. ⚠️ **And the gate caught its
# author on the first run**, who wrote 17 against a config of 18.
setup_m0_complete_hook_count() {
  local dir; dir="$(new_scratch m0-complete-hooks)"
  _m0_scaffold "$dir" m0-complete-hooks
  mkdir -p "$dir/scripts/gates" "$dir/docs/internal/product"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/Cargo.toml" <<'EOF'
[package]
name = "oqueue-core"
version = "0.1.0"
edition = "2021"
EOF
  echo 'pub fn f() {}' > "$dir/crates/oqueue-core/src/lib.rs"
  cat > "$dir/.pre-commit-config.yaml" <<'EOF'
repos:
  - repo: local
    hooks:
      - id: one
      - id: two
EOF
  cat > "$dir/docs/internal/product/requirements.md" <<'EOF'
| NFR-56 | Pre-commit suite completes within **10 s**. | measured across 9 hooks — and **7 today** |
EOF
  cat > "$dir/docs/internal/product/roadmap.md" <<'EOF'
NFR-56 was measured across 9 hooks — 10 once the budget gate itself joined
them, and **2 today**.
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.25: a hook count no config supports")
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: a threshold check-drift.sh cannot see ------------------
#
# `M0.23`. ⚠️ **The assertion exists because the convention failed twice.**
# Non-negotiable 2 is enforced by a name matcher, so a constant nobody named
# conventionally is unenforced while every gate reports green — `M0.15` found
# it with `MIN_CRATE_COVERAGE`, and `M0.16` wrote two more the matcher could
# not see on the very next commit. The fixture is a workspace that compiles and
# a seam with its fake, so the two assertions above this one pass and it is
# this one that fires.
setup_m0_complete_invisible_threshold() {
  local dir; dir="$(new_scratch m0-complete-invisible)"
  _m0_scaffold "$dir" m0-complete-invisible
  mkdir -p "$dir/scripts/gates"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/Cargo.toml" <<'EOF'
[package]
name = "oqueue-core"
version = "0.1.0"
edition = "2021"
EOF
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub trait Clock: Send + Sync {
    fn now(&self) -> u64;
}
pub struct FakeClock;
impl Clock for FakeClock {
    fn now(&self) -> u64 {
        0
    }
}
EOF
  # A matcher that sees none of the constants the gate asserts.
  cat > "$dir/scripts/check-drift.sh" <<'EOF'
#!/usr/bin/env bash
THRESHOLD_RE='nothing_matches_this'
EOF
  cat > "$dir/scripts/check-coverage.sh" <<'EOF'
#!/usr/bin/env bash
COVERAGE_FLOOR=85
EOF
  cat > "$dir/scripts/check-budget.sh" <<'EOF'
#!/usr/bin/env bash
BUDGET_MS=10000
COMPILING_GATE_MS=5000
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.23: a threshold non-negotiable 2 cannot see")
  printf '%s\n' "$dir"
}

# ⚠️ **Only runnable where `cargo-llvm-cov` is installed.** Without it the gate
# *skips* and exits 0, which this suite would report as "reported ok on a broken
# artifact" -- asserting a regression that does not exist, and turning
# `m-1-complete.sh` red on any clone that has not installed the tool. Nothing in
# this repository asks anyone to. So the case is registered conditionally and
# says so, rather than failing for a missing tool: `lib.sh`'s own rule is that a
# missing tool is a skip with a named remedy, never a failure.
# _m0_scaffold <dir> <name>: the two files `_crate_scratch` writes above and
# for the identical reasons, which `M0.18`'s fixtures needed and did not have.
#
# ⚠️ **The pinned toolchain is what stops these cases going vacuously green.**
# `m0-complete.sh` checks two targets, and `SCRATCH_ROOT` is under `mktemp -d`
# where this repository's `rust-toolchain.toml` does not reach — so the aarch64
# leg failed with `can't find crate for std` no matter what the fixture
# contained. Measured by review: with the planted type error *replaced by
# working code*, the case still reported green. That is the same vacuity the
# comment in `_crate_scratch` calls "the third distinct way this one fixture
# found to be vacuously green", found a fourth time, in a fixture written to
# test the gate that exists to catch it.
_m0_scaffold() {
  cp "$REPO_ROOT/rust-toolchain.toml" "$1/rust-toolchain.toml"
  mkdir -p "$1/.cargo"
  cat > "$1/.cargo/config.toml" <<EOF
[build]
target-dir = "$REPO_ROOT/target/tmp/negative-$2"
EOF
}

# --- m0-complete.sh: a workspace that does not compile ---------------------
#
# `M0.18`. ⚠️ **The assertion no other gate makes.** Every other gate here reads
# files; `m0-complete.sh` is the only one that compiles the workspace, and
# NFR-40 is a claim about two targets that nothing else checks. `M-1.46`'s
# standard says the case is written when the gate is, not thirty commits later.
#
# ⚠️ The scratch repo has no `scripts/` beyond the gate and `lib.sh`, so the
# later assertions fail too — hence the `expect` substring, which is what pins
# this case to the defect it plants rather than to the scaffolding it lacks.
setup_m0_complete_broken_workspace() {
  local dir; dir="$(new_scratch m0-complete-broken)"
  _m0_scaffold "$dir" m0-complete-broken
  mkdir -p "$dir/scripts/gates"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/Cargo.toml" <<'EOF'
[package]
name = "oqueue-core"
version = "0.1.0"
edition = "2021"
EOF
  # Does not compile: a type error rustc reports on both targets.
  echo 'pub fn f() -> u8 { "not a u8" }' > "$dir/crates/oqueue-core/src/lib.rs"
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.18: a workspace that does not compile")
  printf '%s\n' "$dir"
}
invoke_m0_complete() {
  bash "$1/scripts/gates/m0-complete.sh"
}

# --- m0-complete.sh: a pub trait in oqueue-core with no fake beside it ------
#
# `M0.18`. `contracts.md` rules 9 and 11 put every fake in the crate that owns
# the trait. ⚠️ `milestones/M0.md`'s completion condition says `oqueue-testkit`
# instead, contradicting both that standard and its own Goal section — so this
# case is what makes the gate's reading of the two the enforced one.
#
# ⚠️ The workspace here **does** compile, so the trait assertion is what fires
# rather than the cargo one above it.
setup_m0_complete_trait_without_fake() {
  local dir; dir="$(new_scratch m0-complete-no-fake)"
  _m0_scaffold "$dir" m0-complete-no-fake
  mkdir -p "$dir/scripts/gates"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/oqueue-core"]
resolver = "2"
EOF
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/Cargo.toml" <<'EOF'
[package]
name = "oqueue-core"
version = "0.1.0"
edition = "2021"
EOF
  # ⚠️ **Indented, inside a `pub mod`**, which is not decoration: the scanner
  # was anchored to column 0 and a trait one level in was invisible to it. A
  # fixture declaring its trait at column 0 leaves that anchor unconstrained,
  # and reverting the fix keeps the whole suite green.
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub mod seam {
    pub trait Clock: Send + Sync {
        fn now(&self) -> u64;
    }
}
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.18: a seam with no fake beside it")
  printf '%s\n' "$dir"
}

_have_llvm_cov() { cargo llvm-cov --version >/dev/null 2>&1; }

run_case "check-commit-msg.sh"          setup_commit_msg          invoke_commit_msg
run_case "check-commit-msg.sh (unstaged backlog row)" setup_commit_msg_unstaged_row invoke_commit_msg_unstaged_row
run_case "check-tests-kept.sh"          setup_tests_kept          invoke_tests_kept
run_case "check-drift.sh"               setup_drift               invoke_drift
run_case "check-layering.sh"            setup_layering            invoke_layering \
  "depends on oqueue-codec, not oqueue-core"
run_case "check-layering.sh (non-UTF-8 crash)" setup_layering_non_utf8 invoke_layering_non_utf8 \
  "the scanner raised UnicodeDecodeError"
run_case "check-layering.sh (no [lints] workspace)" setup_layering_no_lints invoke_layering \
  "no [lints] workspace = true"
run_case "check-layering.sh ([profile] in a member)" setup_layering_member_profile invoke_layering \
  "has a [profile] section"
run_case "check-layering.sh (no overflow-checks)" setup_layering_no_overflow_checks invoke_layering \
  "does not set overflow-checks = true"
run_case "check-sans-io.sh"             setup_sans_io             invoke_sans_io
run_case "check-core-contract.sh"       setup_core_contract       invoke_core_contract
run_case "check-core-contract.sh (non-UTF-8 crash)" setup_core_contract_non_utf8 invoke_core_contract_non_utf8
run_case "check-unsafe.sh"              setup_unsafe              invoke_unsafe
run_case "check-unsafe.sh (non-UTF-8 file doesn't suppress a real violation)" setup_unsafe_non_utf8 invoke_unsafe_non_utf8
run_case "check-reviewed.sh"            setup_reviewed            invoke_reviewed
run_case "check-reviewed.sh (regex task_id)" setup_reviewed_regex_task_id invoke_reviewed_regex_task_id
run_case "check-milestone-review.sh"    setup_milestone_review    invoke_milestone_review
run_case "build-index.sh --check"       setup_build_index         invoke_build_index
run_case "check-requirements-trace.sh"  setup_requirements_trace  invoke_requirements_trace
run_case "check-file-size.sh"           setup_file_size           invoke_file_size
run_case "check-readmes.sh"             setup_readmes             invoke_readmes
run_case "check-readmes.sh (bin/oqueue)" setup_readmes_bin          invoke_readmes_bin \
  "mimalloc"
run_case "check-hot-path-bench.sh"      setup_hot_path_bench      invoke_hot_path_bench
run_case "check-hot-path-bench.sh (required row)" setup_hot_path_bench_required invoke_hot_path_bench_required
run_case "check-hot-path-bench.sh (leftover entry)" setup_hot_path_bench_leftover invoke_hot_path_bench_leftover
# ⚠️ Both portability cases carry an `expect` since `M0.24` gave the gate a
# fourth property: their fixtures name no script, so they trip check 3's
# inspected-nothing guard as a *second* problem, and deleting check 1 outright
# left the suite green. Same regression `M0.21` found for `check-layering.sh`,
# and the header's own remedy for it.
run_case "check-portability.sh"         setup_portability         invoke_portability \
  "vendor syntax (Claude Code's @import)"
run_case "check-portability.sh (index claims a present script is missing)" setup_portability_stale_claim invoke_portability_stale_claim \
  "says a script is missing, and all 1 it refers to exist"
run_case "check-portability.sh (unterminated fence)" setup_portability_unterminated_fence invoke_portability_unterminated_fence \
  "fence"
# ⚠️ The `expect` is `"fails for"`, not the full command line. The gate narrows
# to `--workspace --exclude oqueue` for a non-host target with no cross `cc`, so
# the exact wording depends on the host triple — on macOS, which
# `portability.md` rule 2 makes first-class, *both* legs narrow and a substring
# naming the wide form matches nothing. That would report "failed, but not for
# the reason the fixture plants" for a defect that does not exist. Found by
# review, on a platform this machine is not.
run_case "m0-complete.sh (workspace does not compile)" setup_m0_complete_broken_workspace invoke_m0_complete \
  "mismatched types"
run_case "m0-complete.sh (pub trait with no fake beside it)" setup_m0_complete_trait_without_fake invoke_m0_complete \
  "pub trait Clock has no FakeClock in oqueue-core"
run_case "m0-complete.sh (threshold invisible to check-drift.sh)" setup_m0_complete_invisible_threshold invoke_m0_complete \
  "is invisible to check-drift.sh"
run_case "m0-complete.sh (hook count disagrees with the config)" setup_m0_complete_hook_count invoke_m0_complete \
  "hooks, .pre-commit-config.yaml has"
run_case "m0-complete.sh (a negative case that never ran)" setup_m0_complete_skipped_case invoke_m0_complete \
  "never watched to fail on this machine"
run_case "m-1-complete.sh (missing Non-negotiables section)" setup_m1_complete_missing_section invoke_m1_complete_missing_section
run_case "m-1-complete.sh (non-UTF-8 AGENTS.md, crash path)" setup_m1_complete_non_utf8 invoke_m1_complete_non_utf8
run_case "check-crate.sh (unformatted)"  setup_crate_fmt          invoke_crate_fmt
run_case "check-crate.sh (stale lockfile)" setup_crate_stale_lock invoke_crate_stale_lock
run_case "check-crate.sh (clippy warning)" setup_crate_clippy     invoke_crate_clippy
run_case "check-crate.sh (failing test)" setup_crate_test         invoke_crate_test
setup_budget_over() {
  local dir; dir="$(new_scratch budget_over)"
  copy_gate "$dir" check-budget.sh
  # ⚠️ A timings artifact whose entries belong to *this* process group and
  # exceed the budget. Written directly rather than by running a slow suite:
  # the thing under test is the comparison, and a fixture that took 10 real
  # seconds to prove a 10-second budget would be its own budget problem.
  mkdir -p "$dir/target/timings"
  local pg; pg="$(ps -o pgid= -p $$ | tr -d ' ')"
  # ⚠️ **No single entry over `COMPILING_GATE_MS` (5000).** The gate exempts a
  # run where one gate dominates, because that is a build rather than an eroded
  # suite -- so a fixture with a 7000 ms entry tests the *exemption* and reports
  # the gate as broken. This models the case the budget is actually for: many
  # gates each a little slower, summing past the line.
  {
    printf '%s\tcheck-slow-one.sh\t4200\t%s\n' "$pg" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\tcheck-slow-two.sh\t4100\t%s\n' "$pg" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf '%s\tcheck-slow-three.sh\t3900\t%s\n' "$pg" "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  } > "$dir/target/timings/gates.tsv"
  printf '%s\n' "$dir"
}
invoke_budget_over() {
  # ⚠️ Run in the *same* process group as the setup that wrote the artifact,
  # which is what the gate keys on.
  bash "$1/scripts/check-budget.sh"
}

if _have_llvm_cov; then
  run_case "check-coverage.sh (crate below the floor)" setup_coverage_below_floor invoke_coverage_below_floor
else
  skip_case check-coverage.sh "cargo-llvm-cov not installed" \
    "install: cargo install cargo-llvm-cov"
fi
setup_mutants_weakened() {
  local dir; dir="$(_crate_scratch mutants_weakened)"
  copy_gate "$dir" mutants.sh
  copy_gate "$dir" check-mutants.sh
  mkdir -p "$dir/baselines"
  printf '# fixture: nothing argued\n' > "$dir/baselines/mutants.txt"
  # ⚠️ A function with a real branch, and a test that **executes it without
  # constraining it** — the characteristic failure `testing.md` rule 15 names.
  # Coverage is 100%; `cargo mutants` replaces the body and the test still
  # passes. That is exactly the case this gate exists for.
  cat > "$dir/crates/k/src/lib.rs" <<'EOF'
pub fn classify(n: i32) -> &'static str {
    if n < 0 { "negative" } else { "non-negative" }
}

#[test]
fn it_returns_something() {
    // Executes the line, constrains nothing about it.
    let _ = classify(-1);
    let _ = classify(1);
}
EOF
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  (cd "$dir" && git add -A && git commit -q -m "M0.17: a test that constrains nothing")
  printf '%s\n' "$dir"
}
invoke_mutants_weakened() {
  bash "$1/scripts/check-mutants.sh" --full
}

setup_mutants_narrowed() {
  local dir; dir="$(setup_mutants_weakened)"
  # ⚠️ The **narrowed** mode, which is the one pre-commit runs and the one both
  # vacuous-pass paths were found in. `--in-diff` needs a staged *modification*,
  # not merely a staged file: re-adding an unchanged file leaves
  # `git diff --cached` empty and the gate correctly skips — which is what this
  # fixture did on its first attempt, and a case passing on that skip would
  # prove nothing.
  cat >> "$dir/crates/k/src/lib.rs" <<'EOF'

pub fn also_unconstrained(n: i32) -> &'static str {
    if n > 100 { "big" } else { "small" }
}

#[test]
fn it_runs_the_other_one_too() {
    let _ = also_unconstrained(1);
    let _ = also_unconstrained(1000);
}
EOF
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  (cd "$dir" && git add crates/k/src/lib.rs)
  printf '%s\n' "$dir"
}
invoke_mutants_narrowed() {
  bash "$1/scripts/check-mutants.sh"
}

run_case "check-budget.sh (suite over budget)" setup_budget_over invoke_budget_over
_have_mutants() { cargo mutants --version >/dev/null 2>&1; }
if _have_mutants; then
  run_case "check-mutants.sh (test constrains nothing)" setup_mutants_weakened invoke_mutants_weakened
  run_case "check-mutants.sh (narrowed, test constrains nothing)" setup_mutants_narrowed invoke_mutants_narrowed
else
  skip_case check-mutants.sh "cargo-mutants not installed" \
    "install: cargo install cargo-mutants"
fi

note "$TOTAL gate(s) exercised, $FAILED_CASES failed to fail as expected"

# ⚠️ **Machine-readable, on its own line, and printed even when none skipped.**
# A caller that greps for this must be able to tell "nothing skipped" from
# "this suite is too old to say", and an absent line cannot carry that
# difference. `m0-complete.sh` reads it. `M0.26`.
printf 'SKIPPED_CASES %s\n' "${SKIPPED_GATES[*]:-}"

finish
