#!/usr/bin/env bash
# The review packet says which round it is, and shows a round its delta.
#
#   tests/harness/review-packet.sh
#
# ## Why this is its own suite
#
# ⚠️ **`tests/gates/negative.sh` inverts its pass condition**: a case there
# passes when the gate under test *fails* on a broken artifact. `review.sh
# context` is not a gate and has no failing verdict to provoke — what can go
# wrong with it is that it shows the reviewer **less** than it should, which is
# a silent success rather than a failure. So the cases here assert what the
# packet contains, and the one that matters most asserts a fallback: when the
# delta cannot be computed, the whole diff must appear anyway.
#
# Every case builds its own disposable repository under `mktemp -d` and copies
# `review.sh`, `lib.sh` and `scripts/lib/review_rounds.py` into it, the same
# containment `negative.sh`'s header explains: `lib.sh` resolves `REPO_ROOT`
# from its own location, so a copy is what keeps a case out of the real tree.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
PASS=0
FAIL=0
SKIPPED=0

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }

scratch() {
  local dir; dir="$(mktemp -d)"
  mkdir -p "$dir/scripts/lib" "$dir/docs/internal/product" "$dir/target/review"
  cp "$ROOT/scripts/review.sh" "$ROOT/scripts/lib.sh" "$dir/scripts/"
  cp "$ROOT/scripts/review-lenses.sh" "$dir/scripts/"
  cp "$ROOT/scripts/lib/review_rounds.py" "$dir/scripts/lib/"
  # ⚠️ `which-standards.sh` and the gate list are consulted by the packet. They
  # are copied when present and the packet tolerates their absence; a case here
  # asserts the round and diff sections, never the gate list.
  [ -f "$ROOT/scripts/which-standards.sh" ] && cp "$ROOT/scripts/which-standards.sh" "$dir/scripts/"
  cat > "$dir/docs/internal/product/backlog.md" <<'EOF'
| M-1.1 | a real task | some criterion | todo |
EOF
  # ⚠️ Ignored, as it is in the real tree: a verdict staged into the diff it is
  # a verdict about would change the hash it is bound to, and the fixture would
  # be testing something no commit can reach.
  printf 'target/\n' > "$dir/.gitignore" 
  (
    cd "$dir"
    git init -q .
    git config user.email t@example.com
    git config user.name t
    git add -A
    git commit -qm "M-1.1: a base commit"
  )
  printf '%s\n' "$dir"
}

# Stages `content` as `file.txt` and prints the packet for `M-1.1`.
packet() {
  local dir="$1" content="$2"
  printf '%s\n' "$content" > "$dir/file.txt"
  (cd "$dir" && git add -A && bash scripts/review.sh context --task M-1.1 2>/dev/null)
}

# Records a fixture verdict for whatever is staged now, with the tree beside it.
record_round() {
  local dir="$1" severity="$2"
  (
    cd "$dir"
    h="$(git diff --cached | sha256sum | cut -d' ' -f1)"
    git write-tree > "target/review/$h.tree"
    cat > "target/review/$h.json" <<EOF
{"task_id": "M-1.1", "diff_sha256": "$h", "reviewer": "fixture",
 "verdict": "changes-requested",
 "findings": [{"severity": "$severity", "file": "file.txt", "line": 1,
               "summary": "a fixture finding",
               "failure_scenario": "a fixture scenario"}]}
EOF
  )
}

check() {
  local name="$1" haystack="$2" needle="$3"
  if printf '%s' "$haystack" | grep -qF -- "$needle"; then
    green "ok   $name"
    PASS=$((PASS + 1))
  else
    red "FAIL $name"
    red "     expected to find: $needle"
    FAIL=$((FAIL + 1))
  fi
}

refute() {
  local name="$1" haystack="$2" needle="$3"
  if printf '%s' "$haystack" | grep -qF -- "$needle"; then
    red "FAIL $name"
    red "     did not expect: $needle"
    FAIL=$((FAIL + 1))
  else
    green "ok   $name"
    PASS=$((PASS + 1))
  fi
}

if ! command -v python3 >/dev/null 2>&1 || ! command -v sha256sum >/dev/null 2>&1; then
  echo "SKIPPED python3 or sha256sum missing"
  SKIPPED=$((SKIPPED + 1))
  echo "SKIPPED_COUNT $SKIPPED"
  exit 0
fi

# --- round one: no prior verdict, so the whole diff and no delta -----------
dir="$(scratch)"
out="$(packet "$dir" "first version")"
check "round one names itself" "$out" "This is round 1 of 3"
check "round one shows the staged diff" "$out" "## The staged diff"
refute "round one claims no delta" "$out" "## What changed since the last round"
refute "round one lists no open findings" "$out" "## What earlier rounds left open"
rm -rf "$dir"

