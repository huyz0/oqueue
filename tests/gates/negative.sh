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

# _next_scratch <prefix>: a scratch repo whose name no earlier call has used.
#
# ⚠️ **Counted from the filesystem, not from a shell variable**, and review
# measured why: every caller is `dir="$(_fixture ..)"`, so a `seq=$((seq + 1))`
# inside runs in a subshell and never reaches the parent. Three helpers here
# carried that counter and all three always returned 1; nothing had been
# corrupted yet only because each fixture rewrites its own files with `>`,
# which is luck rather than the guarantee those comments claimed.
_next_scratch() {
  local prefix="$1" n
  n="$(find "$SCRATCH_ROOT" -maxdepth 1 -name "$prefix-*" 2>/dev/null | wc -l)"
  new_scratch "$prefix-$((n + 1))"
}

# copy_gate <dir> <script-name>: brings one real gate script into a scratch
# repo, unmodified, so this suite tests the actual file rather than a copy
# of its logic.
copy_gate() {
  cp "$REPO_ROOT/scripts/$2" "$1/scripts/$2"
  chmod +x "$1/scripts/$2"
  # ⚠️ And whatever the gate imports. `M1.32` moved the Cargo.toml parser to
  # `scripts/lib/manifest.py`; a gate copied without it fails on the import,
  # which `run_case` reports as "failed, but not for the reason the fixture
  # plants" — six cases at once, measured.
  mkdir -p "$1/scripts/lib"
  cp "$REPO_ROOT"/scripts/lib/*.py "$1/scripts/lib/" 2>/dev/null || true
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
# ⚠️ Cases, not gates — one `skip_case` can stand for several `run_case` lines.
SKIPPED_CASE_COUNT=0

# ⚠️ Every gate this suite may name as skipped. A `skip_case` argument that is
# not one of these is a **typo, not a report**: `m0-complete.sh` matches the
# names it is given against its own `M0_GATES`, so a descriptive label — the
# convention every `run_case` line uses — or a rename on one side only would
# leave a gate unproven while the completion gate said otherwise. That is the
# shape `M0.26` exists to remove, one argument over. `M0.28`.
SKIPPABLE_GATES="check-crate.sh check-coverage.sh check-budget.sh check-mutants.sh m0-complete.sh"

# skip_case <gate.sh> <reason> [remedy] [case-count]: report that one or more
# cases could not run, and record which gate is thereby unproven.
#
# ⚠️ The **gate name** is the machine-readable part — a caller asking "was
# `check-mutants.sh` watched to fail?" needs a name, not prose — and
# `case-count` (default 1) is how many `run_case` lines this one call stands
# for, because a single conditional block can guard several. Both are reported
# separately at the end, and they differ: without `cargo-mutants` two cases do
# not run and one call fires.
# ⚠️ **Every exit from `skip_case` records the gate**, all four of them: the
# ordinary skip, and three rejection paths — one of which this same commit
# adds. `M1.25`: the two pre-existing ones used to `return` without touching
# `SKIPPED_GATES`, so a call the suite refused left the gate absent from
# `SKIPPED_CASES` — and `m0-complete.sh` reads that line by *matching* names,
# so an absent one reads exactly like a gate whose case ran. It then printed
# "every M0 gate's negative case actually ran" for a gate nothing watched.
# The suite does exit non-zero on those paths (`fail` increments `lib.sh`'s
# `_FAILURES`), so this is belt to that braces — but the claim on the summary
# line should be true on its own, not only in combination with an exit code
# somebody else has to check.
_skip_case_record() {
  SKIPPED_GATES+=("$1")
  # ⚠️ `10#` — a caller writing `08` or `09` for a count otherwise reaches
  # `$(( ))` as an invalid octal literal, which dies under `set -e` *before*
  # `SKIPPED_COUNT` prints, and `m0-complete.sh` then reports "no
  # SKIPPED_CASES line": the exact misdiagnosis the `return 0, deliberately`
  # note below exists to prevent. Measured: `n=08; echo $(( 0 + n ))` is
  # "value too great for base".
  SKIPPED_CASE_COUNT=$((SKIPPED_CASE_COUNT + 10#$2))
}

skip_case() {
  local gate="$1" reason="$2" remedy="${3:-}" cases="${4:-1}"
  # ⚠️ **The gate name is checked first**, before the argument-shape guards
  # below. A call that is wrong in two ways — `skip_case mutants "..." 3` — is
  # then diagnosed as the typo it is, in one round, rather than reporting the
  # misplaced count, being fixed, and failing again on the name.
  case " $SKIPPABLE_GATES " in
    *" $gate "*) ;;
    *)
      fail "skip_case called with '$gate', which is not a script name this suite knows"
      note "it must be one of: $SKIPPABLE_GATES"
      note "⚠️ m0-complete.sh matches these against M0_GATES; a label here is a typo"
      FAILED_CASES=$((FAILED_CASES + 1))
      # Recorded under the name as given. It matches no `M0_GATES` entry, so
      # `m0-complete.sh` ignores it — but it appears on `SKIPPED_CASES`, where
      # a reader can see that something was refused rather than nothing.
      _skip_case_record "$gate" "$([[ "$cases" =~ ^[0-9]+$ ]] && printf '%s' "$cases" || printf 1)"
      # ⚠️ **`return 0`, deliberately.** `lib.sh` sets `-e`, and this branch is
      # the last command of its `case` arm — a non-zero return aborted the
      # whole suite here, so neither `SKIPPED_COUNT` nor `SKIPPED_CASES` was
      # printed and `m0-complete.sh` then reported "no SKIPPED_CASES line", a
      # stale suite, when the cause was a typo'd argument. Misdiagnosing
      # remedy, in the function added to stop one. ⚠️ The non-zero exit comes
      # from `fail` above — `lib.sh` increments `_FAILURES` and `finish` exits
      # on it; `FAILED_CASES` only feeds the summary line. Swapping that `fail`
      # for a `warn` would make a typo'd name exit 0 with the gate missing from
      # `SKIPPED_CASES`, and `m0-complete.sh` would report every case as having
      # run. Do not.
      return 0
      ;;
  esac
  # ⚠️ **A bare number in the remedy slot is a count somebody put one argument
  # early**, and it is the silent half of this pair: the count then defaults to
  # 1, the number is printed to the reader as though it were advice, and the
  # suite undercounts with nothing failing. `M1.23`'s sibling defect — the
  # fourth argument was validated and the third was not. A remedy is prose; no
  # real one is digits alone.
  if [[ -n "$remedy" && "$remedy" =~ ^[0-9]+$ ]]; then
    fail "skip_case's third argument is a remedy, got the bare number '$remedy'"
    note "usage: skip_case <gate.sh> <reason> [remedy] [case-count]"
    note "to give a count and no remedy, pass an empty one: skip_case $gate \"...\" '' $remedy"
    FAILED_CASES=$((FAILED_CASES + 1))
    # Recorded with the count the caller plainly meant, so the summary is
    # right even though the call is wrong; the `fail` above is what gets the
    # call fixed. ⚠️ One case reads otherwise and is deliberately not special
    # -cased: `skip_case gate "reason" 3 2` — both slots filled — records 3
    # rather than the explicit 2. The call is refused and the suite is red
    # either way, and guessing between two numbers a confused caller supplied
    # is worse than taking the one in the slot being complained about.
    _skip_case_record "$gate" "$remedy"
    return 0
  fi
  # ⚠️ A non-numeric count is a swapped argument, not a count. Unchecked, the
  # arithmetic below dies under `set -u` before either summary line prints and
  # `m0-complete.sh` reports a stale suite — the same misdiagnosis the branch
  # above was written to stop, one argument over. Found by review.
  if [[ ! "$cases" =~ ^[0-9]+$ ]]; then
    fail "skip_case's fourth argument is a case count, got '$cases'"
    note "usage: skip_case <gate.sh> <reason> [remedy] [case-count]"
    FAILED_CASES=$((FAILED_CASES + 1))
    _skip_case_record "$gate" 1
    return 0
  fi
  _skip_case_record "$gate" "$cases"
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
# --- check-drift.sh: a threshold whose unit is spelled `msec` ---------------
#
# `M0.27`. ⚠️ **The narrowing that fixed `_msg` dropped `_msec`**, which is a
# unit spelling too, so a threshold an environment moves passed non-negotiable
# 2. `m0-complete.sh` section 5 does not backstop it — `NFR_CONSTANTS` names
# no `*_MSEC` (⚠️ seven entries since `M1.35`; the load-bearing part is only
# that no `*_MSEC` is among them). The row first argued this
# half could not have a case, on the grounds that removing a false positive is
# inexpressible in an inverted suite; **re-widening is the other half of the
# same fix and makes the gate start catching something**, which is exactly what
# this suite tests. Found by review, twice, which is why the rule above now
# says an untestable claim is worth one attempt at disproof.
setup_drift_msec() {
  local dir; dir="$(new_scratch drift-msec)"
  copy_gate "$dir" check-drift.sh
  mkdir -p "$dir/scripts"
  # Split for the reason `setup_drift` records at length: a literal here would
  # trip the real gate on this very file.
  local dollar='$'
  local poll_expr="${dollar}{OQUEUE_POLL_MSEC:-500}"
  {
    echo '#!/usr/bin/env bash'
    printf 'POLL_MSEC="%s"\n' "$poll_expr"
  } > "$dir/scripts/poll.sh"
  (cd "$dir" && git add -A && git commit -q -m "M0.27: a settable poll interval in msec")
  printf '%s\n' "$dir"
}
invoke_drift_msec() {
  bash "$1/scripts/check-drift.sh"
}

# --- check-drift.sh: a raised clippy threshold ------------------------------
#
# `M2.9` (closing `M1.50`): five clippy.toml thresholds sat on the commit path
# pinned by nothing — NFR_CONSTANTS greps `NAME=` and cannot read TOML. This
# plants the exact move rust-style.md rule 7 names — raising
# too-many-lines-threshold back toward clippy's default — in the spelling
# review measured slipping a parse-then-compare draft: `+100`, which TOML
# reads as 100. The other four keys are planted correct, so the case fails
# for the raised value alone.
setup_clippy_pin() {
  local dir; dir="$(new_scratch clippy-pin)"
  copy_gate "$dir" check-drift.sh
  {
    echo 'too-many-lines-threshold = +100'
    echo 'cognitive-complexity-threshold = 20'
    echo 'too-many-arguments-threshold = 5'
    echo 'type-complexity-threshold = 250'
    echo 'enum-variant-size-threshold = 200'
  } > "$dir/clippy.toml"
  (cd "$dir" && git add -A && git commit -q -m "M1.50: a raised clippy threshold")
  printf '%s\n' "$dir"
}
invoke_clippy_pin() {
  bash "$1/scripts/check-drift.sh"
}

# --- check-drift.sh: a Rust bound moved away from its pinned value --------
#
# ⚠️ **`testing.md` rule 20a, on the day the gate grew a property** (`M3.36`).
# `RUST_BOUNDS` pins eleven `pub const` bounds by value on the commit path,
# because `m0-complete.sh`'s `NFR_CONSTANTS` resolves a shell `NAME=` and
# cannot see a Rust one — so eight bounds deciding how much object storage one
# request may cost were pinned by nothing. A gate nobody has watched fail is a
# gate nobody has tested, and `M2.9` set this precedent in this same script
# when it added `CLIPPY_THRESHOLDS`.
setup_rust_bound() {
  local dir; dir="$(new_scratch rust-bound)"
  copy_gate "$dir" check-drift.sh
  mkdir -p "$dir/crates/oqueue-broker/src/fetch"
  # ⚠️ The real map's own file and name, with the value raised. Raising is the
  # weakening direction for this one: it lets a client's read volume be set by
  # somebody else's write rate.
  cat > "$dir/crates/oqueue-broker/src/fetch/target.rs" <<'RS'
pub(crate) const MAX_READS_PER_REQUEST: u32 = 64;
RS
  (cd "$dir" && git add -A && git commit -q -m "M3.36: a bound raised away from its pinned value")
  printf '%s\n' "$dir"
}
invoke_rust_bound() {
  bash "$1/scripts/check-drift.sh"
}

# --- check-drift.sh: a _days threshold ------------------------------------
#
# ⚠️ `M1.35` widened THRESHOLD_RE with `_days?` and left it unpinned; review
# measured that removing the alternative again passes the whole commit path and
# CI green, because only `m0-complete.sh` notices and nothing invokes that on a
# push. Exactly the regression `M0.27` hit when a narrowing edit dropped
# `_msec`, which is why the case above exists.
setup_drift_days() {
  local dir; dir="$(new_scratch drift-days)"
  copy_gate "$dir" check-drift.sh
  mkdir -p "$dir/scripts"
  # Split for the reason `setup_drift` records: a literal here would trip the
  # real gate on this very file.
  local dollar='$'
  local keep_expr="${dollar}{OQUEUE_KEEP_DAYS:-30}"
  {
    echo '#!/usr/bin/env bash'
    printf 'TIMINGS_KEEP_DAYS="%s"\n' "$keep_expr"
  } > "$dir/scripts/retain.sh"
  (cd "$dir" && git add -A && git commit -q -m "M1.35: a settable retention window in days")
  printf '%s\n' "$dir"
}
invoke_drift_days() {
  bash "$1/scripts/check-drift.sh"
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

# ⚠️ **`scan_clock` in both directions** (`M10.6`). It deletes the
# `tokio::time::Instant::now()` token and matches what is left, so two things
# need watching: a genuine wall-clock read in a library crate still fails, and
# a line carrying *both* a virtual read and a real one still fails — a first
# version filtered the whole line and let the second walk through.
#
# ⚠️ **`crates/oqueue-core/tests/mixed.rs` only** (`M10.27`). A first version
# also planted `src/lib.rs` with its own, unrelated `SystemTime::now()` —
# which fails the gate on its own merit regardless of whether `mixed.rs`'s
# line survives correctly, so a `scan_clock` reverted to the whole-line
# filter this case exists to catch still exited non-zero and the case stayed
# green. Verified directly: with only `src/lib.rs`'s reads removed, a
# whole-line-filter `scan_clock` reports `ok` against this fixture; the
# planted `src/lib.rs` was what had been masking it.
setup_sans_io_wall_clock() {
  local dir; dir="$(new_scratch sans-io-wall-clock)"
  copy_gate "$dir" check-sans-io.sh
  # ⚠️ **One line holding both**, which is what distinguishes deleting the
  # virtual *token* from filtering the whole line: a line filter drops this
  # line and the wall-clock read with it.
  mkdir -p "$dir/crates/oqueue-core/tests"
  cat > "$dir/crates/oqueue-core/tests/mixed.rs" <<'EOF'
pub fn both() -> (tokio::time::Instant, std::time::SystemTime) {
    (tokio::time::Instant::now(), std::time::SystemTime::now())
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.6: a wall clock beside a virtual one")
  printf '%s\n' "$dir"
}
invoke_sans_io_wall_clock() {
  bash "$1/scripts/check-sans-io.sh"
}

# The other direction: a pure `tokio::time` read is virtual under a paused
# runtime and must NOT be flagged, or the gate flags determinism.
setup_sans_io_virtual_clock_in_src() {
  local dir; dir="$(new_scratch sans-io-virtual-in-src)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-core/src"
  cat > "$dir/crates/oqueue-core/src/lib.rs" <<'EOF'
pub fn started() -> tokio::time::Instant {
    tokio::time::Instant::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.6: a runtime clock in shipped code")
  printf '%s\n' "$dir"
}
invoke_sans_io_virtual_clock_in_src() {
  bash "$1/scripts/check-sans-io.sh"
}

# ⚠️ **`*/tests/*` matches `tests` as *any* path component, not just the
# crate-root tree the licence is for** (`M10.27`). This plants the exact shape
# that widened it: a *nested* `src/tests/` directory, which — unless
# `#[cfg(test)]`-gated, which this fixture deliberately is not — compiles into
# the shipped library. Before the anchor, this took the same tests/-tree
# exemption a genuine integration test gets and the virtual-clock deletion hid
# a real one; after it, `crates/[^/]*/tests/*` requires exactly one path
# component between `crates/` and `/tests/`, which `src/tests` is not.
setup_sans_io_nested_tests_dir_in_src() {
  local dir; dir="$(new_scratch sans-io-nested-tests-dir)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-core/src/tests"
  cat > "$dir/crates/oqueue-core/src/tests/mod.rs" <<'EOF'
pub fn started() -> tokio::time::Instant {
    tokio::time::Instant::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.27: a runtime clock in a nested src/tests/ directory")
  printf '%s\n' "$dir"
}
invoke_sans_io_nested_tests_dir_in_src() {
  bash "$1/scripts/check-sans-io.sh"
}

# ⚠️ **The corpus reader, watched to fail** (`testing.md` rule 20a). Both
# fixtures hold only *invalid* rows, so no `cargo test` runs: the point is the
# reader's refusals, and a fixture with a replayable seed would make this case
# cost a build to prove a parse.
# ⚠️ **The sweep's failure path, watched to fail** (`testing.md` rule 20a). Its
# whole deliverable *is* that path — the artifact and the non-zero exit — and
# without a case here, deleting the `printf` that writes the artifact leaves
# every gate in this repository green while a red night files nothing.
#
# ⚠️ **A stub `cargo` on `PATH`**, so the case costs a second rather than a
# build: the sweep's contract is "a run that fails becomes an artifact", and
# what makes the run fail is not the subject.
# ⚠️ **The build-vs-run distinction, watched to fail.** Deleting the sweep's
# `--no-run` guard leaves the artifact case green — measured — because that
# case's stub answers `--no-run` with success and can only exercise the guard's
# passing side. Here the stub fails the build, and a sweep without the guard
# files a random seed as a failing schedule on a tree that does not compile.
setup_seed_sweep_blames_the_build() {
  local dir; dir="$(new_scratch seed-sweep-build)"
  copy_gate "$dir" seed-sweep.sh
  mkdir -p "$dir/stub"
  cat > "$dir/stub/cargo" <<'EOF'
#!/usr/bin/env bash
echo "error: expected one of `;`, found `not`" >&2
exit 101
EOF
  chmod +x "$dir/stub/cargo"
  printf 'name = "tokio"\nversion = "9.9.9"\n' > "$dir/Cargo.lock"
  (cd "$dir" && git add -A && git commit -q -m "M10.11: a tree that does not build")
  printf '%s\n' "$dir"
}
invoke_seed_sweep_blames_the_build() {
  local dir="$1" rc=0
  ( cd "$dir" && PATH="$dir/stub:$PATH" SWEEP_SEEDS=1 bash "$dir/scripts/seed-sweep.sh" ) || rc=$?
  # ⚠️ **No artifact**, which is the guard's whole point: a seed filed here
  # names a schedule that never ran.
  compgen -G "$dir/target/seeds/failing-*.tsv" > /dev/null && return 0
  return "$rc"
}

setup_seed_sweep_files_the_artifact() {
  local dir; dir="$(new_scratch seed-sweep-artifact)"
  copy_gate "$dir" seed-sweep.sh
  mkdir -p "$dir/stub"
  cat > "$dir/stub/cargo" <<'EOF'
#!/usr/bin/env bash
# --no-run builds; anything else is the run this sweep is looking at.
for arg in "$@"; do [[ "$arg" == "--no-run" ]] && exit 0; done
echo "a consumer can see offset 3 with only Some(1) durable" >&2
exit 1
EOF
  chmod +x "$dir/stub/cargo"
  printf 'name = "tokio"\nversion = "9.9.9"\n' > "$dir/Cargo.lock"
  (cd "$dir" && git add -A && git commit -q -m "M10.11: a sweep whose runs fail")
  printf '%s\n' "$dir"
}
invoke_seed_sweep_files_the_artifact() {
  local dir="$1" rc=0
  # ⚠️ One seed, so the case is one stubbed run rather than thirty-two.
  ( cd "$dir" && PATH="$dir/stub:$PATH" SWEEP_SEEDS=1 bash "$dir/scripts/seed-sweep.sh" ) || rc=$?
  # ⚠️ **The artifact is checked, not just the exit code.** Deleting the
  # `printf` that writes it leaves the sweep exiting non-zero with the same
  # message, so a case that looked only at the status would stay green while
  # the deliverable vanished. Returning 0 here is what makes this suite report
  # "failed to fail".
  local artifact
  artifact="$(compgen -G "$dir/target/seeds/failing-*.tsv" || true)"
  [[ -n "$artifact" ]] || return 0
  # ⚠️ **The `found` column too, not just the file.** The extraction patterns
  # are hand-copied from `Violation`'s `Display`, with nothing coupling them —
  # a reworded arm silently returns every sweep to a column that says only
  # "look in the log", which is the transcription this row exists to end.
  grep -q "a consumer can see" "$artifact" || return 0
  return "$rc"
}

setup_seed_corpus_unpinned() {
  local dir; dir="$(new_scratch seed-corpus-unpinned)"
  copy_gate "$dir" seed-corpus.sh
  mkdir -p "$dir/tests/seeds"
  printf '#seed\ttoolchain\ttokio\tfound\tfixed_by\n7\t\t\tno pin\tnobody\n' \
    > "$dir/tests/seeds/corpus.tsv"
  (cd "$dir" && git add -A && git commit -q -m "M10.11: a seed with no versions")
  printf '%s\n' "$dir"
}
invoke_seed_corpus_unpinned() {
  bash "$1/scripts/seed-corpus.sh"
}

setup_seed_corpus_missing() {
  local dir; dir="$(new_scratch seed-corpus-missing)"
  copy_gate "$dir" seed-corpus.sh
  (cd "$dir" && git add -A && git commit -q -m "M10.11: no corpus at all")
  printf '%s\n' "$dir"
}
invoke_seed_corpus_missing() {
  bash "$1/scripts/seed-corpus.sh"
}

check_sans_io_allows_virtual_clock() {
  local dir; dir="$(new_scratch sans-io-virtual-clock)"
  copy_gate "$dir" check-sans-io.sh
  # ⚠️ **In a `tests/` tree, because that is the whole scope of the licence.**
  # A paused runtime is a test construct; the same line in `src/` is a real
  # clock read in shipped code and stays flagged.
  mkdir -p "$dir/crates/oqueue-core/tests"
  cat > "$dir/crates/oqueue-core/tests/paused.rs" <<'EOF'
pub fn started() -> tokio::time::Instant {
    tokio::time::Instant::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.6: a virtual clock read")
  if bash "$dir/scripts/check-sans-io.sh" >/dev/null 2>&1; then
    ok "check-sans-io.sh allows a tokio::time read, which a paused run controls"
  else
    fail "check-sans-io.sh flags tokio::time::Instant::now(), which is virtual"
  fi
}

# ⚠️ **The same licence, spelled the way a real test would** (`M10.27`).
# `use tokio::time::Instant;` plus a bare `Instant::now()` is the idiom, and
# only stripping the fully-qualified form before this row left it flagged
# under a remedy ("inject `Clock`") that does not apply — the same gap
# `REAL_CLOCK_RE`'s own `use std::time::Instant` coverage already closed on
# the real-clock side.
check_sans_io_allows_virtual_clock_via_bare_import() {
  local dir; dir="$(new_scratch sans-io-virtual-clock-bare-import)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-core/tests"
  cat > "$dir/crates/oqueue-core/tests/paused.rs" <<'EOF'
use tokio::time::Instant;

pub fn started() -> Instant {
    Instant::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.27: a virtual clock read via a bare import")
  if bash "$dir/scripts/check-sans-io.sh" >/dev/null 2>&1; then
    ok "check-sans-io.sh allows a bare Instant::now() imported from tokio::time"
  else
    fail "check-sans-io.sh flags a bare tokio::time::Instant::now(), which is virtual"
  fi
}

# ⚠️ **The broker is exempt from `CLOCK_RE` and not from `REAL_CLOCK_RE`**
# (`M10.5`), which is the half a scan of the other crates cannot reach: the
# broker legitimately holds timers, and what it may not hold is a clock a
# seeded run cannot advance. `tokio::time`'s reads are virtual under a paused
# runtime — measured, a 600 s timeout in 1.3 us of wall clock — so this plants
# the form that is not.
setup_sans_io_broker_clock() {
  local dir; dir="$(new_scratch sans-io-broker-clock)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-broker/src"
  # ⚠️ **The realistic spelling**, not the fully-qualified one. The first
  # version of `REAL_CLOCK_RE` matched only `std::time::Instant::now()` — a
  # form nobody writes — and this `use` plus a bare call walked through it.
  cat > "$dir/crates/oqueue-broker/src/session.rs" <<'EOF'
use std::time::Instant;

pub fn started() -> Instant {
    Instant::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.5: a clock no seeded run can control")
  printf '%s\n' "$dir"
}
invoke_sans_io_broker_clock() {
  bash "$1/scripts/check-sans-io.sh"
}

# The one file that may hold it, so the exemption is watched as well as the
# rule — a named exemption nobody checks is a hole rather than a decision.
# ⚠️ **The exemption is watched too**, by a positive check rather than a
# `run_case`: a named exemption nobody exercises is a hole rather than a
# decision, and this suite's shape only proves that broken things fail.
check_sans_io_writer_id_stays_exempt() {
  local dir; dir="$(new_scratch sans-io-writer-id)"
  copy_gate "$dir" check-sans-io.sh
  mkdir -p "$dir/crates/oqueue-broker/src"
  cat > "$dir/crates/oqueue-broker/src/writer_id.rs" <<'EOF'
pub fn stamp() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
EOF
  (cd "$dir" && git add -A && git commit -q -m "M10.5: the exempt file")
  if bash "$dir/scripts/check-sans-io.sh" >/dev/null 2>&1; then
    ok "check-sans-io.sh keeps writer_id.rs exempt from the seeded-clock rule"
  else
    fail "check-sans-io.sh flags writer_id.rs, whose exemption ADR-0028 rests on"
  fi
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

# --- check-milestone-review.sh: a lettered task id commit no review
# artifact covers ------------------------------------------------------------
#
# ⚠️ **Pins the fix M10's closing review found and five separate copies of
# the same bug had already cost three discovery passes** (`M10.33`-`M10.35`):
# `milestone_subject_re`'s `\.[0-9]+[,:]` has no room for a trailing letter,
# so a commit subject like `M-1.1a: ...` matched nothing and
# `milestone_commits` silently excluded it -- not a wrong verdict on the
# commit, an *absent* one, so the gate reported full coverage over a
# milestone whose lettered-id commit it never enumerated at all. Without
# `[a-z]?` this case would report `ok` on a broken artifact, which is
# exactly the failure `negative.sh` exists to catch.
setup_milestone_review_lettered_id() {
  local dir; dir="$(new_scratch milestone-review-lettered-id)"
  copy_gate "$dir" check-milestone-review.sh
  mkdir -p "$dir/docs/internal/product"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1a | a real, lettered task | some criterion | done |
EOF
  (cd "$dir" && git add -A && git commit -q -m "M-1.1a: a lettered subject with no covering review artifact")
  printf '%s\n' "$dir"
}
invoke_milestone_review_lettered_id() {
  bash "$1/scripts/check-milestone-review.sh" --milestone M-1
}

# --- check-milestone-review.sh: a milestone whose every commit is review
# bookkeeping ---------------------------------------------------------------
#
# ⚠️ Pins the message. `M1.46` made this branch and the pre-existing "has no
# commits yet" skip two different outcomes from the same empty enumeration, so
# this fixture's output can carry more than one kind of result and
# `testing.md` rule 20a asks for the pin: without it the case passes on a gate
# that fails for the ordinary uncovered-commit reason instead.
# ⚠️ Takes the scratch name, because `new_scratch` keys on it and two cases
# share this fixture. Re-entering one repo makes the second `git commit` find
# nothing staged, exit 1, and kill the whole suite under `set -e` -- measured:
# the run ended silently after the previous case with no summary line.
_setup_milestone_review_bookkeeping() {
  local dir; dir="$(new_scratch "$1")"
  # Both, because this fixture feeds two cases: the gate and the driver, which
  # carry the same branch.
  copy_gate "$dir" check-milestone-review.sh
  copy_gate "$dir" milestone-review.sh
  mkdir -p "$dir/docs/internal/product" "$dir/reviews"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | done |
EOF
  (cd "$dir" && git add -A && git commit -q -m "bootstrap: not a task subject")
  # The only commit naming the milestone changes nothing but a verdict file,
  # so milestone_commits excludes it and the enumeration comes back empty.
  printf '{"milestone":"M-1","commits":[],"verdict":"pass","findings":[]}\n' \
    > "$dir/reviews/milestone-M-1-0001.json"
  (cd "$dir" && git add -A && git commit -q -m "M-1.1: record the milestone review")
  printf '%s\n' "$dir"
}
setup_milestone_review_bookkeeping() {
  _setup_milestone_review_bookkeeping milestone-review-bookkeeping
}
setup_milestone_review_driver() {
  _setup_milestone_review_bookkeeping milestone-review-driver
}
invoke_milestone_review_bookkeeping() {
  bash "$1/scripts/check-milestone-review.sh" --milestone M-1
}

# ⚠️ The same fixture through the **driver**, because `M1.46` put a
# byte-identical branch in `milestone-review.sh` and review pointed out only
# the gate's copy had a case — so the defect that survived a whole round
# untouched could return with the suite green. ⚠️ `coverage` is just the
# cheapest subcommand to invoke — no remedy line names it (`M2.8`, M1.48's
# minor b; they name `commits` and `context`) — and the branch sits above
# the `case`, so any one subcommand covers them all.
invoke_milestone_review_driver() {
  bash "$1/scripts/milestone-review.sh" coverage --milestone M-1
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
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/benches"
  # ⚠️ A roadmap, because the gate reads milestone states to expire an
  # allowlist reason (`M3.35`). Without one it reports a missing file as well
  # as the defect the fixture plants — a second failure that would let this
  # case pass while the branch it exists to pin had stopped running.
  # ⚠️ **Every milestone the *real* allowlist owes must be here and open**, or
  # the gate fails once per entry on "owes 'M14', which … does not list" — a
  # first draft used an unrelated id and turned one planted failure into nine.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | not started |
ROADMAP
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
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  # ⚠️ A roadmap, for the reason the fixture above gives: without one the gate
  # reports a missing file as well as the defect this plants, and a case that
  # fails for two reasons passes while the branch it pins stops running.
  # ⚠️ **Every milestone this fixture's allowlist owes must be here and open**,
  # or the excuse expires and the gate fails for *that* instead.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | not started |
ROADMAP
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

# --- check-hot-path-bench.sh: an allowlist reason waits on a milestone that
# has closed -- the expiry `M3.35` added, and the one branch the real tree
# cannot exercise incidentally ------------------------------------------
#
# ⚠️ **The deliverable of `M3.35`, and it was unpinned for one round.** Every
# other branch of this gate can fire against the working tree by accident; this
# one fires for the first time only when M14 closes, which is years away and is
# the moment nobody will be reading the allowlist. Round 3's review measured
# that deleting it left the whole suite green.
setup_hot_path_bench_expired() {
  local dir; dir="$(new_scratch hot-path-bench-expired)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  # ⚠️ **M14 complete, M5 open**, and both listed because the real allowlist
  # owes them: a fixture missing either fails on "does not list" instead, which
  # is a different branch and would let this case pass for the wrong reason.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | complete |
ROADMAP
  # All eight of `NOT_YET_BUILT`'s real names, verbatim, for the reason the
  # required-row fixture gives: a shorter table makes every real entry "stale"
  # and the gate fails on *that* instead.
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

## Next section
EOF
  # ⚠️ A workspace, because the gate skips outright without one — a skip is
  # not a failure, so the case would pass for the wrong reason.
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
  (cd "$dir" && git add -A && git commit -q -m "M3.35: an allowlist reason outlives the milestone that owed it")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_expired() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a row is covered but NOT_YET_BUILT still
# lists it -- the loophole that let a later regression go unnoticed --------
setup_hot_path_bench_leftover() {
  local dir; dir="$(new_scratch hot-path-bench-leftover)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/benches"
  # ⚠️ A roadmap, because the gate reads milestone states to expire an
  # allowlist reason (`M3.35`). Without one it reports a missing file as well
  # as the defect the fixture plants — a second failure that would let this
  # case pass while the branch it exists to pin had stopped running.
  # ⚠️ **Every milestone the *real* allowlist owes must be here and open**, or
  # the gate fails once per entry on "owes 'M14', which … does not list" — a
  # first draft used an unrelated id and turned one planted failure into nine.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | not started |
ROADMAP
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

# --- check-hot-path-bench.sh: the roadmap file is missing, so an allowlist
# reason can never expire (`M3.36`'s own expiry mechanism, unproven until
# now) ------------------------------------------------------------------
setup_hot_path_bench_no_roadmap() {
  local dir; dir="$(new_scratch hot-path-bench-no-roadmap)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/crates/oqueue-x/src"
  # No docs/internal/product/roadmap.md at all -- the branch this fixture
  # plants. All eight of NOT_YET_BUILT's real names, verbatim, for the reason
  # every other fixture in this group gives: a shorter table makes an
  # unrelated entry "stale" and the gate fails on that instead.
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: no roadmap.md exists, so no excuse can ever expire")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_no_roadmap() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a roadmap row does not end in '|', so its
# state cell cannot be located -------------------------------------------
#
# ⚠️ **Isolated on M5, not M14.** Every `NOT_YET_BUILT` entry the real script
# carries owes M14, so breaking M14's own row here would also make every
# entry read "owes 'M14', which … does not list" -- the branch the
# owes-unlisted case below pins, not this one. M5 is listed in the table but
# owed by nothing, so corrupting its row trips exactly one failure.
setup_hot_path_bench_row_no_pipe() {
  local dir; dir="$(new_scratch hot-path-bench-row-no-pipe)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  # M5's row has no trailing '|' -- legal GFM, one hand-edit away, and the
  # exact case `awk -F'|' '{print $(NF-1)}'` cannot answer without it.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | not started |
ROADMAP
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: a roadmap row with no trailing pipe hides its own state")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_row_no_pipe() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a roadmap state outside the vocabulary -------
#
# ⚠️ **Fires on M14 on purpose, and the cascade that follows is real, not a
# fixture bug.** Every NOT_YET_BUILT entry owes M14; once M14's state cannot
# be read, `MILESTONE_STATE["M14"]` is never set and every entry also reports
# "owes 'M14', which … does not list" -- the same outcome an actually
# unreadable roadmap has on every excuse that depends on it. The primary
# pin below is present in the output alongside that cascade, not instead of
# it.
setup_hot_path_bench_bad_state() {
  local dir; dir="$(new_scratch hot-path-bench-bad-state)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | paused |
ROADMAP
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: a roadmap state outside complete/in progress/not started")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_bad_state() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: the same milestone id listed twice -----------
#
# ⚠️ **The first row wins, and this fixture's second row's state is chosen so
# that wins cleanly.** `MILESTONE_STATE` is set from the first M14 row it
# sees and never overwritten, so as long as that first row's state is a real,
# open one ("not started"), the duplicate is the only thing that fails --
# unlike the bad-state case above, nothing here cascades into the
# owes-unlisted branch.
setup_hot_path_bench_duplicate_id() {
  local dir; dir="$(new_scratch hot-path-bench-duplicate-id)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
| 2 | [M14](milestones/M14.md) | Performance and cost validation | non-functional | — | 1 | `x` | not started |
| 3 | [M14](milestones/M14.md) | Performance and cost validation, copied | non-functional | — | 1 | `x` | complete |
ROADMAP
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: a roadmap id listed twice, second row silently ignored")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_duplicate_id() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: a roadmap with no milestone rows at all ------
setup_hot_path_bench_no_milestones() {
  local dir; dir="$(new_scratch hot-path-bench-no-milestones)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  # A real file, present and readable, but with no row matching
  # `^\| *[0-9]+ *\| *\[M` -- the shape a rewrite that dropped the numbered
  # list, or renamed the ID column, would produce.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
# Roadmap

Nothing here yet.
ROADMAP
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: a roadmap.md with no milestone rows at all")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_no_milestones() {
  bash "$1/scripts/check-hot-path-bench.sh"
}

# --- check-hot-path-bench.sh: an allowlist entry owes a milestone the
# roadmap does not list at all (as opposed to lists-but-complete, which the
# expired-excuse case above already pins) ---------------------------------
setup_hot_path_bench_owes_unlisted() {
  local dir; dir="$(new_scratch hot-path-bench-owes-unlisted)"
  copy_gate "$dir" check-hot-path-bench.sh
  mkdir -p "$dir/docs/internal/standards" "$dir/docs/internal/product" \
    "$dir/crates/oqueue-x/src"
  # M14 -- the milestone every real NOT_YET_BUILT entry owes -- is simply
  # absent, rather than present-and-broken. Only M5 is listed.
  cat > "$dir/docs/internal/product/roadmap.md" <<'ROADMAP'
| # | ID | Milestone | Kind | Depends on | Tasks | Completion condition | State |
|---|---|---|---|---|---|---|---|
| 1 | [M5](milestones/M5.md) | Compaction and retention | functional | — | 1 | `x` | not started |
ROADMAP
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-x/src/lib.rs"
  (cd "$dir" && git add -A && git commit -q -m "M3.36: an allowlist entry owes a milestone the roadmap never lists")
  printf '%s\n' "$dir"
}
invoke_hot_path_bench_owes_unlisted() {
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

# _portability_scratch <name>: a scratch repo with check-portability.sh and one
# valid skill, ready for a check-3 fixture to plant exactly one defect.
_portability_scratch() {
  local dir; dir="$(new_scratch "$1")"
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
  echo "# oqueue" > "$dir/AGENTS.md"
  echo "# Skills" > "$dir/.agents/skills/README.md"
  printf '%s\n' "$dir"
}

# --- check-portability.sh: a skill invoking a script that does not exist ----
#
# `M0.28`, check 3's **positive** direction. A standard may name a script
# nobody has written — that is what `roadmap.md`'s deferral table is for — but a
# skill naming one cannot run, so its absence is a defect and not a schedule.
# ⚠️ Unpinned until now: `M0.24`'s body measured that deleting this assertion
# left the whole suite green.
setup_portability_missing_script() {
  local dir; dir="$(_portability_scratch portability-missing)"
  cat >> "$dir/.agents/skills/tdd/SKILL.md" <<'EOF'

Then run `scripts/check-nothing.sh`, which nobody wrote.
EOF
  (cd "$dir" && git add -A && git commit -q -m "M0.28: a skill invoking a script nobody wrote")
  printf '%s\n' "$dir"
}

# --- check-portability.sh: the per-file population split --------------------
#
# `M0.28`. ⚠️ **The fix from `M0.24`'s first review round, pinned by nothing
# until now.** Each claim is judged against the population it is *about*: the
# README's against scripts skills invoke, `AGENTS.md`'s against scripts the
# standards name. Merge the two and this fixture goes green — the merged set
# contains the standard's missing script, so both halves skip and the README's
# false claim survives. Measured on `M0.24`.
setup_portability_merged_population() {
  local dir; dir="$(_portability_scratch portability-merged)"
  # Every script a *skill* names exists, so the README's claim is false ...
  cat > "$dir/.agents/skills/README.md" <<'EOF'
# Skills

A few scripts these skills invoke are still unwritten, so a skill that says
"run the gate" describes an intended step rather than an available one.
EOF
  # ... while a *standard* names one that does not, which is legitimate and is
  # exactly what a merged population would hide behind.
  cat > "$dir/docs/internal/standards/security.md" <<'EOF'
# Security

5. Every decoder has a fuzz target. → `scripts/fuzz.sh`
EOF
  (cd "$dir" && git add -A && git commit -q -m "M0.28: a false skills claim behind a real standards deferral")
  printf '%s\n' "$dir"
}

# --- check-portability.sh: AGENTS.md's own arm of check 3 -------------------
#
# `M0.28`. ⚠️ **The two populations need two cases, not one.** The merged-
# population case above pins that the sets are *separate*; nothing pinned that
# `AGENTS.md` is read at all — deleting its entry from `populations` left all
# fifty-one cases green while the file check 3's header names went unchecked.
# Found by review, after the three the row set out to pin.
setup_portability_agents_arm() {
  local dir; dir="$(_portability_scratch portability-agents)"
  # Every script a *standard* names exists, so AGENTS.md's claim is false.
  cat > "$dir/docs/internal/standards/security.md" <<'EOF'
# Security

5. Every decoder has a fuzz target. → `scripts/check-crate.sh`
EOF
  cat > "$dir/AGENTS.md" <<'EOF'
# oqueue

⚠️ A rule whose script is missing is a preference, and some of the scripts
these standards name are still missing.
EOF
  (cd "$dir" && git add -A && git commit -q -m "M0.28: AGENTS.md calling a present script missing")
  printf '%s\n' "$dir"
}

# --- check-portability.sh: check 3 with nothing to inspect ------------------
#
# `M0.28`. ⚠️ A check that inspects nothing and exits 0 is the vacuous-green
# shape this repository keeps finding; the guard exists so check 3 cannot be
# that, and until now deleting the guard left the suite green.
setup_portability_nothing_named() {
  local dir; dir="$(new_scratch portability-nothing)"
  copy_gate "$dir" check-portability.sh
  mkdir -p "$dir/.agents/skills/tdd"
  cat > "$dir/.agents/skills/tdd/SKILL.md" <<'EOF'
---
name: tdd
description: Use when implementing a task test-first.
---

# TDD

Write the test first. This skill names no script at all.
EOF
  echo "# oqueue" > "$dir/AGENTS.md"
  echo "# Skills" > "$dir/.agents/skills/README.md"
  (cd "$dir" && git add -A && git commit -q -m "M0.28: nothing for check 3 to inspect")
  printf '%s\n' "$dir"
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
  # ⚠️ **`[profile.mutants]`, because `mutants.sh` passes `--profile mutants`
  # and cargo refuses a profile nobody defined.** Without it every
  # `check-mutants.sh` case here failed with `profile 'mutants' is not
  # defined` — still a non-zero exit, so `run_case` counted them green while
  # testing nothing about mutation at all. The two pre-existing cases had been
  # passing that way since `M4.19` added the profile to the real workspace and
  # not to this fixture; `M4.32` found it by adding a case whose expected
  # message could not appear. That is the third way this fixture has been
  # vacuously green, after the two its own comment above records.
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = ["crates/k"]
resolver = "2"

[profile.mutants]
inherits = "dev"
debug = false
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
  # ⚠️ Passes a crate explicitly, so the scope label in the pin is intended
  # rather than incidental. The -p path had no case at all before this;
  # `M1.36` gave the failing-test fixture the same treatment, so there are
  # two now.
  bash "$1/scripts/check-crate.sh" k
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
  # ⚠️ `k`, not no argument — the same fix `M1.27` made to the clippy case, and
  # for the same reason: `check-crate.sh` builds its label from whatever scope
  # it is given, so `workspace` was true only because nothing was passed.
  # `M1.36`. Reproduced before fixing.
  bash "$1/scripts/check-crate.sh" k
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

# --- check-conformance-matrix.sh --------------------------------------------
#
# `M1.21`. ⚠️ **These five cases are why the comparison is its own script.**
# Review measured two silent regressions in it while it was still a section of
# `m1-complete.sh` — a status no row matches made one direction vacuous, and
# deleting the other direction's loop left the gate exiting 0 — and neither
# could have a case here, because reaching that section meant scaffolding a
# cargo workspace *and* a Docker daemon. Split out, the whole input is two
# text files.

# Builds a scratch repo holding a matrix and (optionally) a roster.
_conformance_scaffold() {
  local dir="$1" matrix="$2" roster="${3:-}"
  copy_gate "$dir" check-conformance-matrix.sh
  mkdir -p "$dir/baselines"
  printf '%s\n' "$matrix" > "$dir/baselines/conformance-matrix.txt"
  if [[ -n "$roster" ]]; then
    mkdir -p "$dir/target/conformance"
    # ⚠️ **No trailing newline**, deliberately. `record_backend_run` always
    # writes one, but a hand-edited or truncated roster may not, and the
    # `while read` that consumes it silently dropped the final line -- which
    # is the line these fixtures put the defect on.
    printf '%s' "$roster" > "$dir/target/conformance/backends.txt"
  fi
  # ⚠️ Staged, because the gate reads the matrix from the **index** — an
  # unstaged one would make every case below fail for the wrong reason.
  git -C "$dir" add -A
}

# Two rows for one backend: direction 2's comparison stops at the first
# match, so this passes in one order and fails in the other.
setup_conformance_matrix_duplicate_backend() {
  local dir; dir="$(new_scratch conformance-duplicate)"
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
fake  not-yet-run  the same backend, contradicting the row above'
  printf '%s\n' "$dir"
}

# ⚠️ `m1-complete.sh`'s own early guard, which needs neither Docker nor
# cargo to reach -- the rest of that gate does, which is why the comparison
# it delegates to has the five cases here instead.
setup_m1_complete_no_matrix() {
  local dir; dir="$(new_scratch m1-complete-no-matrix)"
  mkdir -p "$dir/scripts/gates"
  cp "$REPO_ROOT/scripts/gates/m1-complete.sh" "$dir/scripts/gates/m1-complete.sh"
  chmod +x "$dir/scripts/gates/m1-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = []
resolver = "2"
EOF
  # No baselines/conformance-matrix.txt at all.
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m1_complete_no_matrix() {
  bash "$1/scripts/gates/m1-complete.sh"
}

# ⚠️ `m2-complete.sh`'s early guard, the same shape as `m1-complete.sh`'s
# above: the public protocol-support matrix is the artifact that gate
# vouches for, and its absence must fail before any leg needing cargo or a
# Kafka client is reached.
setup_m2_complete_no_matrix() {
  local dir; dir="$(new_scratch m2-complete-no-matrix)"
  mkdir -p "$dir/scripts/gates"
  cp "$REPO_ROOT/scripts/gates/m2-complete.sh" "$dir/scripts/gates/m2-complete.sh"
  chmod +x "$dir/scripts/gates/m2-complete.sh"
  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = []
resolver = "2"
EOF
  # No docs/protocol-support.md at all.
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m2_complete_no_matrix() {
  bash "$1/scripts/gates/m2-complete.sh"
}

# ⚠️ `m3-complete.sh`'s zero-LIST leg is the only one of its five that is
# not a `cargo test` filter, and it is the one most easily made vacuous: it
# reads two files' *shape*, so a moved path would leave NFR-30 asserted by
# nothing while every other line still printed green. Two cases, because
# the two failure modes are different — the seam being gone, and the seam
# having grown a fourth method.
setup_m3_complete_no_store_seam() {
  local dir; dir="$(new_scratch m3-complete-no-store-seam)"
  mkdir -p "$dir/scripts/gates" "$dir/crates/oqueue-core/src"
  cp "$REPO_ROOT/scripts/gates/m3-complete.sh" "$dir/scripts/gates/m3-complete.sh"
  chmod +x "$dir/scripts/gates/m3-complete.sh"
  # `op_counts.rs` is present; `store.rs` is not. The gate must name the
  # missing one rather than reporting both or neither.
  cp "$REPO_ROOT/crates/oqueue-core/src/op_counts.rs" "$dir/crates/oqueue-core/src/op_counts.rs"
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m3_complete_no_store_seam() {
  bash "$1/scripts/gates/m3-complete.sh"
}

# A fourth method on the trait — the change `ADR-0009` §2 says must reopen
# the decision rather than widen this line. It has to fail *before* the
# milestone-review leg can mask it, which is why this scratch repo carries
# a `check-milestone-review.sh` that passes.
setup_m3_complete_store_grew_a_method() {
  local dir; dir="$(new_scratch m3-complete-store-grew)"
  mkdir -p "$dir/scripts/gates" "$dir/crates/oqueue-core/src"
  cp "$REPO_ROOT/scripts/gates/m3-complete.sh" "$dir/scripts/gates/m3-complete.sh"
  chmod +x "$dir/scripts/gates/m3-complete.sh"
  cp "$REPO_ROOT/crates/oqueue-core/src/op_counts.rs" "$dir/crates/oqueue-core/src/op_counts.rs"
  cat > "$dir/crates/oqueue-core/src/store.rs" <<'EOF'
pub trait ObjectStore {
    fn get(&self) {}
    fn put(&self) {}
    fn delete(&self) {}
    fn list(&self) {}
}
EOF
  cat > "$dir/scripts/check-milestone-review.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  chmod +x "$dir/scripts/check-milestone-review.sh"
  # No Cargo.toml: `has_rust` is false, so every cargo leg is skipped. The
  # structural check runs *above* that guard, which is the property this
  # case protects — move it below and the gate exits 0 with the leg unrun.
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m3_complete_store_grew_a_method() {
  bash "$1/scripts/gates/m3-complete.sh"
}

# ⚠️ The two shapes round one's reviewer used to defeat the first version of
# section 0c, kept as cases so the fix is pinned rather than remembered. A
# `List` carrying data is what a `MaintenanceStore` counter would actually
# look like (`ADR-0009` §2), and a supertrait is how a method arrives on a
# trait without appearing in its method list.
_m3_scratch_with_seams() {
  local dir; dir="$(new_scratch "$1")"
  mkdir -p "$dir/scripts/gates" "$dir/crates/oqueue-core/src"
  cp "$REPO_ROOT/scripts/gates/m3-complete.sh" "$dir/scripts/gates/m3-complete.sh"
  chmod +x "$dir/scripts/gates/m3-complete.sh"
  cp "$REPO_ROOT/crates/oqueue-core/src/store.rs" "$dir/crates/oqueue-core/src/store.rs"
  cp "$REPO_ROOT/crates/oqueue-core/src/op_counts.rs" "$dir/crates/oqueue-core/src/op_counts.rs"
  cat > "$dir/scripts/check-milestone-review.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
  chmod +x "$dir/scripts/check-milestone-review.sh"
  printf '%s\n' "$dir"
}

setup_m3_complete_list_carries_data() {
  local dir; dir="$(_m3_scratch_with_seams m3-complete-list-data)"
  cat > "$dir/crates/oqueue-core/src/op_counts.rs" <<'EOF'
pub enum Operation {
    Get,
    Put,
    Delete,
    List(crate::ObjectKey),
}
EOF
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m3_complete_list_carries_data() {
  bash "$1/scripts/gates/m3-complete.sh"
}

setup_m3_complete_supertrait_lists() {
  local dir; dir="$(_m3_scratch_with_seams m3-complete-supertrait)"
  cat > "$dir/crates/oqueue-core/src/store.rs" <<'EOF'
pub trait Listing {
    fn list(&self);
}

pub trait ObjectStore: Send + Sync + Listing {
    fn get(&self);
    fn put(&self);
    fn delete(&self);
}
EOF
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}
invoke_m3_complete_supertrait_lists() {
  bash "$1/scripts/gates/m3-complete.sh"
}

# A row that records a gap without recording anything about it -- the rule
# `check-mutants.sh` and `check-unsafe.sh` already enforce on their baselines.
setup_conformance_matrix_no_reason() {
  local dir; dir="$(new_scratch conformance-no-reason)"
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
azure  not-yet-run'
  printf '%s\n' "$dir"
}
invoke_conformance_matrix() {
  bash "$1/scripts/check-conformance-matrix.sh"
}

# `M2.11` (closing `M1.55`): the measured hole — deleting the gcs rows left
# every check green, so a never-run backend read as covered. This matrix is
# valid in every other way; only `gcs` is missing.
setup_conformance_matrix_missing_backend() {
  local dir; dir="$(new_scratch conformance-missing-backend)"
  # ⚠️ **Every required backend but `gcs`**, so `gcs` is the one omission the
  # case plants. `M10.3` added `sim` to `REQUIRED_BACKENDS` and not here, which
  # disarmed this case without failing it: the gate then refused the fixture
  # over the *missing* `sim` row, so deleting `gcs` from the required list
  # stopped being detectable while this case stayed green.
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
sim  verified  the deterministic in-process S3
s3  verified  MinIO at T2
s3-real  not-yet-run  deferred, doc 10 #33
gcs-real  not-yet-run  deferred, doc 10 #33'
  printf '%s\n' "$dir"
}

# `M2.11` (closing `M1.51`): the same image pinned to two different tags in
# two tracked files — the gate and CI silently testing different servers.
# ⚠️ The matrix here carries every required backend, `sim` included: a fixture
# missing one fails the required-backend check *as well as* the defect it
# plants, and a case whose exit code has two causes is one the suite can stop
# constraining. `M10.3` fixed that next door and left this one.
setup_conformance_image_pins() {
  local dir; dir="$(new_scratch conformance-image-pins)"
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
sim  verified  the deterministic in-process S3
s3  verified  MinIO at T2
gcs  not-yet-run  no emulator round-trips the client
s3-real  not-yet-run  deferred
gcs-real  not-yet-run  deferred'
  printf 'image: minio/minio:RELEASE.2025-04-22T22-12-26Z\n' > "$dir/a.yml"
  # ⚠️ The decoy tag is assembled via %s so THIS tracked file never carries
  # the literal -- round 1 of M2.11's review measured the fixture itself
  # tripping the real gate: the check greps every tracked file, and
  # negative.sh is one.
  printf 'MINIO_IMAGE="minio/minio:%s"\n' 'RELEASE.2020-01-01T00-00-00Z' > "$dir/b.sh"
  git -C "$dir" add -A
  printf '%s\n' "$dir"
}

# The agreement half only runs when asked for it -- see the script's header
# for why it is not in pre-commit.
invoke_conformance_matrix_roster() {
  bash "$1/scripts/check-conformance-matrix.sh" --against-roster
}

# ⚠️ The sharpest of the five: an unrecognized status makes the row vanish
# from both comparisons *and* from the counts, so the only visible signal is a
# plausible `ok` line with a smaller number in it.
setup_conformance_matrix_bad_status() {
  local dir; dir="$(new_scratch conformance-bad-status)"
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
gcs  not_yet_run  an underscore where the status wants hyphens'
  printf '%s\n' "$dir"
}

# The roster names something the matrix does not -- the sixteen invented
# backend names `M1.21` actually found sitting in the real artifact.
setup_conformance_matrix_unknown_in_roster() {
  local dir; dir="$(new_scratch conformance-unknown-roster)"
  # ⚠️ The unknown name is **last**, and that is the point: the roster has no
  # trailing newline, so a `while read` without the `|| [[ -n ]]` guard drops
  # its final line. With `concurrent-0` first this case passed either way and
  # pinned nothing.
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake' \
    'fake
concurrent-0'
  printf '%s\n' "$dir"
}

# The matrix claims a backend is verified and nothing ran it.
setup_conformance_matrix_verified_never_ran() {
  local dir; dir="$(new_scratch conformance-verified-never-ran)"
  _conformance_scaffold "$dir" 'fake  verified  the in-memory fake
s3  verified  claims a MinIO run that never happened' \
    'fake'
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: check-drift.sh with no THRESHOLD_RE at all -------------
#
# `M0.27`, and ⚠️ **this case exists because the row first argued it could not
# be written.** The argument was that removing a false positive makes a gate
# *stop* failing, which an inverted suite cannot express. True for two of the
# six fixes and false for this one: before the fix the gate **aborted inside
# section 5** and printed nothing further; after it, it reports the unreadable
# constant and carries on into sections 6 and 7. So the pinned string is
# present only after the fix — exactly what `run_case`'s fourth argument tests.
# Found by review, which wrote the case to show the argument was wrong.
setup_m0_complete_no_threshold_re() {
  local dir; dir="$(new_scratch m0-complete-no-tre)"
  _m0_scaffold "$dir" m0-complete-no-tre
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-core/src/lib.rs"
  # Present, so section 5 reaches the extraction; the constant it looks for is
  # the thing that is missing.
  printf '#!/usr/bin/env bash
COVERAGE_FLOOR=85
' > "$dir/scripts/check-coverage.sh"
  printf '#!/usr/bin/env bash
# a drift gate that names no threshold pattern
' > "$dir/scripts/check-drift.sh"
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.27: a drift gate with no THRESHOLD_RE")
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: a .pre-commit-config.yaml PyYAML cannot load ------------
#
# `M0.27`. ⚠️ The fallback is for a missing **module**, not a broken document.
# Catching every exception sent an unloadable config to a regex that happily
# counted it, so the gate reported a hook count and "every gate is invoked" for
# a file `pre-commit` itself cannot read.
setup_m0_complete_unparseable_config() {
  local dir; dir="$(new_scratch m0-complete-badyaml)"
  _m0_scaffold "$dir" m0-complete-badyaml
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
  echo 'pub fn f() {}' > "$dir/crates/oqueue-core/src/lib.rs"
  cat > "$dir/.pre-commit-config.yaml" <<'EOF'
repos:
  - repo: local
    hooks:
      - id: broken
        name: "an unterminated quoted scalar
        entry: /bin/true
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.27: a config pre-commit cannot load")
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: the fallback parse, with PyYAML shadowed ---------------
#
# `M0.27`. The config declares two hooks, one of them `commit-msg` in **block**
# form, so the pre-commit count is 1; the documents claim 2. Under the fallback
# that is a failure — and before the block-sequence handling landed it was a
# silent pass, because `stages:` was invisible and both hooks counted.
setup_m0_complete_fallback() {
  local dir; dir="$(new_scratch m0-complete-fallback)"
  _m0_scaffold "$dir" m0-complete-fallback
  mkdir -p "$dir/scripts/gates" "$dir/docs/internal/product" "$dir/noyaml"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  printf 'raise ImportError("shadowed by the negative suite")\n' > "$dir/noyaml/yaml.py"
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
        entry: /bin/true
        stages: [pre-commit]
      - id: two
        entry: /bin/true
        stages:
          - commit-msg
EOF
  cat > "$dir/docs/internal/product/requirements.md" <<'EOF'
| NFR-56 | Pre-commit suite completes within **10 s**. | measured across 9 hooks — and **2 today** |
EOF
  cat > "$dir/docs/internal/product/roadmap.md" <<'EOF'
NFR-56 was measured across 9 hooks — 10 once the budget gate itself joined
them, and **2 today**.
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.27: a commit-msg hook the fallback must not count")
  printf '%s\n' "$dir"
}
invoke_m0_complete_no_pyyaml() {
  PYTHONPATH="$1/noyaml" bash "$1/scripts/gates/m0-complete.sh"
}

# --- m0-complete.sh: a quoted stage in block form ---------------------------
#
# `M1.23`, from M0's boundary review. ⚠️ **The two parse paths disagreed on a
# config that is valid YAML either way.** The flow branch stripped quotes
# (`stages: ["pre-commit"]`); the block branch did not, so
#   stages:
#     - "pre-commit"
# parsed as `'"pre-commit"'` without PyYAML and as `'pre-commit'` with it —
# the hook vanished from the pre-commit count on exactly the machines the
# fallback exists for.
#
# ⚠️ **Inverted, like the `THRESHOLD_RE` case above.** The fixture is not
# broken: both hooks really are pre-commit hooks, so the true count is 2 and
# the planted docs say 1. Only a gate that strips the quotes counts 2 and
# reports the mismatch; before the fix it counted 1, agreed with the docs, and
# said nothing. The pinned string is therefore present only after the fix,
# which is what `run_case`'s fourth argument tests.
setup_m0_complete_quoted_block_stage() {
  local dir; dir="$(new_scratch m0-complete-quoted-stage)"
  _m0_scaffold "$dir" m0-complete-quoted-stage
  mkdir -p "$dir/scripts/gates" "$dir/docs/internal/product" "$dir/noyaml"
  cp "$REPO_ROOT/scripts/gates/m0-complete.sh" "$dir/scripts/gates/m0-complete.sh"
  chmod +x "$dir/scripts/gates/m0-complete.sh"
  printf 'raise ImportError("shadowed by the negative suite")\n' > "$dir/noyaml/yaml.py"
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
  # Two pre-commit hooks: one in flow form, one in quoted block form.
  cat > "$dir/.pre-commit-config.yaml" <<'EOF'
repos:
  - repo: local
    hooks:
      - id: one
        entry: /bin/true
        stages: [pre-commit]
      - id: two
        entry: /bin/true
        stages:
          - "pre-commit"
EOF
  cat > "$dir/docs/internal/product/requirements.md" <<'EOF'
| NFR-56 | Pre-commit suite completes within **10 s**. | measured across 9 hooks — and **1 today** |
EOF
  cat > "$dir/docs/internal/product/roadmap.md" <<'EOF'
NFR-56 was measured across 9 hooks — 10 once the budget gate itself joined
them, and **1 today**.
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M1.23: a quoted stage in block form")
  printf '%s\n' "$dir"
}

# --- m0-complete.sh: a gate parked at stages: [manual] ----------------------
#
# `M0.27`. ⚠️ A hook at `stages: [manual]` keeps its `entry:` line and runs on
# no commit, so a grep for the script name called it invoked — the
# mention-versus-invocation error this loop already named for comments and
# `name:` keys, one level deeper.
#
# ⚠️ **One of `M0.27`'s six fixes cannot have a case. The row first claimed
# four could not; review disproved that three times.** Two cases turned a
# silent abort into a reported failure, one shadows PyYAML so the fallback is
# executed at all, and one pins the `_msec` re-widening — because narrowing a
# regex to remove a false positive has a second half that *adds* a catch, and
# that half is directly expressible. What remains inexpressible is exactly one
# thing: `_msg` no longer tripping the gate, and a reflowed line no longer
# failing the hook count, both of which make a gate **stop** failing on a good
# artifact. The pass condition here is
# inverted, so a fix that makes a gate *stop* failing on a good artifact — `_msg`
# no longer tripping `check-drift.sh`, a reflowed line no longer failing the
# hook count — is genuinely inexpressible. But a fix that turns a silent abort
# into a **reported** failure is expressible, because the report is a string
# absent before and present after; the two cases above are that. "It cannot be
# tested" is worth one attempt at disproof before it is written down.
setup_m0_complete_manual_stage() {
  local dir; dir="$(new_scratch m0-complete-manual)"
  _m0_scaffold "$dir" m0-complete-manual
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
  # check-crate.sh runs on every commit; check-coverage.sh is parked.
  cat > "$dir/.pre-commit-config.yaml" <<'EOF'
repos:
  - repo: local
    hooks:
      - id: check-crate
        name: crate
        entry: scripts/check-crate.sh
        language: system
        stages: [pre-commit]
      - id: check-coverage
        name: coverage, temporarily parked
        entry: scripts/check-coverage.sh
        language: system
        stages: [manual]
EOF
  cat > "$dir/docs/internal/product/requirements.md" <<'EOF'
| NFR-56 | Pre-commit suite completes within **10 s**. | measured across 9 hooks — and **1 today** |
EOF
  cat > "$dir/docs/internal/product/roadmap.md" <<'EOF'
NFR-56 was measured across 9 hooks — 10 once the budget gate itself joined
them, and **1 today**.
EOF
  (cd "$dir" && cargo generate-lockfile >/dev/null 2>&1)
  (cd "$dir" && git add -A && git commit -q -m "M0.27: a gate parked where no commit runs it")
  printf '%s\n' "$dir"
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
  # ⚠️ The count wrapped onto a second line on purpose: it is correct for this
  # 2-hook config, so this arm must *pass* while the requirements arm fails.
  # Line-scoped extraction reported it as stating no count at all, and the case
  # was green off the other arm only — a control nobody was watching.
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

# ⚠️ **Only runnable where `cargo-llvm-cov` is installed.** Without it the gate
# *skips* and exits 0, which this suite would report as "reported ok on a broken
# artifact" -- asserting a regression that does not exist, and turning
# `m-1-complete.sh` red on any clone that has not installed the tool. Nothing in
# this repository asks anyone to. So the case is registered conditionally and
# says so, rather than failing for a missing tool: `lib.sh`'s own rule is that a
# missing tool is a skip with a named remedy, never a failure.
_have_llvm_cov() { cargo llvm-cov --version >/dev/null 2>&1; }

run_case "check-commit-msg.sh"          setup_commit_msg          invoke_commit_msg
run_case "check-commit-msg.sh (unstaged backlog row)" setup_commit_msg_unstaged_row invoke_commit_msg_unstaged_row
run_case "check-tests-kept.sh"          setup_tests_kept          invoke_tests_kept
run_case "check-drift.sh"               setup_drift               invoke_drift
run_case "check-drift.sh (a _msec threshold)" setup_drift_msec       invoke_drift_msec \
  "threshold made settable"
run_case "check-drift.sh (a raised clippy threshold)" setup_clippy_pin invoke_clippy_pin
run_case "check-drift.sh (a _days threshold)" setup_drift_days       invoke_drift_days \
  "threshold made settable"
run_case "check-drift.sh (a raised Rust bound)" setup_rust_bound     invoke_rust_bound \
  "MAX_READS_PER_REQUEST is '64'"
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
run_case "check-sans-io.sh (a wall clock beside a virtual one)" \
  setup_sans_io_wall_clock invoke_sans_io_wall_clock \
  "a real clock read"
check_sans_io_allows_virtual_clock
check_sans_io_allows_virtual_clock_via_bare_import
run_case "seed-corpus.sh (a seed with no recorded versions)" \
  setup_seed_corpus_unpinned invoke_seed_corpus_unpinned \
  "records no toolchain or tokio version"
run_case "seed-corpus.sh (no corpus file)" \
  setup_seed_corpus_missing invoke_seed_corpus_missing \
  "no corpus at"
run_case "seed-sweep.sh (a failing seed becomes an artifact)" \
  setup_seed_sweep_files_the_artifact invoke_seed_sweep_files_the_artifact \
  "add it to tests/seeds/corpus.tsv"
run_case "seed-sweep.sh (a tree that does not build blames no seed)" \
  setup_seed_sweep_blames_the_build invoke_seed_sweep_blames_the_build \
  "no seed is implicated"
run_case "check-sans-io.sh (a runtime clock in shipped code)" \
  setup_sans_io_virtual_clock_in_src invoke_sans_io_virtual_clock_in_src \
  "a real clock read"
run_case "check-sans-io.sh (a runtime clock in a nested src/tests/ directory)" \
  setup_sans_io_nested_tests_dir_in_src invoke_sans_io_nested_tests_dir_in_src \
  "a real clock read"
run_case "check-sans-io.sh (a clock the broker's seeded runs cannot control)" \
  setup_sans_io_broker_clock invoke_sans_io_broker_clock \
  "a clock no seeded run can control"
check_sans_io_writer_id_stays_exempt
run_case "check-core-contract.sh"       setup_core_contract       invoke_core_contract
run_case "check-core-contract.sh (non-UTF-8 crash)" setup_core_contract_non_utf8 invoke_core_contract_non_utf8
run_case "check-unsafe.sh"              setup_unsafe              invoke_unsafe
run_case "check-unsafe.sh (non-UTF-8 file doesn't suppress a real violation)" setup_unsafe_non_utf8 invoke_unsafe_non_utf8
run_case "check-reviewed.sh"            setup_reviewed            invoke_reviewed
run_case "check-reviewed.sh (regex task_id)" setup_reviewed_regex_task_id invoke_reviewed_regex_task_id
run_case "check-milestone-review.sh"    setup_milestone_review    invoke_milestone_review
run_case "check-milestone-review.sh (lettered task id)" \
  setup_milestone_review_lettered_id invoke_milestone_review_lettered_id
run_case "check-milestone-review.sh (every commit is review bookkeeping)" \
  setup_milestone_review_bookkeeping invoke_milestone_review_bookkeeping \
  "all of them milestone-review bookkeeping"
run_case "milestone-review.sh (every commit is review bookkeeping)" \
  setup_milestone_review_driver invoke_milestone_review_driver \
  "all of them milestone-review bookkeeping"
run_case "build-index.sh --check"       setup_build_index         invoke_build_index \
  "index is stale"
run_case "check-requirements-trace.sh"  setup_requirements_trace  invoke_requirements_trace \
  "cites FR-999"
run_case "check-file-size.sh"           setup_file_size           invoke_file_size
run_case "check-readmes.sh"             setup_readmes             invoke_readmes
run_case "check-readmes.sh (bin/oqueue)" setup_readmes_bin          invoke_readmes_bin \
  "mimalloc"
# ⚠️ **The invariant is one property per *fixture output*, not per gate.**
# `testing.md` rule 20a: a case must pin its message where the fixture's output
# can carry more than one kind of failure, and "a gate with nine `fail`
# branches whose fixture trips exactly one of them needs no pin, which is why
# most cases carry none".
#
# ⚠️ **Measured for `M1.27` over the nine pins `M0.30` added** —
# `build-index.sh --check`, `check-requirements-trace.sh`, the three
# `check-hot-path-bench.sh` cases below, the four `check-crate.sh` cases
# further down — **only the first `check-hot-path-bench.sh` fixture emits more
# than one kind.** ⚠️ **Re-measured at `M3.35`**, which gave this gate a
# milestone-expiry property and a fourth case: the counts below are that
# task's, and the population is now four `check-hot-path-bench.sh` cases
# rather than three. ⚠️ **Re-measured again at `M10.18b`**, which gave the
# `M3.36` expiry mechanism six negative cases of its own — the population is
# now ten `check-hot-path-bench.sh` cases: the original four plus
# no-roadmap, row-no-pipe, bad-state, duplicate-id, no-milestones, and
# owes-unlisted, all pinned below since three of the six cascade into a
# second kind of failure by construction (see each fixture's own comment).
# A measurement in a comment is a fact with a date on it,
# and this one has been re-taken twice. So rule 20a requires a pin for that one, and the other
# eight carry a pin the rule does not require: harmless extra specificity, and
# cheap insurance for the day one of those gates grows a property, but not an
# obligation. An earlier version of this comment claimed the rule demanded all
# nine ("each of their gates emits more than one kind of failure on a single
# fixture"), which is what the backlog row called wrong for eight of them.
# ⚠️ A first attempt at `M1.27` measured a different population — the cases
# nearest this comment, seven of which are portability pins predating `M0.30`
# and owned by their own comment below — and wrongly reported the row's count
# as the error.
#
# That first fixture is still why the pins exist: it emits several failures,
# most from `NOT_YET_BUILT` staleness rather than the unknown marker it
# plants, so deleting the unknown-marker branch outright left the case green,
# the suite green, and `m0-complete.sh` green. The pins here are read off each
# gate's actual output rather than guessed. ⚠️ **The exact count is
# deliberately not written here any more** (`M3.35`): it moved from eight to
# sixteen and back when this gate grew a property and the fixtures were
# corrected, and a number in a comment that nobody re-measures is the thing
# `M1.27` had to re-measure twice already.
run_case "check-hot-path-bench.sh"      setup_hot_path_bench      invoke_hot_path_bench \
  "bench_micro.rs:1:// hot-path: RecordBatch encode/decode"
run_case "check-hot-path-bench.sh (required row)" setup_hot_path_bench_required invoke_hot_path_bench_required \
  "has no benchmark and is not in NOT_YET_BUILT"
run_case "check-hot-path-bench.sh (leftover entry)" setup_hot_path_bench_leftover invoke_hot_path_bench_leftover \
  "NOT_YET_BUILT still lists"
run_case "check-hot-path-bench.sh (expired excuse)" setup_hot_path_bench_expired invoke_hot_path_bench_expired \
  "which is complete"
run_case "check-hot-path-bench.sh (no roadmap)" setup_hot_path_bench_no_roadmap invoke_hot_path_bench_no_roadmap \
  "roadmap.md not found; NOT_YET_BUILT reasons cannot be checked against it"
run_case "check-hot-path-bench.sh (row with no trailing pipe)" setup_hot_path_bench_row_no_pipe invoke_hot_path_bench_row_no_pipe \
  "does not end in '|', so its state cell cannot be located"
run_case "check-hot-path-bench.sh (state outside the vocabulary)" setup_hot_path_bench_bad_state invoke_hot_path_bench_bad_state \
  "has state 'paused', which is not one of complete/in progress/not started"
run_case "check-hot-path-bench.sh (duplicate milestone id)" setup_hot_path_bench_duplicate_id invoke_hot_path_bench_duplicate_id \
  "lists 'M14' more than once, so its state is whichever row came last"
run_case "check-hot-path-bench.sh (no milestone rows at all)" setup_hot_path_bench_no_milestones invoke_hot_path_bench_no_milestones \
  "lists no milestones, so no NOT_YET_BUILT reason can be checked"
run_case "check-hot-path-bench.sh (owes a milestone the roadmap never lists)" setup_hot_path_bench_owes_unlisted invoke_hot_path_bench_owes_unlisted \
  "which docs/internal/product/roadmap.md does not list"
# ⚠️ The two **pre-existing** portability cases carry an `expect` since `M0.24`
# gave the gate a fourth property: their fixtures name no script, so they trip
# check 3's inspected-nothing guard as a *second* problem, and deleting check 1
# outright left the suite green. Same regression `M0.21` found for
# `check-layering.sh`. The four cases below it carry one for the ordinary
# reason — a gate with four properties needs each case pinned to its own.
run_case "check-portability.sh"         setup_portability         invoke_portability \
  "vendor syntax (Claude Code's @import)"
run_case "check-portability.sh (index claims a present script is missing)" setup_portability_stale_claim invoke_portability_stale_claim \
  "says a script is missing, and all 1 it refers to exist"
run_case "check-portability.sh (a skill invokes a script that does not exist)" setup_portability_missing_script invoke_portability_stale_claim \
  "a SKILL.md invokes scripts/check-nothing.sh"
run_case "check-portability.sh (merged populations hide a false claim)" setup_portability_merged_population invoke_portability_stale_claim \
  "says a script is missing, and all 1 it refers to exist"
run_case "check-portability.sh (check 3 inspected nothing)" setup_portability_nothing_named invoke_portability_stale_claim \
  "check 3 inspected nothing"
run_case "check-portability.sh (AGENTS.md's own population)" setup_portability_agents_arm invoke_portability_stale_claim \
  "says a script is missing, and all 1 it refers to exist"
run_case "check-portability.sh (unterminated fence)" setup_portability_unterminated_fence invoke_portability_unterminated_fence \
  "fence"
# ⚠️ The `expect` is `"fails for"`, not the full command line. The gate narrows
# for a non-host target with no cross `cc` — to `--workspace --exclude oqueue
# --exclude oqueue-store` today, and the exclusion list has grown once already
# (`M1.42`), which is a second reason not to pin the scope string — so
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
run_case "m0-complete.sh (a gate parked at stages: [manual])" setup_m0_complete_manual_stage invoke_m0_complete \
  "check-coverage.sh is invoked by neither"
run_case "m0-complete.sh (check-drift.sh names no THRESHOLD_RE)" setup_m0_complete_no_threshold_re invoke_m0_complete \
  "could not read THRESHOLD_RE from scripts/check-drift.sh"
# ⚠️ **This case needs PyYAML, and registering it unconditionally made the
# suite fail on a machine without it** — the gate's *own* `except ImportError`
# branch exists for exactly that machine, and the fixture then reaches the
# regex fallback, prints no `UNPARSEABLE`, and the suite reports the gate
# stopped catching a defect when the truth is a missing module. Misdiagnosing
# remedy, one file over from the row removing them. `skip_case` names
# `m0-complete.sh`, which is not in `M0_GATES`, so this does not cascade into
# section 6's "never watched to fail" check.
_have_pyyaml() { python3 -c 'import yaml' >/dev/null 2>&1; }
if _have_pyyaml; then
  run_case "m0-complete.sh (a config PyYAML cannot load)" setup_m0_complete_unparseable_config invoke_m0_complete \
    "PyYAML cannot parse .pre-commit-config.yaml"
else
  skip_case m0-complete.sh "PyYAML not installed, so the loud-parse-failure path cannot be reached" \
    "install: python3 -m pip install pyyaml"
fi

# ⚠️ **And the fallback itself, by shadowing PyYAML in the invocation.** Nothing
# else in this suite ever takes the `except ImportError` branch, so the whole
# fallback — including the block-sequence `stages:` handling `M0.27` added —
# was unexecuted by any case: deleting it left the suite and `m0-complete.sh`
# green while a PyYAML-less machine miscounted a `commit-msg` hook as a
# pre-commit one. Found by review, which measured exactly that.
run_case "m0-complete.sh (the no-PyYAML fallback miscounts)" setup_m0_complete_fallback invoke_m0_complete_no_pyyaml \
  "hooks, .pre-commit-config.yaml has"
run_case "m0-complete.sh (a quoted stage in block form)" setup_m0_complete_quoted_block_stage invoke_m0_complete_no_pyyaml \
  "has 2"
run_case "check-conformance-matrix.sh (a claimed backend with no row)" setup_conformance_matrix_missing_backend invoke_conformance_matrix \
  "has no row for"
run_case "check-conformance-matrix.sh (one image, two pins)" setup_conformance_image_pins invoke_conformance_matrix \
  "different tags"
run_case "check-conformance-matrix.sh (row with no reason)" setup_conformance_matrix_no_reason invoke_conformance_matrix \
  "is not <backend>  <status>  <why>"
run_case "check-conformance-matrix.sh (duplicate backend row)" setup_conformance_matrix_duplicate_backend invoke_conformance_matrix \
  "more than once"
run_case "m1-complete.sh (no recorded matrix)" setup_m1_complete_no_matrix invoke_m1_complete_no_matrix \
  "the recorded backend matrix is the artifact this gate checks"
run_case "m2-complete.sh (no protocol-support matrix)" setup_m2_complete_no_matrix invoke_m2_complete_no_matrix \
  "the public protocol-support matrix is an M2 deliverable"
run_case "m3-complete.sh (the object-store seam is gone)" setup_m3_complete_no_store_seam invoke_m3_complete_no_store_seam \
  "the zero-LIST claim is asserted from this file's shape"
run_case "m3-complete.sh (ObjectStore grew a fourth method)" setup_m3_complete_store_grew_a_method invoke_m3_complete_store_grew_a_method \
  "ObjectStore's method set is"
run_case "m3-complete.sh (a List variant that carries data)" setup_m3_complete_list_carries_data invoke_m3_complete_list_carries_data \
  "Operation's variants are"
run_case "m3-complete.sh (a supertrait that can list)" setup_m3_complete_supertrait_lists invoke_m3_complete_supertrait_lists \
  "ObjectStore's declaration is"
run_case "check-conformance-matrix.sh (unrecognized status)" setup_conformance_matrix_bad_status invoke_conformance_matrix \
  "which is neither verified nor not-yet-run"
run_case "check-conformance-matrix.sh (roster names an unknown backend)" setup_conformance_matrix_unknown_in_roster invoke_conformance_matrix_roster \
  "which baselines/conformance-matrix.txt does not name"
run_case "check-conformance-matrix.sh (verified backend never ran)" setup_conformance_matrix_verified_never_ran invoke_conformance_matrix_roster \
  "but the roster records no run for it"
run_case "m-1-complete.sh (missing Non-negotiables section)" setup_m1_complete_missing_section invoke_m1_complete_missing_section
run_case "m-1-complete.sh (non-UTF-8 AGENTS.md, crash path)" setup_m1_complete_non_utf8 invoke_m1_complete_non_utf8
run_case "check-crate.sh (unformatted)"  setup_crate_fmt          invoke_crate_fmt \
  "rustfmt: files are not formatted"
run_case "check-crate.sh (stale lockfile)" setup_crate_stale_lock invoke_crate_stale_lock \
  "Cargo.lock is stale or missing"
# ⚠️ `clippy (k)`, not `clippy (workspace)`. The scope label is whatever
# `check-crate.sh` was given, and `workspace` was it only because this fixture
# passed no argument — an incidental detail rather than a stated intent. The
# fixture now passes `k` explicitly, which makes the label true by
# construction, keeps `clippy` inside the pin so a future
# `fail "rustdoc ($label): warnings denied"` could not satisfy this case, and
# gives the gate's per-crate scope a case at all.
run_case "check-crate.sh (clippy warning)" setup_crate_clippy     invoke_crate_clippy \
  "clippy (k): warnings denied"
run_case "check-crate.sh (failing test)" setup_crate_test         invoke_crate_test \
  "tests (k): failing"
setup_budget_over() {
  local dir; dir="$(new_scratch budget_over)"
  copy_gate "$dir" check-budget.sh
  # ⚠️ A timings artifact whose entries belong to *this* process group and
  # exceed the budget. Written directly rather than by running a slow suite:
  # the thing under test is the comparison, and a fixture that took 10 real
  # seconds to prove a 10-second budget would be its own budget problem.
  mkdir -p "$dir/target/timings"
  # ⚠️ **The same key `check-budget.sh` will group on**, which is
  # `OQUEUE_RUN_ID` when it is set and the process group otherwise
  # (`lib.sh`, `M10.24`). Keyed on the pgid alone, this fixture wrote rows
  # the gate could not see whenever the suite itself ran inside the
  # container — the case reported ok on a planted defect, which is what
  # this suite exists to catch.
  local pg; pg="${OQUEUE_RUN_ID:-$(ps -o pgid= -p $$ | tr -d ' ')}"
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
# ⚠️ **Erosion hiding behind a compiling gate.** `check-budget.sh` exempts a run
# where gates over `COMPILING_GATE_MS` account for the overage -- a build, not an
# eroded suite -- but the exemption is conditional on what *remains* being under
# budget. `M1.30` measured that nothing pinned that conjunct: dropping
# `&& warm_ms <= BUDGET_MS`, so any compiling gate exempts the whole run, left
# the suite green. The gate's own header calls that rule "tried and rejected by
# measurement" because it "would have made this gate unfailable from `M0.17`
# onward, since a mutation run is minutes".
#
# The numbers are the header's own worked example: 21 600 ms of build plus
# 19 904 ms of eroded suite must fail.
setup_budget_eroded_behind_a_build() {
  local dir; dir="$(new_scratch budget_eroded_build)"
  copy_gate "$dir" check-budget.sh
  mkdir -p "$dir/target/timings"
  # ⚠️ **The same key `check-budget.sh` will group on**, which is
  # `OQUEUE_RUN_ID` when it is set and the process group otherwise
  # (`lib.sh`, `M10.24`). Keyed on the pgid alone, this fixture wrote rows
  # the gate could not see whenever the suite itself ran inside the
  # container — the case reported ok on a planted defect, which is what
  # this suite exists to catch.
  local pg; pg="${OQUEUE_RUN_ID:-$(ps -o pgid= -p $$ | tr -d ' ')}"
  local now; now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  {
    # One genuine build, well over COMPILING_GATE_MS.
    printf '%s\tcheck-crate.sh\t21600\t%s\n' "$pg" "$now"
    # And a suite that has eroded past the budget on its own, every entry
    # under the threshold so none is mistaken for a second build.
    printf '%s\tcheck-slow-one.sh\t4990\t%s\n' "$pg" "$now"
    printf '%s\tcheck-slow-two.sh\t4980\t%s\n' "$pg" "$now"
    printf '%s\tcheck-slow-three.sh\t4970\t%s\n' "$pg" "$now"
    printf '%s\tcheck-slow-four.sh\t4964\t%s\n' "$pg" "$now"
  } > "$dir/target/timings/gates.tsv"
  printf '%s\n' "$dir"
}
invoke_budget_eroded_behind_a_build() {
  bash "$1/scripts/check-budget.sh"
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

# ⚠️ **A baseline entry that argues nothing, which nothing could see until
# `M4.32`.** The gate walked survivors and asked whether each was argued; it
# never walked the baseline and asked whether each entry still matched one. So
# an entry whose mutant was killed, moved, or stopped being generated
# suppressed nothing and no gate said so — measured inside one task, where a
# single entry needed re-keying four times and each was caught only because
# that mutant kept surviving. This fixture argues a location no mutant occupies
# while the real survivor is argued correctly, so the case cannot pass by
# tripping the survivor loop instead.
setup_mutants_stale_baseline() {
  local dir; dir="$(_crate_scratch mutants_stale)"
  copy_gate "$dir" mutants.sh
  copy_gate "$dir" check-mutants.sh
  mkdir -p "$dir/baselines"
  cat > "$dir/crates/k/src/lib.rs" <<'EOF'
pub fn classify(n: i32) -> &'static str {
    if n < 0 { "negative" } else { "non-negative" }
}

#[test]
fn it_returns_something() {
    let _ = classify(-1);
    let _ = classify(1);
}
EOF
  (cd "$dir" && cargo fmt --all >/dev/null 2>&1 || true)
  # ⚠️ **Every real survivor argued, by bare location prefix.** The fixture
  # yields five (two `classify -> &'static str` replacements and three
  # comparison mutations), and the gate matches on the location prefix — so
  # two entries cover all five. Arguing fewer would fail this case on the
  # *survivor* loop, which is `run_case`'s "not for the reason the fixture
  # plants", and is what the first version of this fixture did.
  cat > "$dir/baselines/mutants.txt" <<'EOF'
crates/k/src/lib.rs:2:5: replace classify  every survivor at this location, argued so this case cannot pass on the survivor loop
crates/k/src/lib.rs:2:10: replace < with  and every survivor at this one
crates/k/src/lib.rs:999:1: replace nothing with nothing  a line no mutant occupies -- the stale suppression this case exists for
EOF
  (cd "$dir" && git add -A && git commit -q -m "M4.32: a baseline that argues nothing")
  printf '%s\n' "$dir"
}
invoke_mutants_stale_baseline() {
  bash "$1/scripts/check-mutants.sh" --full
}

# ⚠️ **And `unviable:` is the exemption**, because a mutant can stop being
# *generated* as well as stop surviving — `cargo mutants` never lists one that
# no longer compiles, which is indistinguishable from one that was killed. An
# entry saying so is a claim a reader can check; silence is not. Same fixture,
# same absent locations, prefix added: the gate must accept it.
setup_mutants_unviable_exempt() {
  local dir; dir="$(setup_mutants_stale_baseline)"
  cat > "$dir/baselines/mutants.txt" <<'EOF'
crates/k/src/lib.rs:2:5: replace classify  every survivor at this location, argued
crates/k/src/lib.rs:2:10: replace < with  and every survivor at this one
crates/k/src/lib.rs:999:1: replace nothing with nothing  unviable: the mutated body no longer compiles under this profile
EOF
  (cd "$dir" && git add -A && git commit -q --amend --no-edit)
  printf '%s\n' "$dir"
}
invoke_mutants_unviable_exempt() {
  # ⚠️ Inverted: this one must **pass**. `run_case` asserts a non-zero exit, so
  # it is driven by hand below rather than through it.
  bash "$1/scripts/check-mutants.sh" --full
}

# ⚠️ **An `unviable:` claim the run disproves.** The prefix exempts an entry
# from the staleness loop, so anyone can write it — and `M4.32` did, on this
# very baseline, re-keying from stale prose instead of from the six-hour log
# beside it and marking a live survivor unviable. The check that catches that
# is one line inside the same loop, and the loop is nothing without a case
# that watches it fire.
setup_mutants_false_unviable() {
  local dir; dir="$(setup_mutants_stale_baseline)"
  cat > "$dir/baselines/mutants.txt" <<'EOF'
crates/k/src/lib.rs:2:5: replace classify  unviable: a claim the run disproves -- this location has survivors
crates/k/src/lib.rs:2:10: replace < with  and every survivor at this one, argued honestly
EOF
  (cd "$dir" && git add -A && git commit -q --amend --no-edit)
  printf '%s\n' "$dir"
}
invoke_mutants_false_unviable() {
  bash "$1/scripts/check-mutants.sh" --full
}

# ⚠️ **`check-mutants.sh`'s own two format guards had no case either** —
# `M4.69`. The task was filed because `M4.60` copied the `unviable:` branch into
# the union script without its case; the same audit found the guards *here*
# equally unguarded, and deleting both left the whole suite green. That is what
# makes "each copy has its own cases" the answer to `build.md` rule 22 rather
# than a claim about one of them.
setup_mutants_no_reason() {
  local dir; dir="$(setup_mutants_stale_baseline)"
  printf 'crates/k/src/lib.rs:2:5: replace classify\n' > "$dir/baselines/mutants.txt"
  (cd "$dir" && git add -A && git commit -q --amend --no-edit)
  printf '%s\n' "$dir"
}
invoke_mutants_no_reason() {
  bash "$1/scripts/check-mutants.sh" --full
}

setup_mutants_prefix_entry() {
  local dir; dir="$(setup_mutants_stale_baseline)"
  printf 'c  argues every survivor whose line starts with it\n' > "$dir/baselines/mutants.txt"
  (cd "$dir" && git add -A && git commit -q --amend --no-edit)
  printf '%s\n' "$dir"
}
invoke_mutants_prefix_entry() {
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

# --- check-backlog-rows.sh: the three defects M4.26 found by hand -----------
#
# ⚠️ Every fixture needs a real `sdd.md` state list, because the gate reads its
# vocabulary from there and refuses to invent one — a fixture without it fails
# on *that*, which `run_case`'s fourth argument would then report as "not for
# the reason the fixture plants". The helper writes the minimum that parses.
# rewrite_line <file> <1-based line> <replacement>: portable in-place edit.
#
# ⚠️ **`sed -i` is not portable and these were the repository's first uses of
# it.** BSD sed reads `-i`'s argument as a *backup suffix*, so `sed -i '1s/.../'`
# on macOS treats the script as the suffix and the path as the script: the
# fixture is never modified, and `run_case` then reports the case failing "not
# for the reason the fixture plants" — for a gate that is working. GNU-only `\n`
# in a replacement has the same problem. `portability.md` rule 2 keeps macOS a
# platform that runs the fast tiers, and this suite is one of them.
rewrite_line() {
  local file="$1" lineno="$2" replacement="$3"
  awk -v n="$lineno" -v repl="$replacement" \
    'NR == n { print repl; next } { print }' "$file" > "$file.tmp"
  mv "$file.tmp" "$file"
}

# append_after_marker <file> <marker> <text>: insert <text> on the line after
# the first line equal to <marker>. Same portability reason as above.
append_after_marker() {
  local file="$1" marker="$2" text="$3"
  awk -v m="$marker" -v tx="$text" \
    '{ print } $0 == m { print tx }' "$file" > "$file.tmp"
  mv "$file.tmp" "$file"
}

_backlog_fixture() {
  local dir="$1" rows="$2"
  mkdir -p "$dir/docs/internal/product" "$dir/docs/internal/standards"
  cat > "$dir/docs/internal/standards/sdd.md" <<'EOF'
A row's state is one of the words below, between the markers the gate reads.

<!-- states:start -->
- `todo` — not started, or started and not landed.
- `done` — the commit that closes it is in history.
- `dissolved` — the row will never be worked.
<!-- states:end -->

These are the open states:

<!-- states:open:start -->
- `todo`
<!-- states:open:end -->
EOF
  # ⚠️ **The seed commit's own row is here deliberately.** It used to commit
  # `M-1.1: seed the fixture` with no `M-1.1` row, so *every* fixture also
  # emitted "a commit in HEAD names M-1.1, which has no row" — which satisfied
  # the expected substring of the one case whose whole point is that leg, making
  # it pass without its planted scenario. Found by review, by deleting that
  # case's own commit and watching it still pass.
  {
    printf '| Task | Title | Notes | State |\n|---|---|---|---|\n'
    printf '| M-1.1 | the fixture'"'"'s own seed row | some criterion | done |\n'
    printf '%s\n' "$rows"
  } > "$dir/docs/internal/product/backlog.md"
  (cd "$dir" && git add -A && git commit -qm "M-1.1: seed the fixture")
}

# A row whose state cell is gone: M11.3's shape as it first appeared.
setup_backlog_no_state() {
  local dir; dir="$(new_scratch backlog-no-state)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row whose state cell went missing | some criterion |'
  printf '%s\n' "$dir"
}
invoke_backlog_no_state() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# Rows with no header and delimiter above them: what a stray newline plus a
# blank line leaves behind, and the half that is invisible to a line-by-line
# scan because each orphan still looks like a valid row on its own.
setup_backlog_orphaned_rows() {
  local dir; dir="$(new_scratch backlog-orphaned-rows)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row before the break | some criterion | done |'
  cat >> "$dir/docs/internal/product/backlog.md" <<'EOF'

| M-1.3 | orphaned by the blank line above | some criterion | done |
| M-1.4 | orphaned too | some criterion | done |
EOF
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_orphaned_rows() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# A task with a commit in HEAD whose row was never flipped: M4.0 and M11.0.
setup_backlog_committed_todo() {
  local dir; dir="$(new_scratch backlog-committed-todo)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a task that has a commit and is still todo | some criterion | todo |'
  (cd "$dir" && git commit -q --allow-empty -m "M-1.2: land the task whose row stayed todo")
  printf '%s\n' "$dir"
}
invoke_backlog_committed_todo() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ The gate refusing to invent a vocabulary is itself a case: without it, a
# `sdd.md` that lost its list would leave the gate checking states against
# nothing and reporting ok.
setup_backlog_no_vocabulary() {
  local dir; dir="$(new_scratch backlog-no-vocabulary)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a fine row | some criterion | done |'
  printf 'this standard no longer names any states\n' \
    > "$dir/docs/internal/standards/sdd.md"
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_no_vocabulary() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ A header whose last column is not `State`, holding real task rows. The
# first version of this gate skipped such a run *silently*, so one missing
# space in `| ... | State|` disabled every leg for that whole table while the
# gate reported ok. Review reproduced it; this keeps it reproduced.
setup_backlog_header_drift() {
  local dir; dir="$(new_scratch backlog-header-drift)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row under a header that does not say State | criterion | done |'
  rewrite_line "$dir/docs/internal/product/backlog.md" 1 \
    '| Task | Title | Notes | Owner |'
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_header_drift() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ **The spacing variant, which is the round-one finding itself.** The case
# above plants `| Owner |` and so exercises the *has-rows* failure branch, not
# the normalization — review pointed out that reverting the fix to
# `header.endswith("| State |")` left all six earlier cases green while
# `| … | State|` disabled every leg for that table. This fixture pins the
# normalization: the header is one space away from the canonical spelling, and
# the row under it has a state the vocabulary does not define.
setup_backlog_header_spacing() {
  local dir; dir="$(new_scratch backlog-header-spacing)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row under a header missing one space | criterion | finished |'
  rewrite_line "$dir/docs/internal/product/backlog.md" 1 \
    '| Task | Title | Notes | State|'
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_header_spacing() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ `sdd.md` says a state is open and `lib.sh` has not been told. Without this
# check `open_task_ids` and the standard disagree about which rows something
# will still act on, which is the `dissolved`-looked-open bug one level up.
setup_backlog_open_drift() {
  local dir; dir="$(new_scratch backlog-open-drift)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a perfectly fine row | criterion | done |'
  append_after_marker "$dir/docs/internal/standards/sdd.md" \
    '<!-- states:open:start -->' '- `superseded`'
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_open_drift() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ A row split at a table's *last* line. Legs 1 and 2 find a split by its
# consequence — rows below orphaned from their header — so this one orphans
# nothing and slipped through until review reproduced it. What is left is a
# line of prose ending in a state cell, outside every table.
setup_backlog_trailing_fragment() {
  local dir; dir="$(new_scratch backlog-trailing-fragment)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row broken in half by a stray newline | the note is cut here | todo |'
  cat >> "$dir/docs/internal/product/backlog.md" <<'EOF'

and the rest of the note landed out here, rendering as a paragraph | todo |

## the next section
EOF
  (cd "$dir" && git add -A)
  printf '%s\n' "$dir"
}
invoke_backlog_trailing_fragment() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ A commit names an id the backlog no longer lists. Leg 3 read
# `states.get(tid) == "todo"`, so a *deleted* row passed silently — review
# reproduced it by removing a `done` row whose commit is in HEAD and got `ok`.
# `check-commit-msg.sh` does not cover it: that validates only the incoming
# subject, never history.
setup_backlog_commit_without_row() {
  local dir; dir="$(new_scratch backlog-commit-without-row)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | the row that survives | criterion | done |'
  (cd "$dir" && git commit -q --allow-empty -m "M-1.9: a commit whose row was deleted")
  printf '%s\n' "$dir"
}
invoke_backlog_commit_without_row() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ A duplicate id whose second row is `done`, hiding a `todo`. The state map
# was last-wins, so the task read as closed; order-dependent, which is why the
# duplicate goes *below* the real row here.
setup_backlog_duplicate_id() {
  local dir; dir="$(new_scratch backlog-duplicate-id)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | the real row | criterion | todo |'
  cat >> "$dir/docs/internal/product/backlog.md" <<'ROWS'
| M-1.2 | a duplicate below it | criterion | done |
ROWS
  (cd "$dir" && git add -A && git commit -q --allow-empty -m "M-1.2: land it")
  printf '%s\n' "$dir"
}
invoke_backlog_duplicate_id() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ An unclosed ``` fence. `lib.sh` blanks fenced lines for every reader, so an
# odd number of fence lines blanks the rest of the file — and the gate reported
# `ok` for a backlog it had stopped reading. Review reproduced it with a `todo`
# row and a state-cell-less row below the fence, both invisible.
setup_backlog_unbalanced_fence() {
  local dir; dir="$(new_scratch backlog-unbalanced-fence)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row above the fence | criterion | done |'
  cat >> "$dir/docs/internal/product/backlog.md" <<'ROWS'

```text
an unclosed fence, and everything below it is blanked

| M-1.3 | a committed row left todo | criterion | todo |
ROWS
  (cd "$dir" && git add -A && git commit -q --allow-empty -m "M-1.3: land it")
  printf '%s\n' "$dir"
}
invoke_backlog_unbalanced_fence() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# ⚠️ Two state cells on one row — the shape M4.26's *first* repair produced,
# reading a split row as truncated and appending a second `| done |`. The leg
# existed with no fixture, so a future narrowing of either of its two regexes
# would have gone unnoticed.
setup_backlog_two_state_cells() {
  local dir; dir="$(new_scratch backlog-two-state-cells)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row repaired by appending a second cell | criterion | done | done |'
  printf '%s\n' "$dir"
}
invoke_backlog_two_state_cells() {
  bash "$1/scripts/check-backlog-rows.sh"
}

# An unescaped `|` inside a cell: the defect `M4.28` escaped out of thirteen
# rows, and the one this gate could not check until they were. ⚠️ **The
# fixture's stray pipe sits where it renders *plausibly***, which is the trap:
# GFM truncates the Notes cell there and fills State from what follows, so this
# row displays a State of `done` while its real state cell says `todo`. A gate
# that read the rendered table would see nothing wrong.
setup_backlog_unescaped_pipe() {
  local dir; dir="$(new_scratch backlog-unescaped-pipe)"
  copy_gate "$dir" check-backlog-rows.sh
  _backlog_fixture "$dir" '| M-1.2 | a row quoting a table fragment | it mentions | done | in a code span | todo |'
  printf '%s\n' "$dir"
}
invoke_backlog_unescaped_pipe() {
  bash "$1/scripts/check-backlog-rows.sh"
}

run_case "check-backlog-rows.sh (row with no state cell)" \
  setup_backlog_no_state invoke_backlog_no_state "has no state cell"
run_case "check-backlog-rows.sh (unescaped pipe inside a cell)" \
  setup_backlog_unescaped_pipe invoke_backlog_unescaped_pipe \
  "unescaped '|' where a task row has exactly 5"
run_case "check-backlog-rows.sh (rows orphaned by a blank line)" \
  setup_backlog_orphaned_rows invoke_backlog_orphaned_rows \
  "no header and delimiter above them"
run_case "check-backlog-rows.sh (committed task still todo)" \
  setup_backlog_committed_todo invoke_backlog_committed_todo \
  "but a commit in HEAD names it"
run_case "check-backlog-rows.sh (sdd.md names no states)" \
  setup_backlog_no_vocabulary invoke_backlog_no_vocabulary \
  "cannot find the state-vocabulary list"
run_case "check-backlog-rows.sh (header's last column is not State)" \
  setup_backlog_header_drift invoke_backlog_header_drift \
  "not 'State', so none of its rows can be checked"
run_case "check-backlog-rows.sh (row split at a table's last line)" \
  setup_backlog_trailing_fragment invoke_backlog_trailing_fragment \
  "is not inside any table"
run_case "check-backlog-rows.sh (header one space from canonical)" \
  setup_backlog_header_spacing invoke_backlog_header_spacing \
  "which sdd.md does not define"
run_case "check-backlog-rows.sh (sdd.md opens a state lib.sh has not been told about)" \
  setup_backlog_open_drift invoke_backlog_open_drift \
  "open_task_ids and the standard disagree"
run_case "check-backlog-rows.sh (commit names an id with no row)" \
  setup_backlog_commit_without_row invoke_backlog_commit_without_row \
  "which has no row"
run_case "check-backlog-rows.sh (duplicate id hiding a todo)" \
  setup_backlog_duplicate_id invoke_backlog_duplicate_id \
  "already has a row at line"
run_case "check-backlog-rows.sh (unclosed code fence blanks the rest of the file)" \
  setup_backlog_unbalanced_fence invoke_backlog_unbalanced_fence \
  "an odd number"
run_case "check-backlog-rows.sh (row ending in two state cells)" \
  setup_backlog_two_state_cells invoke_backlog_two_state_cells \
  "ends in two state cells"

# --- check-fencing-seam.sh: the gate M4's closing review found checking five
# --- of six codes, and which nothing had ever watched fail ------------------
#
# ⚠️ **`M4.36` is why these exist.** The gate held a hardcoded list of
# `M4.11`'s five codes; `Refusal` grew a sixth, `sync_group.rs` constructed it
# directly, and the gate reported "none of M4.11's five fencing codes are
# constructed outside the seam" — a true sentence about the wrong set. It now
# derives the list from `Refusal::error_code`'s own arms, and these cases are
# the proof that each of its legs can fail, which `testing.md` rule 20a asks
# for and which this gate had never had.
#
# ⚠️ **Six variants, because the gate has a floor of six.** Deriving a list
# creates a failure mode a hardcoded one did not have — delete a variant and
# its arm together and every count shrinks in step — so the gate refuses a
# seam that answers fewer codes than this one has always had. A fixture with
# two variants would trip that floor before reaching the defect it plants,
# which is `run_case`'s "failed, but not for the reason the fixture plants".
# Review of `M4.36` found the hole; this comment is why the fixtures are the
# size they are.
_fencing_fixture() {
  local dir="$1" seam handler
  seam="$dir/crates/oqueue-broker/src/fencing.rs"
  handler="$dir/crates/oqueue-broker/src/sync_group.rs"
  mkdir -p "$dir/crates/oqueue-broker/src"
  cat > "$seam" <<'RS'
pub(crate) enum Refusal {
    CoordinatorLoadInProgress,
    NotCoordinator,
    CoordinatorNotAvailable,
    UnknownMember,
    IllegalGeneration,
    RebalanceInProgress,
}

impl Refusal {
    pub(crate) const fn error_code(self) -> i16 {
        match self {
            Self::CoordinatorLoadInProgress => error_codes::COORDINATOR_LOAD_IN_PROGRESS,
            Self::NotCoordinator => error_codes::NOT_COORDINATOR,
            Self::CoordinatorNotAvailable => error_codes::COORDINATOR_NOT_AVAILABLE,
            Self::UnknownMember => error_codes::UNKNOWN_MEMBER_ID,
            Self::IllegalGeneration => error_codes::ILLEGAL_GENERATION,
            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS,
        }
    }
}
RS
  printf 'fn handle() -> i16 { Refusal::NotCoordinator.error_code() }\n' > "$handler"
}

# A handler reaching for a raw constant instead of the seam -- the shape the
# gate exists for, and the one `sync_group.rs` really had.
setup_fencing_constructed_outside() {
  local dir; dir="$(new_scratch fencing-outside)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  printf 'fn other() -> i16 { error_codes::REBALANCE_IN_PROGRESS }\n' \
    >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_constructed_outside() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A ninth site borrowing a refusal's code without calling `fence`** —
# `M4.67`. The scan above sees only `error_codes::` constants, so a handler
# reaching for `Refusal::RebalanceInProgress.error_code()` names none and
# passed it: it answers the right code for a decision it made itself, which is
# the ad hoc `if` the seam exists to remove. M4's third boundary round found
# eight such sites and this gate had seen none of them. ⚠️ The gate's own
# header records the two shapes this pattern still cannot see; the census is a
# tripwire on growth, not a parser.
setup_fencing_eleventh_borrower() {
  local dir; dir="$(new_scratch fencing-borrower)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  # The fixture's handler already borrows once; the ceiling is 8, so plant
  # enough to cross it rather than relying on the fixture's own count.
  local i
  for i in $(seq 1 10); do
    printf 'fn shortcut%s() -> i16 { Refusal::RebalanceInProgress.error_code() }\n' "$i" \
      >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  done
  printf '%s\n' "$dir"
}
invoke_fencing_eleventh_borrower() {
  bash "$1/scripts/check-fencing-seam.sh"
}
run_case "check-fencing-seam.sh (a handler borrowing a Refusal's code past the ceiling)" \
  setup_fencing_eleventh_borrower invoke_fencing_eleventh_borrower \
  "name a Refusal variant outside"

# ⚠️ **And prose about the shortcut is not the shortcut**, which `run_case`
# cannot express and which had no case at all until review asked: the tree sits
# exactly on the ceiling, so deleting the comment strip put the leg back to
# failing a commit that changed no code — and every other fencing fixture plants
# comment-free lines, so the whole suite stayed green without it. The sibling
# `UNKNOWN_SERVER_ERROR` leg records this lesson at length and this one had not
# inherited it.
TOTAL=$((TOTAL + 1))
_fencing_prose_dir="$(new_scratch fencing-prose)"
copy_gate "$_fencing_prose_dir" check-fencing-seam.sh
_fencing_fixture "$_fencing_prose_dir"
# ⚠️ **The fixture must sit *on* the ceiling, which is the real condition.**
# A first version planted the prose against the fixture's single borrower, so
# the count went 1 → 3, stayed under 8, and the case passed with the strip
# deleted — a must-pass case that asserted headroom rather than the repair.
# Seven more real shortcuts put it at exactly 8; the two comment lines then
# decide between `ok` and `FAIL`.
for _i in $(seq 1 7); do
  printf 'fn real%s() -> i16 { Refusal::RebalanceInProgress.error_code() }\n' "$_i" \
    >> "$_fencing_prose_dir/crates/oqueue-broker/src/sync_group.rs"
done
# ⚠️ **The pre-prose count is asserted, so the case pins itself.** It reaches
# the ceiling by arithmetic over `_fencing_fixture`, which it does not own: if
# that helper's handler ever stops borrowing, the case slides under the ceiling
# and passes with the strip deleted — the exact vacuity round 2 caught in its
# first version, re-enterable from another function with no signal.
_fencing_prose_pre="$( ( cd "$_fencing_prose_dir" && bash scripts/check-fencing-seam.sh ) 2>&1 || true )"
if ! grep -qF "8 site(s) name a Refusal outside" <<< "$_fencing_prose_pre"; then
  fail "the prose fixture does not sit on the ceiling, so it pins nothing"
  note "it plants 7 shortcuts and relies on _fencing_fixture planting exactly 1"
  sed 's/^/     /' <<< "$_fencing_prose_pre" >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi
{
  printf '// Prose naming Refusal::IllegalGeneration.error_code() as the shape.\n'
  printf '//! And a module line naming Refusal::UnknownMember.error_code() too.\n'
} >> "$_fencing_prose_dir/crates/oqueue-broker/src/sync_group.rs"
# ⚠️ **This asserts the census leg's own line, not the gate's exit status.**
# `_fencing_fixture` builds two files, so the handler walk further down fails
# for its own good reason and the gate exits non-zero whatever this leg says —
# a must-pass case keyed on the exit would be asserting that other leg.
_fencing_prose_out="$( ( cd "$_fencing_prose_dir" && bash scripts/check-fencing-seam.sh ) 2>&1 || true )"
if grep -qF 'name a Refusal variant outside' <<< "$_fencing_prose_out"; then
  fail "check-fencing-seam.sh counted prose as a handler deciding for itself"
  sed 's/^/     /' <<< "$_fencing_prose_out" >&2
  FAILED_CASES=$((FAILED_CASES + 1))
elif grep -qF 'within the ceiling of' <<< "$_fencing_prose_out"; then
  ok "check-fencing-seam.sh does not count a Refusal named in a comment"
else
  fail "check-fencing-seam.sh's census leg did not run at all against the prose fixture"
  sed 's/^/     /' <<< "$_fencing_prose_out" >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# A variant named in no arm. rustc would refuse this particular file, but the
# property the leg pins is reachable in compiling code through the wildcard
# case below -- this is the direct statement of it.
setup_fencing_variant_without_arm() {
  local dir; dir="$(new_scratch fencing-no-arm)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 7 \
    '    RebalanceInProgress,
    FencedInstance(MemberId),'
  printf '%s\n' "$dir"
}
invoke_fencing_variant_without_arm() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A wildcard arm, which compiles and defeats the derivation.** Every
# variant still "matches" via `_`, so a per-variant check alone would pass,
# while the code the wildcard answers is absent from the derived list and a
# handler may then construct it anywhere. The one shape that makes this leg
# about more than what rustc already enforces.
setup_fencing_wildcard_arm() {
  local dir; dir="$(new_scratch fencing-wildcard)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 18 \
    '            _ => error_codes::REBALANCE_IN_PROGRESS,'
  printf '%s\n' "$dir"
}
invoke_fencing_wildcard_arm() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A variant and its arm deleted together** -- still exhaustive, still
# compiles, and every count shrinks in step, so nothing that *counts*
# notices. The blocking finding of `M4.36`'s own review: the fixed list this
# gate used to carry caught it by naming the codes, and the floor is what
# replaces that.
setup_fencing_shrunk_seam() {
  local dir; dir="$(new_scratch fencing-shrunk)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 7 ''
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 18 ''
  printf 'fn other() -> i16 { error_codes::REBALANCE_IN_PROGRESS }\n' \
    >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_shrunk_seam() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A trailing comment carrying an arrow**, which every earlier version
# of the derivation was fooled by in a different direction — the last one
# stripped to the *last* arrow on the line and so substituted the comment's
# code for the arm's, leaving the count unchanged, the floor satisfied and
# every variant still named while the gate stopped looking for the code a
# handler was constructing directly. A false pass is worse than a false
# failure, and this is the case that keeps it closed.
setup_fencing_comment_arrow() {
  local dir; dir="$(new_scratch fencing-comment-arrow)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 18 \
    '            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS, // was => error_codes::UNKNOWN_SERVER_ERROR'
  printf 'fn other() -> i16 { error_codes::REBALANCE_IN_PROGRESS }\n' \
    >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_comment_arrow() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A group handler with no source file at all.** `group_handler_files` is
# the walk two gates now share — `check-fencing-seam.sh`'s fatal-code leg and
# `m4-complete.sh`'s FR-40 tripwire — and a handler that vanished takes its
# whole module tree out of both while every remaining file still reads clean.
# ⚠️ **This replaced a count.** `M4.48` floored the total at fourteen and
# review pointed out a file count can lawfully shrink: folding a small module
# back into its parent is legal under `code-structure.md` rule 16, and the
# floor then leaves only exits non-negotiable 2 forbids. A handler going
# missing is never legitimate while the API exists, so that is the invariant.
setup_fencing_missing_handler() {
  local dir; dir="$(new_scratch fencing-missing-handler)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  local root="$dir/crates/oqueue-broker/src"
  mkdir -p "$root/join_group/round" "$root/sync_group" "$root/heartbeat"
  local f
  for f in find_coordinator.rs sync_group.rs heartbeat.rs \
           join_group/mod.rs join_group/deadline.rs \
           join_group/round/mod.rs join_group/round/state.rs \
           join_group/round/slot.rs join_group/round/plan.rs \
           join_group/round/close.rs \
           sync_group/deadline.rs sync_group/barrier.rs heartbeat/deadline.rs; do
    printf '// a handler module\n' > "$root/$f"
  done
  # Thirteen files, five handlers minus one: `leave_group` has nothing.
  printf '%s\n' "$dir"
}
invoke_fencing_missing_handler() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A fatal code answered by a group handler, planted in a submodule the
# gate's predecessor never opened.** `M4.48`'s leg walks the five handlers'
# whole module trees rather than five named files, because
# `code-structure.md` rule 16 has split them into fourteen — so this fixture
# plants the code in `sync_group/barrier.rs`, which `M4.43` created after
# the FR-40 tripwire beside it was written, and which that tripwire could
# not see until `M4.50` pointed it at the same walk.
setup_fencing_fatal_giveup() {
  local dir; dir="$(new_scratch fencing-fatal)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  # ⚠️ **Every one of the five handlers, because the walk refuses to answer
  # otherwise.** `group_handler_files` returns 1 and prints nothing when a
  # handler contributes no file — a list that derives itself can go quiet
  # when a file moves, which is what review caught this leg doing with
  # `0 file(s) read`. A fixture naming two handlers would trip *that* refusal
  # instead of the defect it plants, which is `run_case`'s own "failed, but
  # not for the reason the fixture plants". The count of files per handler is
  # free; the set of handlers is not.
  local root="$dir/crates/oqueue-broker/src"
  mkdir -p "$root/join_group/round" "$root/sync_group" "$root/heartbeat"
  local f
  for f in find_coordinator.rs leave_group.rs sync_group.rs heartbeat.rs \
           join_group/mod.rs join_group/deadline.rs \
           join_group/round/mod.rs join_group/round/state.rs \
           join_group/round/slot.rs join_group/round/plan.rs \
           join_group/round/close.rs \
           sync_group/deadline.rs heartbeat/deadline.rs; do
    printf '// a handler module\n' > "$root/$f"
  done
  # The fourteenth, and the one that matters: a submodule created after the
  # gate's predecessor named its five files.
  printf 'fn evil() -> i16 { error_codes::UNKNOWN_SERVER_ERROR }\n' \
    > "$root/sync_group/barrier.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_fatal_giveup() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A block-bodied arm, with no comment involved at all.** rustfmt-stable,
# clippy-clean, and what anyone writes on adding a second statement — and
# invisible to every line-scanning version of this gate, because the arrow
# and the code are on different lines. It is the case that ended the awk:
# no amount of comment stripping reaches it, and `scripts/lib/fencing_seam.py`
# walks arrows through a brace-matched body instead.
setup_fencing_block_bodied_arm() {
  local dir; dir="$(new_scratch fencing-block-arm)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  rewrite_line "$dir/crates/oqueue-broker/src/fencing.rs" 18 \
    '            Self::RebalanceInProgress => {
                error_codes::REBALANCE_IN_PROGRESS
            }'
  printf 'fn other() -> i16 { error_codes::REBALANCE_IN_PROGRESS }\n' \
    >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_block_bodied_arm() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **Each of these three pins the *code*, not the bare phrase.**
# `constructed outside` is a substring of `check-fencing-seam.sh`'s own
# success line — `none of the seam's N fencing codes are constructed outside
# ...` — and every fixture here exits non-zero for a second reason as well,
# because `_fencing_fixture` writes one handler file and the walk refuses.
# So all three reported `ok` with the grep loop deleted: `violations` stays
# 0, the ok line carries the pin, and `run_case` takes its `elif rc != 0`
# branch. `M4.56` found the shape in a case it was adding; `M4.62` is the
# three that predate it. Rule 20a's clause is about telling one *outcome*
# from another, not one gate from another.
run_case "check-fencing-seam.sh (a raw constant outside the seam)" \
  setup_fencing_constructed_outside invoke_fencing_constructed_outside \
  "error_codes::REBALANCE_IN_PROGRESS constructed outside"
run_case "check-fencing-seam.sh (a variant named in no error_code arm)" \
  setup_fencing_variant_without_arm invoke_fencing_variant_without_arm \
  "answers no error_codes:: path"
run_case "check-fencing-seam.sh (a wildcard arm defeating the derivation)" \
  setup_fencing_wildcard_arm invoke_fencing_wildcard_arm \
  "catch-all arm"
run_case "check-fencing-seam.sh (a trailing comment's arrow hiding a violation)" \
  setup_fencing_comment_arrow invoke_fencing_comment_arrow \
  "error_codes::REBALANCE_IN_PROGRESS constructed outside"
run_case "check-fencing-seam.sh (a block-bodied arm hiding a violation)" \
  setup_fencing_block_bodied_arm invoke_fencing_block_bodied_arm \
  "error_codes::REBALANCE_IN_PROGRESS constructed outside"
run_case "check-fencing-seam.sh (a variant and its arm deleted together)" \
  setup_fencing_shrunk_seam invoke_fencing_shrunk_seam \
  "has had at least"
run_case "check-fencing-seam.sh (a group handler answering a fatal code)" \
  setup_fencing_fatal_giveup invoke_fencing_fatal_giveup \
  "answers UNKNOWN_SERVER_ERROR"
# ⚠️ **A variant carrying an explicit discriminant, whose code is then
# constructed outside the seam.** `fencing_seam.py`'s `VARIANT_RE` did not
# match `FencedInstance = 9,`, so that variant never entered the derived
# list, `MIN_VARIANTS` stayed satisfied at six, and the construction below
# passed unnoticed — a false *pass*, which is the one thing this gate exists
# to prevent. Recorded by `M4.36`'s own commit body and unharvested until
# `M4.56`. Measured against the parser as it stood: one fewer line out,
# exit 0.
setup_fencing_discriminant_variant() {
  local dir; dir="$(new_scratch fencing-discriminant)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  # ⚠️ **The seam is written whole, not patched.** An in-place edit that
  # silently matches nothing leaves a fixture that plants no defect, and the
  # first version of the sibling case below did exactly that — `perl -0pi`
  # exits 0 whether or not its pattern matched, so reindenting
  # `_fencing_fixture`'s heredoc would have turned a must-pass case green
  # while testing nothing. Found by review of `M4.56`. ⚠️ It also keeps
  # `perl` out of the suite's dependencies: these were its only callers in
  # the repository, and `portability.md` rule 10 wants a named skip rather
  # than a setup that aborts the run on a host without it.
  cat > "$dir/crates/oqueue-broker/src/fencing.rs" <<'RS'
pub(crate) enum Refusal {
    CoordinatorLoadInProgress,
    NotCoordinator,
    CoordinatorNotAvailable,
    UnknownMember,
    IllegalGeneration,
    RebalanceInProgress,
    FencedInstance = 9,
}

impl Refusal {
    pub(crate) const fn error_code(self) -> i16 {
        match self {
            Self::CoordinatorLoadInProgress => error_codes::COORDINATOR_LOAD_IN_PROGRESS,
            Self::NotCoordinator => error_codes::NOT_COORDINATOR,
            Self::CoordinatorNotAvailable => error_codes::COORDINATOR_NOT_AVAILABLE,
            Self::UnknownMember => error_codes::UNKNOWN_MEMBER_ID,
            Self::IllegalGeneration => error_codes::ILLEGAL_GENERATION,
            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS,
            Self::FencedInstance => error_codes::FENCED_INSTANCE_ID,
        }
    }
}
RS
  printf 'fn evil() -> i16 { error_codes::FENCED_INSTANCE_ID }\n' \
    >> "$dir/crates/oqueue-broker/src/sync_group.rs"
  printf '%s\n' "$dir"
}
invoke_fencing_discriminant_variant() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **An arm answering two codes.** `CODE_RE.search` took the first and
# dropped the rest silently, so the second stayed out of the derived list and
# a handler could construct it anywhere. There is no correct single answer
# for such an arm, so the parser refuses rather than choosing.
setup_fencing_two_codes_in_one_arm() {
  local dir; dir="$(new_scratch fencing-two-codes)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  cat > "$dir/crates/oqueue-broker/src/fencing.rs" <<'RS'
pub(crate) enum Refusal {
    CoordinatorLoadInProgress,
    NotCoordinator,
    CoordinatorNotAvailable,
    UnknownMember,
    IllegalGeneration,
    RebalanceInProgress,
}

impl Refusal {
    pub(crate) const fn error_code(self) -> i16 {
        match self {
            Self::CoordinatorLoadInProgress => error_codes::COORDINATOR_LOAD_IN_PROGRESS,
            Self::NotCoordinator => {
                if true { error_codes::NOT_COORDINATOR } else { error_codes::UNKNOWN_SERVER_ERROR }
            }
            Self::CoordinatorNotAvailable => error_codes::COORDINATOR_NOT_AVAILABLE,
            Self::UnknownMember => error_codes::UNKNOWN_MEMBER_ID,
            Self::IllegalGeneration => error_codes::ILLEGAL_GENERATION,
            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS,
        }
    }
}
RS
  printf '%s\n' "$dir"
}
invoke_fencing_two_codes_in_one_arm() {
  bash "$1/scripts/check-fencing-seam.sh"
}

# ⚠️ **A correct seam whose or-pattern rustfmt wrapped, which must PASS** —
# so `run_case` cannot express it, the same shape as the `unviable:`
# exemption below. The parser took the pattern's *last line* only, so
# `Self::A\n| Self::B =>` lost `Self::A` and the gate reported
# `Refusal::NotCoordinator answers no error_codes:: path` against
# a seam with nothing wrong with it. A false failure costs a commit rather
# than a violation, which is why it ranks below the two cases above — and a
# gate that cries wolf on correct code is one people learn to route around.
setup_fencing_wrapped_or_pattern() {
  local dir; dir="$(new_scratch fencing-wrapped-or)"
  copy_gate "$dir" check-fencing-seam.sh
  _fencing_fixture "$dir"
  cat > "$dir/crates/oqueue-broker/src/fencing.rs" <<'RS'
pub(crate) enum Refusal {
    CoordinatorLoadInProgress,
    NotCoordinator,
    CoordinatorNotAvailable,
    UnknownMember,
    IllegalGeneration,
    RebalanceInProgress,
}

impl Refusal {
    pub(crate) const fn error_code(self) -> i16 {
        match self {
            Self::CoordinatorLoadInProgress => error_codes::COORDINATOR_LOAD_IN_PROGRESS,
            Self::NotCoordinator
            | Self::CoordinatorNotAvailable => error_codes::NOT_COORDINATOR,
            Self::UnknownMember => error_codes::UNKNOWN_MEMBER_ID,
            Self::IllegalGeneration => error_codes::ILLEGAL_GENERATION,
            Self::RebalanceInProgress => error_codes::REBALANCE_IN_PROGRESS,
        }
    }
}
RS
  # ⚠️ **Every handler, because this fixture has to reach `ok` on all four
  # legs, not just the parser's.** `_fencing_fixture` writes `sync_group.rs`
  # alone, which is enough for a case that only has to fail — the walk's own
  # refusal is a non-zero exit like any other. A case asserting a *pass*
  # cannot borrow that shortcut, and the first version of this one reported
  # `refused a legitimate artifact` for the missing handlers rather than for
  # the or-pattern it exists to cover.
  local root="$dir/crates/oqueue-broker/src"
  mkdir -p "$root/join_group/round" "$root/sync_group" "$root/heartbeat"
  local f
  for f in find_coordinator.rs leave_group.rs heartbeat.rs \
           join_group/mod.rs join_group/deadline.rs \
           join_group/round/mod.rs join_group/round/state.rs \
           join_group/round/slot.rs join_group/round/plan.rs \
           join_group/round/close.rs \
           sync_group/deadline.rs sync_group/barrier.rs heartbeat/deadline.rs; do
    printf '// a handler module\n' > "$root/$f"
  done
  printf '%s\n' "$dir"
}

run_case "check-fencing-seam.sh (a group handler with no source file)" \
  setup_fencing_missing_handler invoke_fencing_missing_handler \
  "no source file for handler(s): leave_group"
# ── the baseline judged over a sharded union (M4.60) ───────────────────────
#
# ⚠️ **The converse loop moved out of `check-mutants.sh` when the nightly was
# sharded**, because a shard sees 1/k of the mutants and every baseline entry
# arguing a survivor in another shard looks dead to it. These drive
# `check-mutants-baseline.sh` directly: it needs no cargo and no workspace,
# only a survivors directory and a baseline.
#
# ⚠️ **Two directions, and the second is the one sharding created.** An entry
# that argues nothing must still fail — that is `M4.32`'s property, which
# must survive the move. And a *missing* shard must be refused rather than
# treated as an empty one, because judging the baseline against a partial
# union reports every entry about the absent shard as dead: the same false
# report, arriving through absence instead of through logic.
# ⚠️ **A fresh name per call, not one shared directory.** `new_scratch`
# does `mkdir -p`, so calling it three times with the same name hands back
# the same directory three times; this helper resets the two files it
# writes and nothing else, so a later case writing a third shard file would
# leave it there and inflate the count the must-pass block below checks —
# failing working code. Found by review of `M4.60`.
_baseline_union_dir() {
  local dir; dir="$(_next_scratch mutants-union)"
  mkdir -p "$dir/scripts" "$dir/baselines" "$dir/surv"
  cp "$REPO_ROOT/scripts/check-mutants-baseline.sh" "$REPO_ROOT/scripts/lib.sh" \
    "$dir/scripts/"
  cat > "$dir/baselines/mutants.txt" <<'BASE'
crates/k/src/lib.rs:2:5: replace classify  argued, and it does survive
crates/k/src/lib.rs:9:9: replace nothing  argues a mutant no shard reported
BASE
  printf 'crates/k/src/lib.rs:2:5: replace classify
' > "$dir/surv/1-of-2.txt"
  : > "$dir/surv/2-of-2.txt"
  # ⚠️ **Staged, since `M4.69`.** `new_scratch` already makes the directory a
  # repository; what changed is that `check-mutants-baseline.sh` now reads the
  # baseline from the index alone, as `check-mutants.sh` always did and as its
  # own comment always claimed. A fixture that only writes the file therefore
  # judges an empty baseline and passes for a reason with nothing to do with
  # what it planted. ⚠️ **`git add` only, no commit, and the `|| true` matters**:
  # an earlier version ran `( cd ... && git init && git add && git commit )`,
  # and that subshell returning non-zero aborted this function under `set -e`
  # *before* its final `printf`, so it produced the empty string — `run_case`
  # then ran the gate in the suite's own directory and reported `ok`. Found
  # with `bash -x` after the sibling case below aborted the whole suite.
  ( cd "$dir" && git add -A ) || true
  printf '%s\n' "$dir"
}

setup_baseline_argues_nothing() {
  _baseline_union_dir
}
invoke_baseline_argues_nothing() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}

# ⚠️ **A shard that never reported.** The directory holds one file where the
# caller launched two, so the union is incomplete and the entry arguing the
# absent shard's survivor would read as dead.
setup_baseline_missing_shard() {
  local dir; dir="$(_baseline_union_dir)"
  rm "$dir/surv/2-of-2.txt"
  printf '%s\n' "$dir"
}
invoke_baseline_missing_shard() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}

# ⚠️ **The shard branches themselves, which nothing guarded.** The three
# cases below drive `check-mutants-baseline.sh`; these drive the two
# branches that decide a shard's behaviour in the nightly, and review found
# both deletable with the suite staying green.
#
# ⚠️ **Only `mutants.sh`'s two refusals live here**, because its argument
# parsing precedes `has_rust` and `require_tool`, so they genuinely need no
# cargo. The two that drive `check-mutants.sh` are inside the
# `_have_mutants` guard further down: a stub stands in for the *runner*, but
# `check-mutants.sh` checks for `cargo mutants` itself before ever invoking
# it. A first version of this comment claimed otherwise and put them here.
# ⚠️ **A fresh name per call**, for `_baseline_union_dir`'s reason twelve
# lines up: `new_scratch` does `mkdir -p`, this helper rewrites three paths
# and removes nothing, so call 1's `target/mutants-survivors/1-of-8.txt`
# would survive into call 2. Harmless only until the obvious next case — an
# unsharded run recording *no* survivors file — which would read the
# leftover and fail working code. Review caught this reintroduced in the
# helper written beside the one that had just been fixed.
_shard_fixture() {
  local dir; dir="$(_next_scratch mutants-shard)"
  mkdir -p "$dir/scripts" "$dir/baselines"
  cp "$REPO_ROOT/scripts/check-mutants.sh" "$REPO_ROOT/scripts/lib.sh" "$dir/scripts/"
  # ⚠️ An entry arguing a mutant no shard reports. Under `--full` this is a
  # stale-baseline failure; under `--shard` it must be *silent*, because a
  # shard sees an eighth of the mutants and cannot tell stale from elsewhere.
  printf 'crates/k/src/lib.rs:9:9: replace nothing  argues a mutant nobody reported\n' \
    > "$dir/baselines/mutants.txt"
  printf '[workspace]\nmembers = []\nresolver = "2"\n' > "$dir/Cargo.toml"
  cat > "$dir/scripts/mutants.sh" <<'STUB'
#!/usr/bin/env bash
echo "2 mutants tested: 2 caught"
exit 0
STUB
  chmod +x "$dir/scripts/mutants.sh"
  ( cd "$dir" && git add -A ) >/dev/null 2>&1 || true
  printf '%s\n' "$dir"
}

# ⚠️ **`mutants.sh`'s own refusals**, which decide whether a flag was heard.
setup_shard_without_full() {
  local dir; dir="$(new_scratch mutants-shard-arg)"
  mkdir -p "$dir/scripts"
  cp "$REPO_ROOT/scripts/mutants.sh" "$REPO_ROOT/scripts/lib.sh" "$dir/scripts/"
  printf '[workspace]\nmembers = []\nresolver = "2"\n' > "$dir/Cargo.toml"
  printf '%s\n' "$dir"
}
invoke_shard_without_full() {
  ( cd "$1" && bash scripts/mutants.sh --shard 1/8 )
}
run_case "mutants.sh (--shard without --full)" \
  setup_shard_without_full invoke_shard_without_full \
  "only meaningful with --full"

setup_shard_malformed() {
  setup_shard_without_full
}
invoke_shard_malformed() {
  ( cd "$1" && bash scripts/mutants.sh --full --shard three-eighths )
}
run_case "mutants.sh (a --shard that is not k/n)" \
  setup_shard_malformed invoke_shard_malformed \
  "takes k/n"

# ⚠️ **The `unviable:` branch, which had no case at all** — `M4.69`. `M4.60`
# moved this loop out of `check-mutants.sh` and copied that branch across
# without the case guarding it, so deleting the branch left all three fixtures
# green while the mutated script answered `ok every argued baseline entry still
# argues a live survivor` on a baseline whose `unviable:` claim the run had just
# disproved. That is verbatim the defect `M4.32` committed on
# `baselines/mutants.txt`, and the reason `check-mutants.sh`'s own copy carries
# a dedicated case.
_baseline_false_unviable_dir() {
  local dir; dir="$(_baseline_union_dir)"
  cat > "$dir/baselines/mutants.txt" <<'BASE'
crates/k/src/lib.rs:2:5: replace classify  unviable: cargo mutants no longer generates this one
BASE
  ( cd "$dir" && git add -A ) || true
  printf '%s\n' "$dir"
}
setup_baseline_false_unviable() {
  _baseline_false_unviable_dir
}
invoke_baseline_false_unviable() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}
run_case "check-mutants-baseline.sh (an 'unviable:' claim the union disproves)" \
  setup_baseline_false_unviable invoke_baseline_false_unviable \
  "claims 'unviable:' for a mutant that survived"

# ⚠️ **And the two format guards this copy did not have** (`M4.69`). Both are
# `check-mutants.sh`'s, and the second is the one with a measured consequence:
# this script matches a survivor with `[[ "$s" == "$loc"* ]]`, so a one-character
# entry argues every survivor whose line starts with it.
setup_baseline_no_reason() {
  local dir; dir="$(_baseline_union_dir)"
  printf 'crates/k/src/lib.rs:2:5: replace classify\n' > "$dir/baselines/mutants.txt"
  ( cd "$dir" && git add -A ) || true
  printf '%s\n' "$dir"
}
invoke_baseline_no_reason() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}
run_case "check-mutants-baseline.sh (an entry with no reason)" \
  setup_baseline_no_reason invoke_baseline_no_reason \
  "baseline entry has no reason"

setup_baseline_prefix_entry() {
  local dir; dir="$(_baseline_union_dir)"
  printf 'c  argues every survivor whose line starts with it\n' > "$dir/baselines/mutants.txt"
  ( cd "$dir" && git add -A ) || true
  printf '%s\n' "$dir"
}
invoke_baseline_prefix_entry() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}
run_case "check-mutants-baseline.sh (an entry whose location is not a location)" \
  setup_baseline_prefix_entry invoke_baseline_prefix_entry \
  "does not start with a <file>.rs:<line>:<col>: location"

# ⚠️ **And an unstaged baseline argues nothing** (`M4.69`). The read is the
# index alone, as `check-mutants.sh`'s always was and as this script's own
# comment always claimed; while it fell back to the worktree, a line nobody
# staged suppressed a survivor here and was invisible there.
setup_baseline_unstaged() {
  local dir; dir="$(_baseline_union_dir)"
  # ⚠️ **Nothing about the baseline is staged, and staging an *empty* one
  # instead is what review measured pinning nothing**: `git show :` then
  # succeeds with empty output, the `|| cat` arm is unreachable, and restoring
  # the fallback left this case green. The index must have no entry for the
  # path at all, so a script that falls back to the worktree reads the line
  # below and a script that does not reads nothing.
  ( cd "$dir" && git rm -q --cached baselines/mutants.txt ) >/dev/null 2>&1 || true
  cat > "$dir/baselines/mutants.txt" <<'BASE'
crates/k/src/lib.rs:9:9: replace nothing  unstaged, and it argues a mutant no shard reported
BASE
  printf '%s\n' "$dir"
}
invoke_baseline_unstaged() {
  ( cd "$1" && bash scripts/check-mutants-baseline.sh surv 2 )
}

run_case "check-mutants-baseline.sh (an entry that argues nothing)" \
  setup_baseline_argues_nothing invoke_baseline_argues_nothing \
  "argues no surviving mutant"
run_case "check-mutants-baseline.sh (a shard that never reported)" \
  setup_baseline_missing_shard invoke_baseline_missing_shard \
  "shard file(s) in surv, expected 2"

# --- check-milestone-handoff.sh: a milestone that closed over its own rows ---
#
# The gate `M4.73` added, and the loop it ends: `milestone-review` turns every
# major finding into a backlog row, `sdd.md` decomposes only the *current*
# milestone, so every finding lands in the milestone under review and reading
# the commits that close them is the next round's job. The way out — close with
# the rows open, hand them on in one `roadmap.md` deferral row, let the next
# milestone's opening commit absorb them — is what `M1`→`M2.0` and `M3`→`M10.0`
# each did by hand. This is what makes the handoff checkable rather than a
# promise.
# ⚠️ **A fresh name per call**, `_baseline_union_dir`'s own reason: two cases
# sharing a scratch directory let the second one pass on the first one's files.
_handoff_fixture() {
  local deferred="$1"
  local dir; dir="$(_next_scratch milestone-handoff)"
  copy_gate "$dir" check-milestone-handoff.sh
  mkdir -p "$dir/docs/internal/product" "$dir/docs/internal/standards"
  cat > "$dir/docs/internal/standards/sdd.md" <<'SDD'
<!-- states:start -->
- `todo` — not started.
- `done` — landed.
- `dissolved` — never to be worked.
<!-- states:end -->

<!-- states:open:start -->
- `todo`
<!-- states:open:end -->
SDD
  cat > "$dir/docs/internal/product/backlog.md" <<'BL'
## M-1: the milestone under test

| Task | Title | Notes | State |
|---|---|---|---|
| M-1.1 | a row that landed | some criterion | done |
| M-1.7 | a row the boundary round opened and nobody worked | some criterion | todo |
BL
  {
    printf '## Milestones\n\n'
    printf '| # | Milestone | What | Kind | Depends | Tasks | Gate | State |\n'
    printf '|---|---|---|---|---|---|---|---|\n'
    printf '| 1 | [M-1](milestones/M-1.md) | the milestone under test | functional | - | 2 | `scripts/gates/m-1-complete.sh` | complete |\n'
    printf '\n## Deferred into a later milestone\n\n'
    printf '| Deferred | Into | Why, and what it shapes |\n|---|---|---|\n'
    printf '%s\n' "$deferred"
  } > "$dir/docs/internal/product/roadmap.md"
  (cd "$dir" && git add -A && git commit -qm "M-1.1: seed the fixture")
  printf '%s\n' "$dir"
}

setup_handoff_unnamed() {
  _handoff_fixture '| Something else entirely | M0 (from M-1) | not the row that is open |'
}
invoke_handoff_unnamed() {
  bash "$1/scripts/check-milestone-handoff.sh"
}
run_case "check-milestone-handoff.sh (a complete milestone whose open row is named nowhere)" \
  setup_handoff_unnamed invoke_handoff_unnamed \
  "open row(s) no deferral row names"

# ⚠️ **A range whose ends are different milestones is not expanded**, and
# saying so beats silently expanding it to one of them: `M1`'s real row writes
# `M1.47`-`M1.51`, so ranges are the shape a reader actually uses and a typo in
# one is the shape a reader actually makes. ⚠️ Its *endpoints* are still
# discharged, by the unconditional scan of the same cell — they are written
# down, so they are named; what is lost is everything between them, and that is
# what the complaint is about.
setup_handoff_crossed_range() {
  _handoff_fixture '| Rows `M-1.7`-`M0.9` | M0 (from M-1) | a range across two milestones |'
}
invoke_handoff_crossed_range() {
  bash "$1/scripts/check-milestone-handoff.sh"
}
run_case "check-milestone-handoff.sh (a deferral range spanning two milestones)" \
  setup_handoff_crossed_range invoke_handoff_crossed_range \
  "whose ends are different milestones"

# ⚠️ **An id in the row's *prose* hands nothing on**, and review measured the
# first version of this gate discharging three real ids that way: `roadmap.md`
# has a row whose Why column says "none became a row until `M0.27`-`M0.29`
# harvested them by hand", which is an aside about history, not a handoff.
# Measured against that roadmap: a whole-row scan treats **112** ids as handed
# and the first-cell scan treats **17**.
setup_handoff_named_only_in_prose() {
  _handoff_fixture '| Something else entirely | M0 (from M-1) | an aside mentioning `M-1.7`, which hands it to nobody |'
}
invoke_handoff_named_only_in_prose() {
  bash "$1/scripts/check-milestone-handoff.sh"
}
run_case "check-milestone-handoff.sh (an open row named only in a deferral row's prose)" \
  setup_handoff_named_only_in_prose invoke_handoff_named_only_in_prose \
  "open row(s) no deferral row names"

# ⚠️ **The id form is `lib.sh`'s, not a narrower transcription**, which is the
# hazard `lib.sh`'s own note names: a copy accepting only `a`-`d` made a row
# like `M-1.9z` unsatisfiable — no deferral row could ever discharge it.
TOTAL=$((TOTAL + 1))
_handoff_letter_dir="$(_handoff_fixture '| Rows `M-1.7` and `M-1.9z` | M0 (from M-1) | a lettered id past the fourth letter |')"
cat >> "$_handoff_letter_dir/docs/internal/product/backlog.md" <<'BL'
| M-1.9z | a row whose id carries a late letter | some criterion | todo |
BL
(cd "$_handoff_letter_dir" && git add -A && git commit -qm "M-1.1: a lettered row")
if ( cd "$_handoff_letter_dir" && bash scripts/check-milestone-handoff.sh ) >/dev/null 2>&1; then
  ok "check-milestone-handoff.sh reads the same id form open_task_ids emits"
else
  fail "check-milestone-handoff.sh cannot discharge an id lib.sh considers valid"
  ( cd "$_handoff_letter_dir" && bash scripts/check-milestone-handoff.sh ) 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **Three more positive cases, because three round-1 repairs had none** and
# review measured each: deleting the en-dash class, collapsing the lettered
# expansion to its endpoints, or turning the backwards-range check into `if
# False` each left all 139 cases green. A repair with no case is a repair the
# next edit undoes silently, which is `testing.md` rule 20a's whole subject.
TOTAL=$((TOTAL + 1))
_handoff_endash_dir="$(_handoff_fixture '| Rows `M-1.5`–`M-1.9` | M0 (from M-1) | an en-dash range, which is how this prose writes them |')"
if ( cd "$_handoff_endash_dir" && bash scripts/check-milestone-handoff.sh ) >/dev/null 2>&1; then
  ok "check-milestone-handoff.sh reads an en-dash as a range separator"
else
  fail "check-milestone-handoff.sh stranded a row named inside an en-dash range"
  ( cd "$_handoff_endash_dir" && bash scripts/check-milestone-handoff.sh ) 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

TOTAL=$((TOTAL + 1))
_handoff_lettered_dir="$(_handoff_fixture '| Rows `M-1.7` and `M-1.9a`-`M-1.9c` | M0 (from M-1) | a lettered range |')"
cat >> "$_handoff_lettered_dir/docs/internal/product/backlog.md" <<'BL'
| M-1.9b | a row inside a lettered range | some criterion | todo |
BL
(cd "$_handoff_lettered_dir" && git add -A && git commit -qm "M-1.1: a lettered interior row")
if ( cd "$_handoff_lettered_dir" && bash scripts/check-milestone-handoff.sh ) >/dev/null 2>&1; then
  ok "check-milestone-handoff.sh expands a lettered range over its interior"
else
  fail "check-milestone-handoff.sh stranded M-1.9b inside M-1.9a-M-1.9c"
  ( cd "$_handoff_lettered_dir" && bash scripts/check-milestone-handoff.sh ) 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **A backwards range is reported, not silently expanded to nothing.**
# `range(9, 5 + 1)` is empty, so without the check the row reads as naming its
# two endpoints and the ids between them vanish with no complaint.
setup_handoff_backwards_range() {
  _handoff_fixture '| Rows `M-1.9`-`M-1.5` | M0 (from M-1) | a range written backwards |'
}
invoke_handoff_backwards_range() {
  bash "$1/scripts/check-milestone-handoff.sh"
}
run_case "check-milestone-handoff.sh (a deferral range written backwards)" \
  setup_handoff_backwards_range invoke_handoff_backwards_range \
  "backwards"

# ⚠️ **And it must *pass* on an honest handoff**, which `run_case` cannot
# express — it asserts a non-zero exit. Without these two, a gate that refused
# every complete milestone would satisfy both cases above, and the rule it
# enforces would be "never close a milestone", which is the loop rather than
# the fix for it.
TOTAL=$((TOTAL + 1))
_handoff_named_dir="$(_handoff_fixture '| Rows `M-1.7` | M0 (from M-1) | handed on by name |')"
if ( cd "$_handoff_named_dir" && bash scripts/check-milestone-handoff.sh ) >/dev/null 2>&1; then
  ok "check-milestone-handoff.sh passes when the open row is named"
else
  fail "check-milestone-handoff.sh refused a milestone that handed its open row on"
  ( cd "$_handoff_named_dir" && bash scripts/check-milestone-handoff.sh ) 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

TOTAL=$((TOTAL + 1))
_handoff_range_dir="$(_handoff_fixture '| Rows `M-1.5`-`M-1.9` | M0 (from M-1) | handed on as a range |')"
if ( cd "$_handoff_range_dir" && bash scripts/check-milestone-handoff.sh ) >/dev/null 2>&1; then
  ok "check-milestone-handoff.sh expands a range rather than demanding each id"
else
  fail "check-milestone-handoff.sh refused a row named inside a deferral range"
  ( cd "$_handoff_range_dir" && bash scripts/check-milestone-handoff.sh ) 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **And the union must *pass* on a complete one**, which `run_case` cannot
# express — it asserts a non-zero exit. Without this, a script that refused
# every input would satisfy both cases above.
TOTAL=$((TOTAL + 1))
_baseline_ok_dir="$(_baseline_union_dir)"
printf 'crates/k/src/lib.rs:9:9: replace nothing\n' > "$_baseline_ok_dir/surv/2-of-2.txt"
if ( cd "$_baseline_ok_dir" && bash scripts/check-mutants-baseline.sh surv 2 ) >/dev/null 2>&1; then
  ok "check-mutants-baseline.sh passes when every entry argues a live survivor"
else
  fail "check-mutants-baseline.sh refused a complete, honest union"
  note "$( ( cd "$_baseline_ok_dir" && bash scripts/check-mutants-baseline.sh surv 2 ) 2>&1 | sed 's/^/     /')"
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **And the union judges the *staged* baseline**, which is a different
# property from the two above and needs its own must-pass case: a script that
# refused every input would satisfy the red cases, and one that read the
# worktree would satisfy this file's other green ones. `M4.69`.
TOTAL=$((TOTAL + 1))
_baseline_unstaged_dir="$(setup_baseline_unstaged)"
if invoke_baseline_unstaged "$_baseline_unstaged_dir" >/dev/null 2>&1; then
  ok "check-mutants-baseline.sh judges the staged baseline, not the worktree"
else
  fail "check-mutants-baseline.sh read an unstaged baseline line"
  invoke_baseline_unstaged "$_baseline_unstaged_dir" 2>&1 | sed 's/^/     /' >&2
  FAILED_CASES=$((FAILED_CASES + 1))
fi


# ── a harness leg's own broker outliving its ceiling (M4.61) ───────────────
#
# ⚠️ **`run_bounded` signals the direct child only**, and three harness legs
# start an `oqueue serve` of their own, so a ceiling that fires sends
# `SIGTERM` to python and leaves the broker reparented.
# `scripts/harness/reap.py` installs the handler that stops that, and this
# drives its own self-test. It uses a `sleep` rather than a broker because
# the property is the signal path, and `oqueue serve` would need a built
# workspace the negative suite does not have.
#
# ⚠️ **Both arms**, because "the child did not outlive us" proves nothing on
# its own: a child that was never started does not outlive anything either.
# The untracked arm is what shows the leak is real and that `track` closes
# it.
#
# ⚠️ **An earlier version dropped the untracked arm on a false reading.** It
# failed with `left the child alive=no, wanted yes`, which was recorded as
# "the untracked `sleep` dies with its parent anyway in this environment".
# It does not: the child inherited python's stdout, so the command
# substitution never returned and the case never sampled liveness at all.
# `reap.py`'s self-test passes `stdout=subprocess.DEVNULL` now and both arms
# report in about a second each. Found by review of `M4.61` — a measurement
# that looked like evidence for removing a test was an artefact of the test
# harness.
#
# ⚠️ **The handler's own marker is pinned**, not just liveness. `reap.py`
# prints `HANDLER DID NOT FIRE` when `SIGTERM` was noted and never
# delivered, and with `atexit` still registered the child is reaped on that
# path too — so a change that kept the reaper and lost the unwinding handler
# left `alive=no` and the case green, while the legs' `finally` blocks
# stopped running and the leg's exit status moved from 143 to 1, which
# `kafka-client-harness.sh`'s own `124` branches do not expect.
_reap_case() {
  local track="$1" expect="$2" out pid alive
  TOTAL=$((TOTAL + 1))
  out="$(REAP_SELFTEST_TRACK="$track" python3 "$REPO_ROOT/scripts/harness/reap.py" 2>/dev/null || true)"
  pid="$(head -1 <<<"$out")"
  sleep 1
  if [[ ! "$pid" =~ ^[0-9]+$ ]]; then
    fail "reap.py's self-test printed no child pid (track=$track): '$pid'"
    FAILED_CASES=$((FAILED_CASES + 1))
    return 0
  fi
  if kill -0 "$pid" 2>/dev/null; then alive=yes; else alive=no; fi
  kill -9 "$pid" 2>/dev/null || true
  if grep -q "HANDLER DID NOT FIRE" <<<"$out"; then
    fail "reap.py's SIGTERM handler did not raise (track=$track)"
    note "the child may still be reaped by atexit, so liveness alone cannot see this"
    note "each leg's own finally blocks stop running, and the exit status is 1, not 143"
    FAILED_CASES=$((FAILED_CASES + 1))
  elif [[ "$alive" == "$expect" ]]; then
    if [[ "$track" == 1 ]]; then
      ok "a tracked broker does not outlive its leg's SIGTERM"
    else
      ok "an untracked one does -- the leak M4.61 closes reproduces"
    fi
  else
    fail "reap.py self-test track=$track left the child alive=$alive, wanted $expect"
    note "track=1 failing means a ceiling would orphan the leg's own broker"
    note "track=0 failing means this pair proves nothing -- the leak must reproduce"
    FAILED_CASES=$((FAILED_CASES + 1))
  fi
}

_reap_case 1 no
_reap_case 0 yes

# ⚠️ **The ordering inside the handler, which the two arms above cannot
# see.** Their child is a `sleep` with nothing to unwind, so a reaper that
# fires from `atexit` after the unwind looks identical to one that kills
# before raising — and the kill-first ordering *is* round one's blocking
# finding. This arm puts a ten-second `finally` in the way and kills the leg two
# seconds in, the way `_run_bounded_stop` escalates, so only a handler that
# reaps before it raises can save the child. Found by review of `M4.61`,
# which is the third test in this row that could not fail.
TOTAL=$((TOTAL + 1))
_reap_slow_pidfile="$(new_scratch reap-slow)/pid"
REAP_SELFTEST_SLOW=1 python3 "$REPO_ROOT/scripts/harness/reap.py" \
  > "$_reap_slow_pidfile" 2>/dev/null &
_reap_slow_leg=$!
sleep 1
# ⚠️ `_run_bounded_stop`'s own escalation, spelled out rather than sourced,
# so this case keeps testing the ordering if that function is rewritten.
kill -TERM "$_reap_slow_leg" 2>/dev/null || true
sleep 2
kill -KILL "$_reap_slow_leg" 2>/dev/null || true
wait "$_reap_slow_leg" 2>/dev/null || true
_reap_slow_pid="$(head -1 "$_reap_slow_pidfile" 2>/dev/null)"
if [[ ! "$_reap_slow_pid" =~ ^[0-9]+$ ]]; then
  fail "reap.py's slow-unwind self-test printed no child pid: '$_reap_slow_pid'"
  FAILED_CASES=$((FAILED_CASES + 1))
elif kill -0 "$_reap_slow_pid" 2>/dev/null; then
  kill -9 "$_reap_slow_pid" 2>/dev/null || true
  fail "a slow unwind orphaned the tracked child -- the handler reaps too late"
  note "_kill_tracked must run before the raise: an atexit-only reaper is"
  note "overtaken by _run_bounded_stop's escalation to SIGKILL two seconds in"
  FAILED_CASES=$((FAILED_CASES + 1))
else
  ok "a tracked broker survives no longer than the handler, even with a slow unwind"
fi

# ── the FR-40 tripwire's pattern (M4.59) ────────────────────────────────────
#
# ⚠️ **`m4-complete.sh` cannot carry a `negative.sh` case**, for the reason
# `M4.50`'s row states: `has_rust` and its `cargo test --workspace` leg both
# `finish` before leg 2b runs, so a scratch tree never reaches it. The
# pattern that leg turns on lives in `lib.sh` since `M4.59` precisely so it
# can be driven here, with no workspace and no cargo — the same move
# `fencing_seam.py` made for the seam parser.
#
# ⚠️ **Both directions, because a tripwire fails in both.** A pattern that
# stops matching a check that is present leaves the leg green forever while
# `M12` lands `GroupGrants`; one that matches a handler with no check at all
# fails every commit until someone deletes the leg. `run_case` asserts a
# non-zero exit, so neither of these is a `run_case`: they are driven by
# hand, the shape the `unviable:` exemption and the wrapped-or-pattern case
# already use.
_fr40_pattern_dir() {
  local dir; dir="$(new_scratch fr40-pattern)"
  mkdir -p "$dir/scripts"
  cp "$REPO_ROOT/scripts/lib.sh" "$dir/scripts/lib.sh"
  printf '%s\n' "$dir"
}

_fr40_pattern_case() {
  local label="$1" body="$2" expect="$3" dir rc=0
  dir="$(_fr40_pattern_dir)"
  printf '%s\n' "$body" > "$dir/handler.rs"
  TOTAL=$((TOTAL + 1))
  bash -c '
    OQUEUE_SUPPRESS_TIMING=1 source "$1/scripts/lib.sh" 2>/dev/null
    group_names_a_principal "$1/handler.rs"
  ' _ "$dir" >/dev/null 2>&1 || rc=$?
  # ⚠️ **`1`, not "any non-zero", and that distinction is the whole case.**
  # `grep` answers 0 for a match and 1 for none; everything else is an error
  # — 2 for a bad pattern or an unreadable file, 127 when the helper is
  # absent or `lib.sh` fails to source. Accepting any non-zero reported `ok`
  # for a run that never happened: measured by renaming
  # `group_names_a_principal` in the copied `lib.sh`, where the negative case
  # agreed all was well while every positive one accused the pattern.
  # `lib.sh` is copied alone, so the day it sources a sibling that is exactly
  # what happens. Rule 20a's "not merely a non-zero exit", one layer down.
  # Found by review of `M4.59`.
  # ⚠️ `should` is derived rather than interpolated: `"should $expect"` reads
  # "should does not match" for the negative direction, which review left on
  # screen after the first repair and is worth one `case` arm to avoid.
  local want should
  case "$expect" in
    matches) want=0; should="must match" ;;
    *) want=1; should="must not match" ;;
  esac
  if [[ "$rc" == "$want" ]]; then
    ok "the FR-40 tripwire's pattern $expect: $label"
  elif [[ "$rc" != 0 && "$rc" != 1 ]]; then
    fail "the FR-40 tripwire's pattern could not be run against $label (rc=$rc)"
    note "rc 2 is a bad pattern or an unreadable file; 127 is a missing helper"
    FAILED_CASES=$((FAILED_CASES + 1))
  else
    fail "the FR-40 tripwire's pattern $should: $label (rc=$rc)"
    note "a tripwire wrong in this direction is what M12.md task 3a's promotion"
    note "signal depends on -- see group_names_a_principal's own note in lib.sh"
    FAILED_CASES=$((FAILED_CASES + 1))
  fi
}

# ⚠️ **One body per alternative, and each names *only* its own spelling.**
# A first version used `fn handle(principal: &Principal)` for the first
# case, which carries both spellings — so deleting either from the pattern
# failed nothing, and two of the five alternatives stayed exactly as
# unguarded as before this row. Review measured it and named what then slips
# through: `fn handle(req: &JoinGroupRequest, principal: &str)`, a check
# naming only the binding, which is the spelling `M12` is most likely to
# write.
_fr40_pattern_case "a lowercase principal binding" \
  'fn handle(req: &Req, principal: &str) -> i16 { 0 }' matches
_fr40_pattern_case "a capitalised Principal type" \
  'fn handle(p: &Principal) -> i16 { 0 }' matches
_fr40_pattern_case "an AuthzContext argument" \
  'fn handle(authz: AuthzContext) -> i16 { 0 }' matches
_fr40_pattern_case "an authz:: path" \
  'fn handle() -> i16 { authz::refuse() }' matches
_fr40_pattern_case "this repo's own _authorized( idiom" \
  'fn handle(g: &GroupId) -> i16 { group_authorized(g) }' matches
# ⚠️ **And a handler with no check at all must not match**, or the leg fails
# every commit until someone deletes it — the other way a tripwire dies.
_fr40_pattern_case "a handler that checks nothing" \
  'fn handle(request: &JoinGroupRequest) -> i16 { error_codes::NONE }' "does not match"

# ⚠️ **And an unreadable handler is neither answer**, which is the input
# `M4.68` is about: `grep` exits 2, and `m4-complete.sh`'s leg read the status
# as a boolean, so `chmod 000` on one handler printed `ok ... across 14
# file(s)` having scanned thirteen while `check-fencing-seam.sh`, walking the
# same `group_handler_files`, refused it. This pins the half that is testable
# here — that the helper reports it rather than answering "no check".
# ⚠️ **The leg's own branch has no case and cannot have one**: `m4-complete.sh`
# `finish`es at `has_rust` and again at `cargo test --workspace` before leg 2b
# runs, so a scratch tree never reaches it (`M4.50`'s row states this, and it
# is why the pattern moved into `lib.sh` at all). `testing.md` rule 20a's
# stated-reason escape, said here rather than implied by an absent case.
_fr40_unreadable_dir="$(_fr40_pattern_dir)"
printf 'fn handle(p: &Principal) -> i16 { 0 }\n' > "$_fr40_unreadable_dir/handler.rs"
chmod 000 "$_fr40_unreadable_dir/handler.rs"
_fr40_unreadable_rc=0
bash -c '
  OQUEUE_SUPPRESS_TIMING=1 source "$1/scripts/lib.sh" 2>/dev/null
  group_names_a_principal "$1/handler.rs"
' _ "$_fr40_unreadable_dir" >/dev/null 2>&1 || _fr40_unreadable_rc=$?
chmod 644 "$_fr40_unreadable_dir/handler.rs"
# ⚠️ Skipped when the suite runs as root, which can read a 000 file: the case
# would then report `matches` and accuse a helper that is behaving. A skip
# that says so beats a failure that does not.
# ⚠️ **`lib.sh`'s plain `skip`, never `skip_case`.** `skip_case` takes a
# *script name* and refuses anything outside `SKIPPABLE_GATES`, so calling it
# with a prose label fails the whole suite and prints that label in the
# `SKIPPED_CASES` line `m0-complete.sh` reads — the exact call shape this
# file's own `harness_check "an unknown gate name is refused"` exists to
# catch. Found by review; latent, since the host runs as uid 1000 and
# `docker-test.sh` passes `--user "$HOST_UID:$HOST_GID"`.
# ⚠️ `TOTAL` is incremented only where a case actually ran, so a skipped run
# does not inflate the count `m0-complete.sh` reads.
if [[ "$(id -u)" == 0 ]]; then
  skip "the FR-40 tripwire's pattern on an unreadable handler: running as root, which can read mode 000"
elif (( _fr40_unreadable_rc == 2 )); then
  TOTAL=$((TOTAL + 1))
  ok "the FR-40 tripwire's pattern reports an unreadable handler rather than calling it clean"
else
  TOTAL=$((TOTAL + 1))
  fail "the FR-40 tripwire's pattern answered $_fr40_unreadable_rc for an unreadable handler, not 2"
  note "1 would mean 'no principal check', which is what m4-complete.sh's leg used to conclude"
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **The other half of the leg that turns on a literal.** `M4.59`'s row
# names "the pattern and the strip", and the module layout under these
# handlers has already moved once beneath this leg (`M4.50`). When it moves
# again, a prefix that stops matching leaves the FAIL line naming offenders
# by full repo-relative path — legible, and not what the leg says it prints.
# ⚠️ **`|| _fr40_rc=$?`, because an unguarded `$( )` aborts the suite.**
# `lib.sh` sets `-e`, so a subshell that fails — a missing
# `group_handler_label`, a `lib.sh` that will not source — makes the
# assignment fail and the whole run stop there, silently: the remaining
# cases never execute and the `SKIPPED_COUNT` line the harness's own step in
# `gates.yml` greps is never printed. A case that cannot fail, in the row
# whose subject is exactly that. Found by review of `M4.59`, one fix behind
# the same correction to the pattern cases above.
TOTAL=$((TOTAL + 1))
_fr40_rc=0
_fr40_label="$(
  OQUEUE_SUPPRESS_TIMING=1 bash -c '
    source "$1/scripts/lib.sh" 2>/dev/null
    group_handler_label "crates/oqueue-broker/src/join_group/round/plan.rs"
  ' _ "$(_fr40_pattern_dir)" 2>/dev/null
)" || _fr40_rc=$?
if (( _fr40_rc != 0 )); then
  fail "the FR-40 tripwire's label helper could not be run (rc=$_fr40_rc)"
  note "a missing group_handler_label, or a lib.sh that will not source"
  FAILED_CASES=$((FAILED_CASES + 1))
elif [[ "$_fr40_label" == "join_group/round/plan.rs" ]]; then
  ok "the FR-40 tripwire names a handler by its path below the crate's source root"
else
  fail "the FR-40 tripwire's label is '$_fr40_label', not 'join_group/round/plan.rs'"
  note "the leg's FAIL line would name offenders by a path its own text does not describe"
  FAILED_CASES=$((FAILED_CASES + 1))
fi

# ⚠️ **The pin names the *code*, and `constructed outside` alone did not.**
# That substring is in the gate's own success line too — `none of the seam's
# N fencing codes are constructed outside ...` — so with the `VARIANT_RE`
# discriminant fix reverted the case reported `ok` while the defect it
# plants went unnoticed, which is the acceptance criterion it exists to
# meet. Found by review of `M4.56`; rule 20a's "pin the message of the
# defect it plants" is about telling one *outcome* from another, not one
# gate from another.
run_case "check-fencing-seam.sh (a variant with an explicit discriminant)" \
  setup_fencing_discriminant_variant invoke_fencing_discriminant_variant \
  "error_codes::FENCED_INSTANCE_ID constructed outside"
run_case "check-fencing-seam.sh (one arm answering two codes)" \
  setup_fencing_two_codes_in_one_arm invoke_fencing_two_codes_in_one_arm \
  "answers more than one error_codes:: path"
# ⚠️ **This one must *pass*** — `run_case` asserts a non-zero exit, which is
# the whole of what it checks, so a seam that is merely formatted differently
# is driven by hand. `M4.56`.
_fencing_wrapped_dir="$(setup_fencing_wrapped_or_pattern)"
TOTAL=$((TOTAL + 1))
if bash "$_fencing_wrapped_dir/scripts/check-fencing-seam.sh" >/dev/null 2>&1; then
  ok "check-fencing-seam.sh (a rustfmt-wrapped or-pattern) passes on a legitimate artifact"
else
  fail "check-fencing-seam.sh (a rustfmt-wrapped or-pattern) refused a legitimate artifact"
  note "$(bash "$_fencing_wrapped_dir/scripts/check-fencing-seam.sh" 2>&1 | sed 's/^/     /')"
  FAILED_CASES=$((FAILED_CASES + 1))
fi


run_case "check-budget.sh (suite over budget)" setup_budget_over invoke_budget_over
run_case "check-budget.sh (erosion behind a compiling gate)" setup_budget_eroded_behind_a_build invoke_budget_eroded_behind_a_build \
  "over the 10000 ms budget"
# ⚠️ **What this case pins changed under it, and that is worth knowing.**
# `M4.63` deleted the propagate-a-skip branch entirely, having measured it
# unreachable for both cases it named — so this fixture now reaches the
# survivor loop because there is no branch in front of it, not because the
# pattern in that branch is anchored. It keeps its value as a regression
# guard: reintroduce any branch that swallows a `MISSED` line on the
# strength of a phrase in the output, and this case fails. It no longer
# distinguishes an anchored pattern from a loose one, because there is no
# pattern.
#
# ⚠️ **A tool failure whose output contains the skip phrase.** The propagate
# -a-skip branch matched `^(skip|.*skip) ...` until `M4.62`, and `.*skip`
# reaches a line's interior — so a rustc error reading `error: could not skip
# mutation testing (no Rust staged)` turned a failed run into a reported
# skip, which `m0-complete.sh` and CI both read as "nothing to do". The
# stub stands in for `mutants.sh` because reproducing that exact rustc
# diagnostic from a real crate is incidental to the property.
setup_mutants_skip_phrase_in_an_error() {
  local dir; dir="$(new_scratch mutants-skip-phrase)"
  copy_gate "$dir" check-mutants.sh
  mkdir -p "$dir/baselines" "$dir/scripts"
  printf '# fixture: nothing argued\n' > "$dir/baselines/mutants.txt"
  printf '[workspace]\nmembers = []\nresolver = "2"\n' > "$dir/Cargo.toml"
  # ⚠️ **The shape is a *real* unargued survivor, reported with a non-zero
  # exit, whose output also carries the phrase.** A stub that merely fails
  # is caught by the tool-failure branch (non-zero with no `MISSED`) three
  # checks earlier, and one that merely succeeds is indistinguishable from a
  # pass — the first version of this fixture stopped at that branch and
  # passed whatever the gate did, measured.
  #
  # ⚠️ **Why that shape, in the past tense**: it was built to isolate the
  # propagate-a-skip branch, where under the pattern `M4.62` replaced the
  # skip won and the survivor was never compared against the baseline.
  # `M4.63` then deleted that branch as unreachable, so the shape is now
  # what makes the fixture reach the survivor loop with nothing in front of
  # it. See the note above the `run_case` for what this still pins.
  cat > "$dir/scripts/mutants.sh" <<'STUB'
#!/usr/bin/env bash
echo "error: could not skip mutation testing (no Rust staged) -- rustc said so"
echo "1 mutants tested: 0 caught"
echo "MISSED crates/k/src/lib.rs:2:5: replace classify with \"\""
exit 2
STUB
  chmod +x "$dir/scripts/mutants.sh"
  printf '%s\n' "$dir"
}
invoke_mutants_skip_phrase_in_an_error() {
  bash "$1/scripts/check-mutants.sh" --full
}

_have_mutants() { cargo mutants --version >/dev/null 2>&1; }
if _have_mutants; then
  # ⚠️ **Both pin the survivor message, and `M4.32` is why.** These two were
  # the suite's oldest `check-mutants` cases and carried no expected string,
  # so `run_case` asked only for a non-zero exit — and from `M4.19` until
  # `M4.32` both had been passing on `profile 'mutants' is not defined`,
  # counted green while proving nothing about a surviving mutant. `M4.32`
  # repaired the fixture and left the cases unpinned; `M4.52` is this.
  #
  # ⚠️ **The justification is this fixture's measured output, not the gate's
  # branch count** — rule 20a says the second in terms ("a gate with nine
  # `fail` branches whose fixture trips exactly one of them needs no pin,
  # which is why most cases carry none"), and this suite says the same just
  # below its `check-readmes.sh` cases — "one property per *fixture output*,
  # not per gate". What
  # earns the pin here is that these fixtures' output demonstrably carries a
  # *second* kind: delete `[profile.mutants]` from `_crate_scratch`'s
  # workspace manifest and both print `cargo mutants exited 4 with no
  # surviving mutants reported` instead — measured, and the regression
  # `M4.32` found. The pinned string comes from the unargued-survivor branch;
  # the other is the tool-failure branch. ⚠️ **Branch names, not line
  # numbers** — six of those were written here first and `M4.55` is already
  # the row for sixteen decayed references of exactly that kind.
  #
  # ⚠️ **Both pin the same string, so the narrowed case does not assert that
  # it narrowed** — argued rather than fixed, because `run_case` takes one
  # expectation and the alternative (`running cargo mutants (narrowed)`) is
  # printed before the run and survives a build failure, so it would trade a
  # strong property for a weak one. Carrying both needs `run_case` to accept
  # a list, which is a harness change and not this row's. Recorded in
  # `M4.52`.
  run_case "check-mutants.sh (test constrains nothing)" setup_mutants_weakened invoke_mutants_weakened \
    "surviving mutant, not killed and not argued"
  run_case "check-mutants.sh (narrowed, test constrains nothing)" setup_mutants_narrowed invoke_mutants_narrowed \
    "surviving mutant, not killed and not argued"
  run_case "check-mutants.sh (a baseline entry that argues nothing)" \
    setup_mutants_stale_baseline invoke_mutants_stale_baseline \
    "argues no surviving mutant"
  run_case "check-mutants.sh (an 'unviable:' claim the run disproves)" \
    setup_mutants_false_unviable invoke_mutants_false_unviable \
    "claims 'unviable:' for a mutant that survived"
  run_case "check-mutants.sh (an entry with no reason)" \
    setup_mutants_no_reason invoke_mutants_no_reason \
    "baseline entry has no reason"
  run_case "check-mutants.sh (an entry whose location is not a location)" \
    setup_mutants_prefix_entry invoke_mutants_prefix_entry \
    "does not start with a <file>.rs:<line>:<col>: location"
  # ⚠️ **Inside `_have_mutants`, and it has to be.** The stub stands in for
  # `mutants.sh`, but `check-mutants.sh` runs `cargo mutants --version`
  # itself before invoking it and skips when that fails — so on a host
  # without the tool this case's gate exits 0 and `run_case` would report
  # `reported ok on a broken artifact`, a red suite accusing a working gate.
  # A first version of this comment said the opposite, and acting on it is
  # what would have produced that. Found by review of `M4.62`.
  run_case "check-mutants.sh (the skip phrase inside a tool error)" \
    setup_mutants_skip_phrase_in_an_error invoke_mutants_skip_phrase_in_an_error \
    "surviving mutant, not killed and not argued"
  # ⚠️ **Inside `_have_mutants`, and the comment above these two used to say
  # the opposite.** Their stub stands in for `mutants.sh`, but
  # `check-mutants.sh` runs `require_tool cargo` and `cargo mutants
  # --version` itself long before invoking it — so on a host without the
  # tool the gate exits 0, the must-pass block takes its else arm and the
  # `run_case` below reports a working gate green on a broken artifact.
  # Two red cases accusing a working gate, which is what this guard exists
  # to prevent. `M4.62`'s review found the identical defect forty lines
  # below; `M4.60` reintroduced it one round later. ⚠️ The two `mutants.sh`
  # cases stay outside: its refusals precede `has_rust`, so they genuinely
  # need no cargo.
  # ⚠️ **Must *pass*, so `run_case` cannot express it.** The property is that a
  # shard stays quiet about the baseline: delete the `if [[ -n "$SHARD" ]]`
  # block in `check-mutants.sh` and this fixture reports the entry stale,
  # which is the nightly failing a correct baseline on seven runs in eight.
  # ⚠️ **`|| _shard_rc=$?`, because an unguarded `$( )` aborts the suite.**
  # `lib.sh` sets `-e` with `inherit_errexit`, so a failing substitution ends
  # the run at this line rather than failing this case — and this case exists
  # for the state in which that gate *does* fail. A first version omitted it
  # and the probe that should have shown the case working showed the suite
  # stopping instead. Second time in this task; the first was `M4.61`'s label
  # case.
  TOTAL=$((TOTAL + 1))
  _shard_dir="$(_shard_fixture)"
  _shard_rc=0
  _shard_out="$( ( cd "$_shard_dir" && bash scripts/check-mutants.sh --full --shard 1/8 ) 2>&1 )" \
    || _shard_rc=$?
  if (( _shard_rc == 0 )) \
    && ! grep -q "argues no surviving mutant" <<<"$_shard_out" \
    && [[ -f "$_shard_dir/target/mutants-survivors/1-of-8.txt" ]]; then
    ok "a shard stays silent about the baseline and records its survivors for the union"
  else
    fail "a shard judged the baseline, or recorded no survivors (rc=$_shard_rc)"
    note "the converse loop must not run per-shard: it sees an eighth of the mutants,"
    note "so every entry arguing a survivor elsewhere reads as dead"
    note "$(sed 's/^/     /' <<<"$_shard_out")"
    FAILED_CASES=$((FAILED_CASES + 1))
  fi

  # ⚠️ **And the same fixture *without* `--shard` must report it**, or the case
  # above passes on a gate that never judges the baseline at all.
  setup_shard_full_still_judges() {
    _shard_fixture
  }
  invoke_shard_full_still_judges() {
    ( cd "$1" && bash scripts/check-mutants.sh --full )
  }
  run_case "check-mutants.sh (unsharded, an entry that argues nothing)" \
    setup_shard_full_still_judges invoke_shard_full_still_judges \
    "argues no surviving mutant"

  # ⚠️ **The one case that must *pass*, so `run_case` cannot express it** —
  # that helper asserts a non-zero exit, which is the whole of what it checks.
  # A gate that refuses a legitimate `unviable:` entry is as broken as one that
  # accepts a stale suppression, and only the first kind has a helper.
  TOTAL=$((TOTAL + 1))
  _unviable_dir="$(setup_mutants_unviable_exempt)"
  # ⚠️ Output captured, not discarded: a must-pass case that only says "it
  # refused" sends the reader back to rebuild the fixture by hand to find out
  # why, which is what the first version of this case cost.
  _unviable_out="$(invoke_mutants_unviable_exempt "$_unviable_dir" 2>&1)" && _unviable_rc=0 || _unviable_rc=$?
  if (( _unviable_rc == 0 )); then
    ok "check-mutants.sh (an 'unviable:' entry is exempt) passes on a legitimate artifact"
  else
    fail "check-mutants.sh (an 'unviable:' entry is exempt) refused a legitimate artifact"
    note "an entry that says why its mutant is no longer generated must not be stale"
    while IFS= read -r _line; do note "$_line"; done <<<"$(tail -8 <<<"$_unviable_out")"
    FAILED_CASES=$((FAILED_CASES + 1))
  fi
else
  skip_case check-mutants.sh "cargo-mutants not installed" \
    "install: cargo install cargo-mutants" 10
fi

# --- the harness checks itself ----------------------------------------------
#
# ⚠️ **`skip_case` is this suite's own code, so `run_case` cannot reach it** —
# `run_case` invokes a gate script against a scratch directory, and `skip_case`
# is a shell function in this file. `M1.25` asked for a case proving a
# malformed third argument is rejected and a legitimate skip is counted;
# without a harness of its own that promise has nowhere to live, and the two
# defects `M1.25` fixed were both invisible precisely because nothing ever
# exercised the function.
#
# Each check runs inside a command substitution, which is a subshell: a
# deliberate `fail` increments `lib.sh`'s `_FAILURES` in a copy of this shell
# and never in this one, and the `SKIPPED_*` state each check sets up is
# discarded with it. The suite must not go red for proving its own guard works.
HARNESS_FAILURES=0

harness_check() {
  local label="$1" expect="$2" snippet="$3"
  local out ec=0
  # ⚠️ The brace group, then `2>&1` on the group. Written as a bare `2>&1`
  # after the last command it binds to *that command* and `fail`'s message —
  # which goes to stderr — escapes the capture, so a check looking for it
  # fails while the guard it tests works fine. Measured writing this.
  out="$( {
    SKIPPED_GATES=()
    SKIPPED_CASE_COUNT=0
    FAILED_CASES=0
    # ⚠️ `|| ec=$?` immediately, and printed. Capturing the substitution's own
    # status instead reads the status of the **last** command in the group —
    # the `printf` — which is always 0, so `skip_case` returning non-zero was
    # invisible. Measured: the first version of this guard asserted on that
    # and stayed green through exactly the regression it was written for.
    ec=0
    eval "$snippet" || ec=$?
    printf 'RECORDED [%s] %s EXIT %s\n' "${SKIPPED_GATES[*]-}" "$SKIPPED_CASE_COUNT" "$ec"
  } 2>&1 )" || true
  # ⚠️ `|| true`. Dropping the dead `rc` capture also dropped the only thing
  # stopping a snippet that aborts its own subshell from aborting this whole
  # suite: `set -e` fires on the failed substitution, the run stops mid-section
  # and prints no summary. Measured — removing `10#` made the zero-padded check
  # kill the suite instead of failing it. A check must fail, not abort; the
  # `EXIT n` field is what carries the status now, and an aborted subshell
  # prints none, so the grep below fails cleanly.
  # ⚠️ **The status is read out of the captured text, not from the
  # substitution.** `out="$( … )" || rc=$?` reads the status of the *last*
  # command in the group — the `printf` — which is always 0, so a `skip_case`
  # that started returning non-zero left every check green. That is the single
  # regression the function is most heavily commented against (see the
  # `return 0, deliberately` note above: a non-zero return aborts the whole
  # suite before `SKIPPED_COUNT`/`SKIPPED_CASES` print, and `m0-complete.sh`
  # then misreports a stale suite).
  #
  # ⚠️ **Two comments here got the bash wrong before this one**, in opposite
  # directions, so the measurements are recorded rather than the conclusion.
  # `x=$(false)` **does** abort under `set -e` — the assignment takes the
  # substitution's status. `x=$(false; echo reached)` **does not** — the
  # group's status is the last command's, and errexit is not inherited by the
  # commands inside a substitution unless `inherit_errexit` is set. Those are
  # different claims and each earlier comment asserted one while meaning the
  # other. ⚠️ **This file sets `inherit_errexit`** (line 60), so inside these
  # substitutions the second case aborts too.
  #
  # Neither is why this guard reads the text. It reads the text because the
  # group has two possible endings and the observable status is useless in
  # both: when the snippet runs to completion the last command is the
  # `printf`, so the status is 0 no matter what `skip_case` returned; and when
  # the snippet aborts its own subshell — an arithmetic expansion error under
  # `inherit_errexit`, say — the `printf` never runs, the substitution fails,
  # and there is no status to read either. Measured under this file's own
  # settings: `n=$(( 0 + 08 ))` inside the group yields substitution status 1
  # and no `EXIT` line at all. That second ending is what `|| true` above
  # contains, and the missing `EXIT n` line is what makes the check fail
  # rather than the suite abort. ⚠️ A third version of this comment claimed
  # the status "is always 0 whatever errexit does", which contradicted the
  # `|| true` twenty lines above it — a maintainer believing it would remove
  # that guard and restore the abort.
  #
  # ⚠️ `shopt -s inherit_errexit` is set at the **top of this file**, not in
  # `lib.sh` — an earlier version of this comment sent a reader to the wrong
  # file, where finding nothing invites deleting the real line. It is what
  # makes a failing `setup_*` inside `dir="$(setup_fn)"` abort instead of
  # yielding an empty path, and all 59 `run_case` invocations depend on it.
  # (55 of those lines are unconditional; the other four sit inside
  # `if _have_*` branches, which is where a "55" in an earlier draft came
  # from.)
  if ! grep -q 'EXIT 0$' <<< "$out"; then
    fail "harness: $label -- skip_case did not return 0"
    note "a non-zero return aborts the suite before SKIPPED_CASES is printed"
    sed 's/^/     /' <<< "$out" >&2
    HARNESS_FAILURES=$((HARNESS_FAILURES + 1))
  elif grep -qF -- "$expect" <<< "$out"; then
    ok "harness: $label"
  else
    fail "harness: $label -- expected output containing: $expect"
    sed 's/^/     /' <<< "$out" >&2
    # ⚠️ Its own counter, not `FAILED_CASES`. That one feeds "N gate(s)
    # exercised, M failed to fail as expected", and a harness defect is not a
    # gate that failed to fail — reporting it there sends the reader hunting a
    # broken fixture among 59 cases that all passed.
    HARNESS_FAILURES=$((HARNESS_FAILURES + 1))
  fi
}

# --- the shared manifest reader agrees with itself --------------------------
#
# ⚠️ `M1.32` consolidated three hand-rolled `Cargo.toml` parsers into
# `scripts/lib/manifest.py`. Like the routing checks below, this cannot be a
# `run_case`: the module returns values rather than failing, and this suite's
# pass condition is a non-zero exit. `testing.md` rule 20a's "a stated reason
# instead". The fixture is the manifest that broke all three copies at once.
manifest_check() {
  local label="$1" toml="$2" want="$3" expr="$4"
  local dir out
  dir="$(mktemp -d -p "$SCRATCH_ROOT" manifest-XXXX)"
  printf '%s\n' "$toml" > "$dir/Cargo.toml"
  out="$(python3 -c "
import sys, pathlib
sys.path.insert(0, 'scripts/lib')
import manifest
p = pathlib.Path('$dir/Cargo.toml')
print($expr)
" 2>&1)" || out="ERROR: $out"
  if [[ "$out" == "$want" ]]; then
    ok "manifest: $label"
  else
    fail "manifest: $label -- wanted '$want', got '$out'"
    HARNESS_FAILURES=$((HARNESS_FAILURES + 1))
  fi
}

# ⚠️ **One spelling per check.** A single manifest carrying every case cannot
# isolate them: whichever header ends the dependency table first hides the ones
# after it, so mutations to the later branches survive. Measured -- an earlier
# mega-fixture left three of the module's four fail-safe branches unpinned.

# `M1.23`'s review: single-quoted values were invisible to the old pattern.
manifest_check "a single-quoted package name is read" \
  "[package]
name = 'oqueue-broker'" \
  "oqueue-broker" "manifest.package_name(p)"

# ⚠️ The one that mattered: `[[bench]]` directly after the flow keys. With a
# `[dependencies.serde]` table between them the old parser was already in table
# state, where body lines are ignored, so an earlier fixture proved nothing.
manifest_check "an array-of-tables ends the dependency table" \
  '[dependencies]
tokio = "1"
"oqueue-core" = { path = "../oqueue-core" }

[[bench]]
name = "bench_micro"
harness = false' \
  "['oqueue-core', 'tokio']" "sorted(manifest.runtime_deps(p))"

# A header carrying a trailing comment is still a header. An anchored pattern
# does not recognise it, and then every key under it is a runtime dependency --
# or, on `[dependencies]` itself, none of them are.
manifest_check "a trailing comment does not hide a header" \
  '[dependencies]
tokio = "1"

[dev-dependencies]  # test only
proptest = "1"' \
  "['tokio']" "sorted(manifest.runtime_deps(p))"
# ⚠️ And on `[dependencies]` itself, which is the direction the fail-safe
# fallback cannot cover for: an anchored pattern makes this an unnameable
# header, the table never opens, and *every* runtime dependency vanishes --
# a silent pass on NFR-52 rather than a false red. Measured: with the close
# bracket anchored, the check above still passed and only this one fails.
manifest_check "a trailing comment on [dependencies] hides nothing" \
  '[dependencies]  # runtime only
tokio = "1"' \
  "['tokio']" "sorted(manifest.runtime_deps(p))"

# ⚠️ Fail *safe*: a line this reader cannot parse as a header still ends the
# table before it. Returning None there instead leaves the section sticky and
# harvests `stray` -- the silent-pass shape `sections()`'s docstring records.
manifest_check "an unparseable header still ends the table" \
  '[dependencies]
tokio = "1"

[this opens a table and never closes it
stray = "value"' \
  "['tokio']" "sorted(manifest.runtime_deps(p))"

# ⚠️ The named sub-table form, `[dependencies.<name>]` — the standard spelling
# once a dependency has several keys. Unpinned until review measured it: with
# the named-sub-table branch dropped, `oqueue-core` goes invisible to
# `check-layering.sh` (a silent pass on NFR-52) while `path` and `version` are
# recorded as dependency names for `check-readmes.sh` to report as
# undocumented. The `[target...dev-dependencies]` line pins the other half:
# relaxing the target test to `segments[0] == "target"` alone makes
# `segments.index("dependencies")` raise and takes the gate down.
manifest_check "a named dependency sub-table is one dependency, not its keys" \
  '[dependencies.oqueue-core]
path = "../oqueue-core"
version = "0.1"

[target."cfg(unix)".dev-dependencies]
tempfile = "3"' \
  "['oqueue-core']" "sorted(manifest.runtime_deps(p))"

# ⚠️ `foo.workspace = true` — the dotted-key form, which this workspace uses
# for nearly every dependency. Unpinned until review measured it: dropping
# `_KEY`'s dotted-suffix alternative made every one of them invisible, with the
# whole suite and `check-layering.sh` still green.
manifest_check "a dotted key is one dependency" \
  '[dependencies]
object_store.workspace = true
tokio = { workspace = true }' \
  "['object_store', 'tokio']" "sorted(manifest.runtime_deps(p))"

# ⚠️ A single-quoted dependency key, and a cfg string containing an escaped
# quote — both valid Cargo, both unpinned until review's fifth mutation pass:
# dropping `_KEY`'s single-quote alternative loses the dependency entirely, and
# turning `_SEGMENT`'s skip-a-character fallback into a `break` loses every
# dependency under a cfg whose text embeds a quote.
manifest_check "a single-quoted key and an escaped quote in a cfg are read" \
  "[dependencies]
'oqueue-core' = { path = '../oqueue-core' }

[target.\"cfg(target_os = \\\"macos\\\")\".dependencies]
libc = \"0.2\"" \
  "['libc', 'oqueue-core']" "sorted(manifest.runtime_deps(p))"

# ⚠️ A target-scoped *named* sub-table. Off-by-one in the segment index records
# a dependency literally called `dependencies`.
manifest_check "a target-scoped named sub-table names the dependency" \
  '[target."cfg(unix)".dependencies.libc]
version = "0.2"' \
  "['libc']" "sorted(manifest.runtime_deps(p))"

# ⚠️ `name` outside `[package]` is not the package name. Without the section
# guard the first `name =` anywhere wins — here a benchmark's.
manifest_check "only [package] supplies the package name" \
  '[[bench]]
name = "bench_micro"

[package]
name = "oqueue-core"' \
  "oqueue-core" "manifest.package_name(p)"

# dev- and build-dependencies stay out; quoted keys and target-scoped tables in.
manifest_check "target-scoped deps count, dev-dependencies do not" \
  '[dependencies]
tokio = "1"

[dev-dependencies]
proptest = "1"

[target."cfg(unix)".dependencies]
libc = "0.2"' \
  "['libc', 'tokio']" "sorted(manifest.runtime_deps(p))"

# --- which-standards.sh routes a diff to the right standards -----------------
#
# ⚠️ **`run_case` cannot express this**: `which-standards.sh` is not a pass/fail
# gate, it prints a list, and this suite's pass condition is a *non-zero exit*.
# So `M1.31` pins its routing the same way `M1.25` pinned `skip_case` — by
# running it and asserting on the output. `testing.md` rule 20a's "a stated
# reason instead" is why this is here rather than as a case.
# ⚠️ **stdout and stderr kept apart, and the exit status checked.** Merging
# them was a real hole, not a tidiness point: `which-standards.sh` reports a
# missing or malformed `applies_to` as `PROBLEM docs/internal/standards/…` on
# *stderr* and exits 1 — and that line contains the path the check greps for,
# so with `2>&1` a deleted `applies_to` made both positive checks print `ok`
# while `security.md` routed to nothing at all. Measured. `review.sh` is the
# only other consumer of this script and it separates the streams too.
_routing_run() {
  local file="$1"
  _ROUTING_ERR="$(mktemp)"
  _ROUTING_RC=0
  _ROUTING_OUT="$(bash "$REPO_ROOT/scripts/which-standards.sh" "$file" \
    2>"$_ROUTING_ERR")" || _ROUTING_RC=$?
}
_routing_fail() {
  fail "routing: $1"
  sed 's/^/     out: /' <<< "$_ROUTING_OUT" >&2
  sed 's/^/     err: /' < "$_ROUTING_ERR" >&2
  rm -f "$_ROUTING_ERR"
  HARNESS_FAILURES=$((HARNESS_FAILURES + 1))
}
routing_check() {
  local label="$1" file="$2" want="$3"
  _routing_run "$file"
  if (( _ROUTING_RC != 0 )); then
    _routing_fail "$label -- which-standards.sh exited $_ROUTING_RC"
  elif grep -qF -- "$want" <<< "$_ROUTING_OUT"; then
    rm -f "$_ROUTING_ERR"
    ok "routing: $label"
  else
    _routing_fail "$label -- $file should select $want"
  fi
}

# ⚠️ `oqueue-core` holds every secret-shaped type in the workspace —
# `Redacted`, `WrappedKey`, `KeyId`, `Error::SecretRejected`, `FakeKeyProvider`
# — and was absent from `security.md`'s `applies_to` until `M1.31`, so a diff
# to the file defining them handed the reviewer no security standard at all.
routing_check "a diff to oqueue-core's key material selects security.md" \
  "crates/oqueue-core/src/key.rs" "standards/security.md"
# ⚠️ And `bin/oqueue` is where the `KeyProvider` implementation is chosen.
routing_check "a diff to the composition root selects security.md" \
  "bin/oqueue/src/main.rs" "standards/security.md"
# ⚠️ The negative direction, so the patterns are not simply matching everything:
# a pure documentation diff selects no security standard.
routing_check_absent() {
  local label="$1" file="$2" unwanted="$3"
  _routing_run "$file"
  # ⚠️ A crash is a failure here too. Without the rc check an absent-check
  # passes whenever the script dies before printing anything, which is the
  # easiest way for it to "not select" something.
  if (( _ROUTING_RC != 0 )); then
    _routing_fail "$label -- which-standards.sh exited $_ROUTING_RC"
  elif grep -qF -- "$unwanted" <<< "$_ROUTING_OUT"; then
    _routing_fail "$label -- $file should not select $unwanted"
  else
    rm -f "$_ROUTING_ERR"
    ok "routing: $label"
  fi
}
routing_check_absent "a docs-only diff selects no security standard" \
  "docs/internal/product/roadmap.md" "standards/security.md"

# A bare number where the remedy goes is refused...
harness_check "a numeric third argument is refused" \
  "third argument is a remedy, got the bare number '3'" \
  'skip_case check-mutants.sh "reason" 3'
# ...and the count it plainly meant is recorded, not silently defaulted to 1.
harness_check "the count a numeric third argument meant is still recorded" \
  "RECORDED [check-mutants.sh] 3" \
  'skip_case check-mutants.sh "reason" 3'

# An unknown gate name is refused *and* still appears, so `SKIPPED_CASES`
# cannot read as "nothing was skipped" for a case that did not run.
harness_check "an unknown gate name is refused" \
  "not a script name this suite knows" \
  'skip_case mutants "a typo for check-mutants.sh"'
# ⚠️ Two checks, not one. With only the `RECORDED` half, deleting the
# gate-name guard outright left this green — the call fell through to the
# ordinary path, which emits the identical line. Measured; `testing.md`
# rule 20a's "passes on an unrelated failure while the property it exists for
# is deleted" exactly.
harness_check "an unknown gate name is still recorded" \
  "RECORDED [mutants] 1" \
  'skip_case mutants "a typo for check-mutants.sh"'

# A non-numeric fourth argument is refused and the gate still recorded.
# ⚠️ Two checks again, for the reason the gate-name pair records: with only the
# `RECORDED` half, replacing this guard's `fail` with a silent `cases=1`
# leaves every check green and restores on the fourth slot exactly the
# undercount `M1.25` closed on the third. Measured.
harness_check "a non-numeric fourth argument is refused" \
  "fourth argument is a case count" \
  'skip_case check-mutants.sh "reason" "install it" "two"'
harness_check "a non-numeric fourth argument is still recorded" \
  "RECORDED [check-mutants.sh] 1" \
  'skip_case check-mutants.sh "reason" "install it" "two"'

# ⚠️ Pins the `10#` in `_skip_case_record`. Without it `08` reaches `$(( ))`
# as an invalid octal literal, `set -e` aborts the subshell, and no `RECORDED`
# line is printed at all — so this check fails, where nothing pinned it before.
harness_check "a zero-padded count is arithmetic, not octal" \
  "RECORDED [check-mutants.sh] 8" \
  'skip_case check-mutants.sh "reason" "install it" 08'

# The ordinary path still records exactly what it always did.
harness_check "a legitimate skip is counted" \
  "RECORDED [check-mutants.sh] 2" \
  'skip_case check-mutants.sh "reason" "install it" 2'

if (( HARNESS_FAILURES > 0 )); then
  note "$HARNESS_FAILURES self-check(s) failed — this suite's own helper, or the"
  note "routing it asserts, is broken. Read the FAIL lines above: 'harness:' is"
  note "skip_case, 'routing:' is which-standards.sh and a standard's applies_to."
fi
note "$TOTAL gate(s) exercised, $FAILED_CASES failed to fail as expected"

# ⚠️ **Machine-readable, on its own line, and printed even when none skipped.**
# A caller that greps for this must be able to tell "nothing skipped" from
# "this suite is too old to say", and an absent line cannot carry that
# difference. `m0-complete.sh` reads it. `M0.26`.
# ⚠️ Count **and** names. `M0.26`'s acceptance asked how many cases skipped and
# what shipped was which gates — the more useful thing for `m0-complete.sh`,
# and not the thing the row said. Both are here now, and they differ: without
# `cargo-mutants` two `run_case` lines do not run while one `skip_case` fires,
# so a reader deriving a case count from the names gets 1 where the answer is
# 2. `SKIPPED_COUNT` is the cases; `SKIPPED_CASES` is the gates. `M0.28`.
printf 'SKIPPED_COUNT %s\n' "$SKIPPED_CASE_COUNT"
printf 'SKIPPED_CASES %s\n' "${SKIPPED_GATES[*]:-}"

finish