# --- round two: a prior verdict with its tree, so a delta ------------------
dir="$(scratch)"
packet "$dir" "first version" > /dev/null
record_round "$dir" blocking
out="$(packet "$dir" "second version")"
check "round two names itself" "$out" "This is round 2 of 3"
check "round two shows the delta" "$out" "## What changed since the last round"
check "round two carries the open finding" "$out" "a fixture finding"
check "round two carries its scenario" "$out" "a fixture scenario"
check "the delta holds the new line" "$out" "+second version"
refute "the delta drops what round one cleared" "$out" "+first version"
rm -rf "$dir"

# --- ⚠️ the case this suite exists for: the tree is gone, so the whole diff -
dir="$(scratch)"
packet "$dir" "first version" > /dev/null
record_round "$dir" blocking
rm -f "$dir"/target/review/*.tree
out="$(packet "$dir" "second version")"
check "a missing tree still names the round" "$out" "This is round 2 of 3"
check "a missing tree falls back to the whole diff" "$out" "## The staged diff"
check "and says why" "$out" "recorded no tree"
check "the whole diff is genuinely whole" "$out" "+second version"
refute "no delta is claimed" "$out" "## What changed since the last round"
rm -rf "$dir"

# --- a minor in an earlier round is not an open finding --------------------
dir="$(scratch)"
packet "$dir" "first version" > /dev/null
record_round "$dir" minor
out="$(packet "$dir" "second version")"
check "a minor leaves nothing open" "$out" "Nothing blocking or major"
refute "and is not listed" "$out" "a fixture finding"
rm -rf "$dir"

# --- the tree object is gone, not just the sidecar ------------------------
# ⚠️ A different branch from the one above, and review noticed it had no case:
# the sidecar names a tree `git cat-file` cannot find, which is ordinary once
# `target/` has been cleaned or gc has run.
dir="$(scratch)"
packet "$dir" "first version" > /dev/null
record_round "$dir" blocking
for tree in "$dir"/target/review/*.tree; do
  printf '%s\n' "0000000000000000000000000000000000000000" > "$tree"
done
out="$(packet "$dir" "second version")"
check "a pruned tree falls back to the whole diff" "$out" "## The staged diff"
check "and says which of the two reasons it was" "$out" "no longer in the object database"
check "the whole diff is genuinely whole" "$out" "+second version"
refute "no delta is claimed" "$out" "## What changed since the last round"
rm -rf "$dir"

# --- the delta is empty: the tree is the one staged now --------------------
dir="$(scratch)"
packet "$dir" "first version" > /dev/null
record_round "$dir" blocking
# Nothing staged since, so tree and index agree and the delta is empty. The
# hash is unchanged too, which makes this round one again -- the packet must
# still print the diff rather than a heading with nothing under it.
out="$(packet "$dir" "first version")"
check "an empty delta still shows the diff" "$out" "## The staged diff"
check "and the diff is not empty" "$out" "+first version"
refute "no delta is claimed" "$out" "## What changed since the last round"
rm -rf "$dir"

# --- the lenses are selected from the paths, not from the author ----------
# ⚠️ Run directly rather than through a packet: a lens that fails to appear is
# invisible in a packet and obvious here. `M5.52`.
lenses() { (cd "$ROOT" && bash scripts/review-lenses.sh "$@"); }

out="$(lenses docs/internal/product/backlog.md)"
check "a docs path selects the prose lens" "$out" "false before anyone re-reads it"
refute "and not the write-path lens" "$out" "acknowledged data, lost silently"
refute "and not the gate lens" "$out" "reports success while checking nothing"

out="$(lenses scripts/check-drift.sh)"
check "a gate path selects the gate lens" "$out" "reports success while checking nothing"
refute "and not the prose lens" "$out" "false before anyone re-reads it"

out="$(lenses crates/oqueue-compact/src/merge.rs)"
check "the write path selects its own lens" "$out" "acknowledged data, lost silently"
check "and a .rs file selects the test lens" "$out" "without constraining anything"

out="$(lenses crates/oqueue-core/src/store.rs)"
check "a core seam selects the fakes lens" "$out" "fakes no longer say what the backends do"

# ⚠️ The case review round one caught: `*commit*` matched eight unrelated
# paths, and the three-paths case below counted only the lens it expected.
out="$(lenses .pre-commit-config.yaml)"
check "the hook config selects the gate lens" "$out" "reports success while checking nothing"
refute "and not the write-path lens" "$out" "acknowledged data, lost silently"

out="$(lenses crates/oqueue-codec/src/offset_commit.rs)"
refute "a codec named for a commit is not the write path" "$out" "acknowledged data, lost silently"
check "it is still Rust, so the test lens applies" "$out" "without constraining anything"

out="$(lenses some/unmatched/file.txt)"
check "a path matching nothing selects nothing" "$out" "No lens matched these paths"
refute "rather than everything" "$out" "### Lens:"

out="$(lenses scripts/a.sh scripts/b.sh .pre-commit-config.yaml)"
if [ "$(printf '%s' "$out" | grep -c '^### Lens: a gate')" = "1" ]; then
  green "ok   three selecting paths print one lens"
  PASS=$((PASS + 1))
else
  red "FAIL three selecting paths print one lens"
  FAIL=$((FAIL + 1))
fi

# --- the packet's cap and the standard's cap are the same number -----------
# ⚠️ ⚠️ **The comment this replaces claimed they could not drift.** They are two
# copies -- a shell constant and a sentence in `review.md` rule 15a -- so the
# claim needs a check rather than a promise.
cap="$(awk -F= '$1 == "REVIEW_ROUND_CAP" { print $2; exit }' "$ROOT/scripts/review.sh")"
case "$cap" in
  2) word="Two" ;;
  3) word="Three" ;;
  4) word="Four" ;;
  *) word="" ;;
esac
standard="$(cat "$ROOT/docs/internal/standards/review.md")"
if [ -z "$word" ]; then
  red "FAIL the cap is $cap, which this suite has no word for -- add one"
  FAIL=$((FAIL + 1))
else
  check "review.md rule 15a states the cap review.sh uses" \
    "$standard" "**$word rounds is the cap**"
fi

# --- the override file says what the packet says about signing -------------
# ⚠️ Both directions, the discipline `AGENTS.md` uses for its own "every script
# exists" paragraph: a grant recorded while the file still says none is in
# force is a contradiction, and so is a signed line with no grant above it.
# ── A gate that cannot run here is not a gate that passed ───────────────────
#
# ⚠️ **`M5.55`, and the shape is a silent success rather than a failure.**
# `lib.sh`'s `skip` prints a line and the gate exits **zero**, so a packet that
# knew only pass and fail reported a gate that never ran as green. A reviewer
# reading that has been told the tree is checked where it is not — `M0.17`'s
# rule, reached from the other side.
#
# Three planted gates, because the classification has three outcomes and each
# one has to be observed: one that skips without running anything, one that
# skips a leg but reports an `ok` for what it did check, and one that fails.
#
# ⚠️ **The `ok` line carries `lib.sh`'s own two leading spaces**, and a fixture
# without them is what let the first draft ship an anchor matching nothing:
# `lib.sh`'s `ok()` is `printf '%s  ok %s %s'`, so a planted gate printing a
# bare `ok ` tests a format no gate produces and the suite stays green while
# every real gate takes the broken branch. Found by `M5.55`'s first round.
gates="$(scratch)"
mkdir -p "$gates/scripts"
cat > "$gates/scripts/check-aaa-unrunnable.sh" <<'EOF'
#!/usr/bin/env bash
printf 'skip coverage (cargo-llvm-cov not installed)\n'
exit 0
EOF
cat > "$gates/scripts/check-bbb-partly.sh" <<'EOF'
#!/usr/bin/env bash
printf '  ok  the part that did run\n'
printf 'skip the part that did not\n'
exit 0
EOF
cat > "$gates/scripts/check-ccc-broken.sh" <<'EOF'
#!/usr/bin/env bash
printf 'FAIL something real\n'
exit 1
EOF
cat > "$gates/scripts/check-ddd-after.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF
chmod +x "$gates/scripts"/check-*.sh
out="$(packet "$gates" "a change to review")"
check "a gate that could not run says so" "$out" \
  "check-aaa-unrunnable.sh: **not run here** — coverage (cargo-llvm-cov not installed)"
refute "and is not reported as passed" "$out" "check-aaa-unrunnable.sh: passed"
check "a gate that ran and skipped one leg is a pass" "$out" "check-bbb-partly.sh: passed"
check "a gate that failed says so" "$out" "check-ccc-broken.sh: **FAILED**"
# ⚠️ **The alphabetical name is the assertion.** `check-ddd-after.sh` sorts
# after the failing one, so its line proves the list did not stop at the
# failure -- which is what the packet did when this was written.
check "a failure does not stop the list" "$out" "check-ddd-after.sh: passed"
check "the packet says what an unrun gate is worth" "$out" \
  "neither a pass nor a failure"
rm -rf "$gates"

overrides="$(cat "$ROOT/reviews/overrides.md")"
check "the override file states the cap it guards" "$overrides" "rule 15a"
if printf '%s' "$overrides" | grep -q "No standing authority is in force"; then
  # ⚠️ `[^<]`, because the file's own format block shows `approved-by: <name>`
  # as a template. A template is not a signature, and a check that could not
  # tell them apart would red on the day the file was written.
  if printf '%s' "$overrides" | grep -qE 'approved-by: [^<]'; then
    red "FAIL a signature is recorded while the file says no authority is in force"
    FAIL=$((FAIL + 1))
  else
    green "ok   no signature stands against the no-authority sentence"
    PASS=$((PASS + 1))
  fi
else
  check "a grant is recorded, with its scope and expiry" "$overrides" "expires"
fi

printf '\n%s passed, %s failed\n' "$PASS" "$FAIL"
echo "SKIPPED_COUNT $SKIPPED"
[ "$FAIL" -eq 0 ]
