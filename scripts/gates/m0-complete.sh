#!/usr/bin/env bash
# M0's own completion condition. `M0.18`.
#
#   scripts/gates/m0-complete.sh
#
# ## What "complete" means here, and how it differs from M-1's
#
# `m-1-complete.sh` asks a question about *documents*: every non-negotiable in
# `AGENTS.md` names a script, and that script passes. M0's question is about a
# **workspace**, because M0 is the milestone that created one — so this script
# compiles it, runs the tree-scanning gates against it, and asks whether they
# actually saw anything.
#
# ⚠️ **The last part is the whole point.** `M0.14` ran all five tree-scanning
# gates against the finished workspace for the first time and recorded what each
# inspected, because a matcher written for an imagined tree returns zero while
# the gate exits 0 — green, and having looked at nothing. Those counts are
# recorded in `backlog.md` as a dated measurement. **This gate is what stops
# them regressing**, and a dated measurement with nothing re-checking it is a
# fact that was true once.
#
# ## Not a second copy of the backlog
#
# Same rule `m-1-complete.sh`'s header states: this does not check that every M0
# row is `done`. `docs/internal/product/backlog.md` is the authoritative task
# list. A milestone can satisfy this condition with rows still open if those
# rows bear on nothing here, and whether that is actually true is a judgement
# for whoever reads this next to the backlog.
#
# ## ⚠️ Where this runs, and why not in pre-commit
#
# **Standalone, at the milestone boundary.** It compiles the workspace for two
# targets and runs the full negative suite; NFR-56 gives the whole pre-commit
# suite 10 s and this alone is minutes. `m-1-complete.sh` is not a hook either,
# for the same reason.
#
# ⚠️ **It runs `tests/gates/negative.sh` itself.** Until `M0.30` nothing else
# did — `m-1-complete.sh` ran it as part of the *previous* milestone's
# condition, and neither the hooks nor CI did; CI now does, per push. This call
# stays because a commit is made locally, where CI has not run yet. Without this line, M0's four cargo fixtures —
# each of which was vacuously green at some point while being written — would be
# exercised only by a finished milestone's gate.
#
# ## What this cannot check
#
# The limit every gate here accepts: that a script which exists and exits 0 is
# testing the right thing. This asserts structure. Review is the rest.
source "$(dirname "${BASH_SOURCE[0]}")/../lib.sh"

cd "$REPO_ROOT"

require_python || finish

# ⚠️ **NFR-40's two targets, as literals.** Both are first-class, and a matrix
# that silently degrades to one is the failure this names. `cargo check` does
# not link, which is why the non-host leg needs no cross linker and why the
# link check below is host-only.
TARGETS=(x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu)

# ⚠️ **The composition root is excluded from a non-host target**, and this is
# ADR-0007's recorded cost arriving rather than a hole. `bin/oqueue` sets
# mimalloc, whose `libmimalloc-sys` build script runs `cc` — so checking it for
# aarch64 needs `aarch64-linux-gnu-gcc`, and without one the build script fails
# with `ToolNotFound` before rustc is reached. NFR-42 asks for "cargo and a C
# compiler", not a **cross** C toolchain, and `portability.md` rule 9 builds
# release artifacts natively, so nothing in this project requires one.
# Everything under `crates/` is pure Rust and is checked on both targets.
# ⚠️ The exclusion is **conditional**: where a cross `cc` is present the whole
# workspace is checked, so a host that can do more is not held to less.
CROSS_EXCLUDED=oqueue

# The five tree-scanning gates `M0.14` instrumented. ⚠️ `check-core-contract.sh`
# is deliberately absent: alone among the M-1 gates it is scoped to
# `git diff --cached -- '*.rs'` and skips outright when no Rust is staged, so
# "what it inspected on this workspace" is not a question it can answer. The
# evidence that it fires on real Rust is `M0.9`, `M0.10` and `M0.11`, each of
# which it refused without an ADR staged beside the new trait.
INSTRUMENTED=(
  check-layering.sh
  check-sans-io.sh
  check-unsafe.sh
  check-file-size.sh
  check-readmes.sh
)

# The four gates M0 itself added. ⚠️ **A gate nothing invokes is a preference**,
# and a gate nobody has watched fail is untested — so each must be named by
# `.pre-commit-config.yaml` or CI *and* have a case in the negative suite.
M0_GATES=(
  check-crate.sh
  check-coverage.sh
  check-budget.sh
  check-mutants.sh
)

# NFR-55 and NFR-56's constants, and the values the requirements table states.
# ⚠️ Checked here as well as by `check-drift.sh` because they are different
# questions: `check-drift.sh` asks whether *any* threshold reads the
# environment, and this asks whether **these two** still hold the numbers two
# requirements were agreed against. A literal nothing can move is only half of
# it; the other half is that it is still the agreed number.
declare -A NFR_CONSTANTS=(
  ["scripts/check-coverage.sh|COVERAGE_FLOOR"]="85"
  ["scripts/check-budget.sh|BUDGET_MS"]="10000"
  # ⚠️ **`COMPILING_GATE_MS` is not a requirement's number and is here anyway**,
  # because it decides whether NFR-56's budget is *applied*: a run holding one
  # gate above it is reported as compiling rather than over. A threshold that
  # switches another threshold off is the one most worth pinning, and until
  # `M0.23` it was asserted by nothing, anywhere.
  ["scripts/check-budget.sh|COMPILING_GATE_MS"]="5000"
)

# run_gate <script> [args...]: runs a gate, captures its output into the global
# `GATE_OUT`, and returns its exit code. ⚠️ Never a bare call — `lib.sh` sets
# `-e`, and every caller here needs to *observe* a non-zero exit rather than die
# on it. The same discipline `run_case` and `check-mutants.sh` both needed.
GATE_OUT=""
run_gate() {
  local script="$1"; shift
  local tmp rc=0
  tmp="$(mktemp)"
  "$REPO_ROOT/$script" "$@" > "$tmp" 2>&1 || rc=$?
  GATE_OUT="$(cat "$tmp")"
  rm -f "$tmp"
  return "$rc"
}

report_gate() {
  local label="$1" rc="$2"
  if (( rc == 0 )); then
    ok "$label"
    return 0
  fi
  fail "$label (exit $rc)"
  note "captured output:"
  sed 's/^/     /' <<< "$GATE_OUT" >&2
  return 1
}

# ── 1. The workspace compiles for both targets, and links on the host ───────
if ! has_rust; then
  fail "no Cargo.toml at the repository root — M0 is the milestone that adds one"
  finish
fi
# ⚠️ **A `fail`, not `require_tool`'s `skip`** — the same departure `lib.sh`
# makes for `require_python`, and for the reason its comment gives: "skip stays
# the default for genuinely optional tools; this is not one of them." Measured:
# with `require_tool cargo ... || finish`, running this on a minimal `PATH`
# (cargo lives in `~/.cargo/bin`, so: any container, any non-login shell)
# printed `skip cargo not installed` and **exited 0** — the milestone's own
# completion gate declaring M0 complete having compiled nothing, scanned
# nothing, and skipped the boundary-review refusal it is currently and
# correctly producing. A completion gate that passes by being unable to run is
# the worst instance of the shape this whole milestone is about. Found by
# review.
if ! command -v cargo >/dev/null 2>&1; then
  fail "cargo not found, and this gate cannot assert anything about M0 without it"
  note "install Rust via https://rustup.rs"
  note "⚠️ a skip here would report M0 complete having checked nothing"
  finish
fi

HOST_TARGET="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')"
if [[ -z "$HOST_TARGET" ]]; then
  fail "could not determine the host target from rustc -vV"
  finish
fi

for target in "${TARGETS[@]}"; do
  args=(check --workspace --locked --all-targets --target "$target")
  scope="--workspace"
  if [[ "$target" != "$HOST_TARGET" ]] &&
     ! command -v "${target%%-*}-linux-gnu-gcc" >/dev/null 2>&1; then
    args+=(--exclude "$CROSS_EXCLUDED")
    scope="--workspace --exclude $CROSS_EXCLUDED"
  fi
  rc=0
  # ⚠️ `--all-targets`, so tests and benches are checked too. Without it a
  # `tests/` file that stopped compiling passes this gate, and NFR-40 is a
  # claim about the whole workspace rather than about its libraries.
  out="$(mktemp)"                  # mktemp, not /tmp -- portability.md rule 16
  cargo "${args[@]}" > "$out" 2>&1 || rc=$?
  if (( rc == 0 )); then
    ok "cargo check $scope passes for $target"
    [[ "$scope" == "--workspace" ]] ||
      note "⚠️ $CROSS_EXCLUDED was not cross-checked: no ${target%%-*}-linux-gnu-gcc for its C allocator (ADR-0007)"
  else
    fail "cargo check $scope fails for $target (exit $rc)"
    note "captured output:"
    tail -30 "$out" >&2
  fi
  rm -f "$out"
done

# ⚠️ **Linking is a separate question from checking**, and the one `cargo check`
# cannot answer: `bin/oqueue` sets a C allocator (ADR-0007), so a host with no
# `cc` gets all the way through both checks above and fails here. Host only —
# the aarch64 leg would need a cross linker, which NFR-42 does not ask for.
rc=0
link_out="$(mktemp)"
cargo build --workspace --locked > "$link_out" 2>&1 || rc=$?
if (( rc == 0 )); then
  ok "cargo build --workspace links on the host"
else
  fail "cargo build --workspace does not link on the host (exit $rc)"
  note "⚠️ bin/oqueue links a C allocator — ADR-0007 — so this needs a cc"
  tail -30 "$link_out" >&2
fi
rm -f "$link_out"

# ── 2. Every pub trait in oqueue-core has a fake beside it, in oqueue-core ──
#
# ⚠️ **Beside it, in `oqueue-core`** — not in `oqueue-testkit`.
# `contracts.md` rules 9 and 11 put every fake in the crate that owns the trait,
# and `oqueue-testkit` holds harness and generators and no fake. ⚠️
# `milestones/M0.md`'s completion condition says `oqueue-testkit`; it
# contradicts both that standard and its own Goal section, which says "a fake
# beside it". This gate follows the standard. Recorded rather than silently
# resolved, because a plan and a gate disagreeing is worth a reader knowing.
trait_report="$(python3 - <<'PYEOF'
import pathlib, re, sys

src = pathlib.Path("crates/oqueue-core/src")


def uncommented(text):
    """`text` with `//`-to-end-of-line removed from every line.

    ⚠️ **Line comments only, and that is a decision with a cost in *both*
    directions.** A doc comment quoting `pub trait Foo` is the realistic false
    positive and this removes it. A `pub trait` inside a `/* */` block is still
    counted — a false **red**, the safe direction. ⚠️ **But a `Fake` struct and
    its `impl` inside a `/** */` block are also still counted**, and that one
    is a false **green** in the only automated enforcement of `contracts.md`
    rule 9: measured, a doc block containing `pub struct FakeClock;` and
    `impl Clock for FakeClock` satisfies a trait with no real fake anywhere.
    Recorded rather than solved, because solving it means a block-aware
    stripper and the one this repository has is subtle enough to have failed
    twice — see below. `check-core-contract.sh` carries a
    full string-and-block-aware stripper whose docstring records two ways a
    naive block pass failed, one of which silently deleted every trait after
    an unmatched `/*` in a string literal. Copying it here would be a second
    copy of a subtle thing (`build.md` rule 22); needing it here is what would
    justify extracting it to a module both can import."""
    return "\n".join(re.sub(r"//.*$", "", ln) for ln in text.splitlines())


traits, fakes = {}, set()
for f in sorted(src.rglob("*.rs")):
    text = uncommented(f.read_text(encoding="utf-8"))
    # ⚠️ `^\s*`, not `^`. An anchored match missed a `pub trait` declared inside
    # an inline `pub mod` — one level of indentation and the only automated
    # enforcement of `contracts.md` rule 9 was bypassed, silently and green.
    # `check-core-contract.sh` holds no fake logic and is diff-scoped, so this
    # is the whole of it. Found by review.
    for m in re.finditer(r"^\s*pub trait ([A-Za-z0-9_]+)", text, re.M):
        # ⚠️ Keyed by `(name, file)`. Two same-named `pub trait`s in different
        # modules is legal Rust, and a dict keyed by bare name silently kept
        # the last one — so a fakeless trait could be masked by a same-named
        # one that has a fake.
        traits[(m.group(1), f)] = f
    for m in re.finditer(r"^\s*(?:pub )?(?:struct|enum) (Fake[A-Za-z0-9_]+)", text, re.M):
        fakes.add(m.group(1))
    for m in re.finditer(r"^\s*impl ([A-Za-z0-9_]+) for (Fake[A-Za-z0-9_]+)", text, re.M):
        fakes.add(f"IMPL:{m.group(1)}:{m.group(2)}")

if not traits:
    print("PROBLEM no pub trait found in oqueue-core -- the scanner matched nothing")
for (name, _), f in sorted(traits.items(), key=lambda kv: (kv[0][0], str(kv[0][1]))):
    fake = f"Fake{name}"
    if fake not in fakes:
        print(f"PROBLEM {f}: pub trait {name} has no {fake} in oqueue-core")
    elif f"IMPL:{name}:{fake}" not in fakes:
        print(f"PROBLEM {f}: {fake} exists but does not impl {name}")
print(f"COUNT {len(traits)}")
PYEOF
)" || { fail "the pub-trait scanner crashed"; finish; }

trait_problems=0
while IFS= read -r line; do
  case "$line" in
    "PROBLEM "*) fail "${line#PROBLEM }"; trait_problems=$((trait_problems + 1)) ;;
    "COUNT "*) trait_count="${line#COUNT }" ;;
  esac
done <<< "$trait_report"
if (( trait_problems == 0 )); then
  ok "every pub trait in oqueue-core has a fake beside it (${trait_count:-0} trait(s))"
fi

# ── 3-4. The three named gates pass, and the five instrumented ones saw code ─
#
# ⚠️ Three of M0's requirements name `check-layering.sh`, `check-sans-io.sh` and
# `check-unsafe.sh` as their *whole* verification, so a milestone that ends with
# any of them failing has not met them. All three are also in the instrumented
# set below and are run once, there.
for gate in "${INSTRUMENTED[@]}"; do
  rc=0
  run_gate "scripts/$gate" || rc=$?
  report_gate "$gate passes" "$rc" || continue
  # ⚠️ **The count, not just the exit code.** This is the regression `M0.14`'s
  # measurement exists to catch: a matcher that stops matching turns a real
  # inspection into a vacuous one with no change in exit status.
  n="$(grep -oE '[0-9]+ (\.rs )?(file|crate|manifest)\(s\)' <<< "$GATE_OUT" |
       head -1 | grep -oE '^[0-9]+' || true)"
  if [[ -z "$n" ]]; then
    fail "$gate reports no inspected count — M0.14's measurement cannot be re-derived"
    note "its output was: $(head -1 <<< "$GATE_OUT")"
  elif (( n == 0 )); then
    fail "$gate inspected 0 — green having looked at nothing"
  else
    ok "$gate inspected $n"
  fi
done

# ── 5. NFR-55 and NFR-56's constants are literals, and still the agreed ones ─
for key in "${!NFR_CONSTANTS[@]}"; do
  file="${key%%|*}"
  name="${key##*|}"
  want="${NFR_CONSTANTS[$key]}"
  # ⚠️ Tolerates `readonly` and leading whitespace: an anchored `^NAME=` said
  # "does not assign NAME at all" about a file that plainly does.
  line="$(grep -E "^[[:space:]]*(readonly[[:space:]]+)?${name}=" "$file" 2>/dev/null | head -1 || true)"
  if [[ -z "$line" ]]; then
    fail "$file does not assign $name at all"
    continue
  fi
  # ⚠️ A literal, so `${NAME:-85}` and `$NAME` both fail. `check-drift.sh`
  # enforces this across every threshold; here it is asserted for the two the
  # requirements table names, against the values it names.
  got="${line#*=}"
  got="${got%%#*}"                 # a trailing `# NFR-55` is not part of the value
  got="${got%"${got##*[![:space:]]}"}"
  if [[ "$got" != "$want" ]]; then
    # ⚠️ Phrased as what *this gate* holds, not as what the table says: nothing
    # here reads `requirements.md`, and a message asserting its contents would
    # be a claim this script cannot back. ⚠️ It is a **third copy** of both
    # numbers — the script, the requirements row, and here — which is the cost
    # of asserting them at all, and the reason the remedy names every place.
    fail "$file: $name is '$got'; M0.18's gate holds $want"
    note "raising it is fine and is a requirements change — update the script,"
    note "this gate's NFR_CONSTANTS, and requirements.md's NFR-55/NFR-56 row together"
  else
    ok "$name is the literal $want in $file"
  fi

  # ⚠️ **And visible to `check-drift.sh`**, which is the half that kept
  # failing. Non-negotiable 2 is enforced by a *name* matcher, so a constant
  # nobody named conventionally is unenforced while every gate reports green —
  # `M0.15` found that with `MIN_CRATE_COVERAGE` and recorded that the name is
  # load-bearing, and `M0.16` wrote two more the matcher could not see on the
  # very next commit. Asserting the two scripts agree is what makes the
  # convention a check instead of a thing to remember. `M0.23`.
  # ⚠️ `|| true`, and no `head` in the pipeline. Without the first, a
  # non-matching grep aborts the whole gate under `lib.sh`'s `pipefail` and the
  # `-z` branch below is unreachable — the early-abort shape section 5b's own
  # comment rejects, introduced one section earlier by the commit that wrote
  # this. Without the second, two matching lines SIGPIPE the grep, which is
  # `portability.md` rules 21-22, cited forty lines further down this file.
  # ⚠️ Quotes optional on both sides, so `THRESHOLD_RE="..."` extracts too
  # rather than yielding a regex of one stray character.
  drift_re="$(grep -E "^[[:space:]]*(readonly[[:space:]]+)?THRESHOLD_RE=" scripts/check-drift.sh |
    sed -E "1!d; s/^[^=]*=//; s/^['\"]//; s/['\"]$//" || true)"
  if [[ -z "$drift_re" ]]; then
    fail "could not read THRESHOLD_RE from scripts/check-drift.sh"
  elif printf '%s\n' "$name" | grep -qEi "$drift_re"; then
    ok "$name is visible to check-drift.sh"
  else
    fail "$name is invisible to check-drift.sh — non-negotiable 2 does not cover it"
    note "its THRESHOLD_RE is: $drift_re"
    note "widen that regex, or rename the constant — M0.15's note: the name is load-bearing"
  fi
done

# ── 5b. The hook count NFR-56 is stated against ────────────────────────────
#
# ⚠️ **Three documents held three different numbers**, which `M0.25` found:
# `requirements.md` and `roadmap.md` said 15 and a comment in `check-budget.sh`
# said 16, against a config with more than either. NFR-56's measurement is
# "2.27 s across N hooks", so N is part of the claim — a budget measured across
# a suite that has since grown is a floor nobody restated. Alone among the ten
# claims that row corrected, this one is a number a script can count, so it is
# the one that gets a gate rather than a repair.
# ⚠️ **Both documents, because both held the stale 15.** Gating one and
# correcting the other by hand would leave exactly the state this row exists to
# end. Matched on the `NFR-56`/budget sentence rather than the whole file, so a
# `today` written elsewhere in either document is not mistaken for this claim.
# ⚠️ **Hooks that run at the `pre-commit` stage, not every `- id:` line.**
# NFR-56's series — "14 hooks, 15 once this gate joined them" — counts the
# suite `check-budget.sh` actually sums, and `check-commit-msg` and
# `check-tests-kept` are `stages: [commit-msg]`: a separate git-hook
# invocation whose timing rows can never enter that process group's total. A
# raw `- id:` count is 18 against a suite of 16, and it would have forced an
# edit to a *measurement* row every time somebody added a `commit-msg` hook
# that costs the pre-commit suite nothing. Found by review, which also found
# the first version's `^      - id: ` to be exactly-six-spaces and therefore
# blind to a second repo block written in pre-commit's own 4-space style —
# silent green from the gate written to stop a number going stale.
# ⚠️ **PyYAML if it is there, a line parse if it is not.** `yaml` would be the
# only third-party import in `scripts/`, and "pre-commit depends on it" does
# not survive a pipx or brew install, where PyYAML lives in pre-commit's venv
# and not in system `python3`. Measured by review with the module shadowed: the
# gate aborted at 13 checks with a message reading like a malformed config, and
# sections 6 through 8 — every M0 gate invoked and watched to fail, the
# negative suite, the boundary review — never ran. A completion gate that stops
# early because of an import is the shape this milestone is about.
# ⚠️ One parse, two answers: how many hooks run at the `pre-commit` stage, and
# which gate scripts they invoke. Section 6 needs the second, and asking twice
# with two different parsers is how a gate ends up disagreeing with itself.
parsed="$(python3 - <<'PYEOF'
import re

text = open(".pre-commit-config.yaml").read()
hooks = []                      # (id, stages|None, entry)


def runs_at_pre_commit(stages):
    return stages is None or "pre-commit" in stages


try:
    import yaml

    try:
        cfg = yaml.safe_load(text) or {}
    except yaml.YAMLError as exc:
        print("UNPARSEABLE", str(exc).replace("\n", " ")[:120])
        raise SystemExit(0)
    for repo in cfg.get("repos", []) or []:
        for hook in repo.get("hooks", []) or []:
            hooks.append((hook.get("id", ""), hook.get("stages"), hook.get("entry", "")))
except ImportError:
    # ⚠️ **`ImportError` only, and a `YAMLError` is a `fail` below.** Catching
    # every exception was the first fix and it went one step too far: a config
    # `pre-commit` itself cannot load then fell through to a regex that happily
    # counted it, so the gate reported a hook count and "every gate is invoked"
    # for a file nothing can read. Loud is right there; the fallback is for a
    # missing *module*, not a broken document. Both directions found by review.
    #
    # One hook per `- id:`, its block running to the next `- id:` at any depth
    # — indentation-agnostic, which an anchored six-space pattern was not.
    for block in re.split(r"\n(?=\s*- id: )", text):
        m = re.match(r"\s*- id: (\S+)", block)
        if not m:
            continue
        # ⚠️ Both YAML spellings of a sequence. The flow form `[a, b]` was all
        # the first version read, so a block form —
        #   stages:
        #     - commit-msg
        # — left `stages` unseen and a commit-msg hook counted as a pre-commit
        # one, making the two parse paths disagree, which is the single thing
        # the fallback exists to prevent. Found by review.
        # ⚠️ Comments stripped and the key line-anchored. Unanchored, a comment
        # reading `# never park this at stages: [manual]` was read as the
        # hook's configuration and the two parse paths disagreed — on a machine
        # without PyYAML, which is the one thing this branch exists to prevent.
        # Found by review.
        block = re.sub(r"#.*$", "", block, flags=re.M)
        flow = re.search(r"^\s*stages:\s*\[([^\]]*)\]", block, re.M)
        if flow:
            stages = [x.strip().strip("'\"") for x in flow.group(1).split(",") if x.strip()]
        elif re.search(r"^\s*stages:\s*$", block, re.M):
            # ⚠️ **Stops at the next key.** An unbounded scan from `stages:` to
            # end-of-block swallowed any later block sequence in the same hook:
            # measured, `stages:` → `- commit-msg` followed by
            # `additional_dependencies:` → `- pre-commit` made the fallback
            # count a commit-msg hook as a pre-commit one and report its gate
            # invoked — the same false green the `stages: [manual]` fix in this
            # row removes. Found by review.
            tail = block.split("stages:", 1)[1]
            stages = []
            for ln in tail.splitlines():
                if re.match(r"^\s*-\s*\S+\s*$", ln):
                    stages.append(ln.strip().lstrip("-").strip())
                elif ln.strip() and not ln.lstrip().startswith("#"):
                    break
        else:
            stages = None
        entry = re.search(r"^\s*entry:\s*(.+)$", block, re.M)
        hooks.append((m.group(1), stages, entry.group(1).strip() if entry else ""))

print("COUNT", sum(1 for _, st, _ in hooks if runs_at_pre_commit(st)))
for _, st, entry in hooks:
    if runs_at_pre_commit(st) and entry:
        print("ENTRY", entry)
PYEOF
)" || parsed=""
if grep -q '^UNPARSEABLE ' <<< "$parsed"; then
  fail "PyYAML cannot parse .pre-commit-config.yaml — pre-commit cannot either"
  note "$(grep '^UNPARSEABLE ' <<< "$parsed" | sed 's/^UNPARSEABLE //')"
  parsed=""
fi
hooks="$(grep '^COUNT ' <<< "$parsed" | head -1 | sed 's/^COUNT //' || true)"
# ⚠️ `entry:` lines of **`pre-commit`-stage hooks only**. A hook parked at
# `stages: [manual]` keeps its `entry:` line and runs on no commit, so a plain
# grep for the script name called it invoked — the mention-versus-invocation
# error this loop already names for comments and `name:` keys, one level
# deeper. Found by review.
PRECOMMIT_ENTRIES="$(grep '^ENTRY ' <<< "$parsed" | sed 's/^ENTRY //' || true)"
# ⚠️ **`fail` and carry on, never `finish`.** This gate holds eight independent
# assertions and one unreadable file is not a reason to stop asking the other
# seven — a missing config used to abort here, so sections 6 through 8 (every
# M0 gate invoked and watched to fail, the negative suite, the boundary review)
# never ran and the run ended looking like a config problem. Review named the
# shape on `M0.25`; `M0.26`'s own fixture then walked straight into it.
if [[ -z "$hooks" ]]; then
  fail "could not read a hook count from .pre-commit-config.yaml"
  note "NFR-56 is measured across a suite, so its size is part of the claim"
  hook_count_checked=0
else
  hook_count_checked=1
fi
hook_count_ok=$hook_count_checked
for doc in $( ((hook_count_checked)) && echo docs/internal/product/requirements.md docs/internal/product/roadmap.md ); do
  # ⚠️ **The paragraph, not the line.** `**N today**` only has to be in the
  # same block of prose as the trigger, not on the same physical line: a
  # 240-character sentence reflowed to 78 columns is a formatting edit no hook
  # objects to, and it used to turn this gate red with a message contradicting
  # the document it was reading. A blank line ends the block, so a `today`
  # written in some other paragraph is still not mistaken for this claim.
  stated="$(python3 - "$doc" <<'PYEOF'
import re, sys

text = open(sys.argv[1]).read()
# ⚠️ A markdown **table row** is its own unit, because a table is one
# paragraph and an unrelated `**N today**` in a neighbouring row would
# otherwise be read as NFR-56's. Measured by review with an invented NFR-99
# row: the gate failed naming a claim NFR-56 does not make.
blocks = []
for para in re.split(r"\n\s*\n", text):
    blocks.extend(para.splitlines() if para.lstrip().startswith("|") else [para])
for block in blocks:
    if not re.search(r"NFR-56|budget gate itself joined", block):
        continue
    m = re.search(r"\*\*(\d+) today\*\*", block)
    if m:
        print(m.group(1))
        break
PYEOF
  )" || stated=""
  if [[ -z "$stated" ]]; then
    fail "$doc states no current hook count for NFR-56"
    note "the sentence reads '... **N today**'; this gate counts the config and compares"
    hook_count_ok=0
  elif [[ "$hooks" != "$stated" ]]; then
    fail "$doc says $stated hooks, .pre-commit-config.yaml has $hooks"
    note "NFR-56 is measured across a suite; the size of that suite is part of the claim"
    hook_count_ok=0
  fi
done
(( hook_count_ok == 1 )) && ok "both documents state the config's hook count ($hooks)"

# ── 6. Every gate M0 added is invoked, and has been watched to fail ─────────
for gate in "${M0_GATES[@]}"; do
  invoked=0
  # ⚠️ **An invocation, not a mention**, and this is not hypothetical:
  # `.github/workflows/gates.yml` names `check-crate.sh`, `check-budget.sh` and
  # `check-coverage.sh` **only inside comments**, and their one real invocation
  # is a pre-commit `entry:` line. A substring search therefore reported every
  # one of them invoked, and deleting its hook block would have left the gate
  # running nowhere while this said otherwise. Found by review — which had
  # already stated the rule for the negative-suite half of this same loop.
  grep -qE "${gate//./\.}" <<< "$PRECOMMIT_ENTRIES" && invoked=1
  # ⚠️ In a workflow a gate is invoked by a `run:` step, so a `name:` key that
  # merely labels a step does not count — comments are stripped first, and then
  # `name:` lines are dropped. Without the second half, a step reading
  # `- name: check-coverage.sh (temporarily disabled)` / `run: echo skipping`
  # reported the gate invoked while nothing ran it. Review reproduced it, and
  # it is the same mention-versus-invocation error as the line above, one
  # comment away from where that error was already named.
  if [[ -d .github/workflows ]]; then
    # ⚠️ No `grep -q` at the end of a pipeline — `portability.md` rule 21. `-q`
    # exits at the first match, the `sed` upstream takes SIGPIPE, and under
    # `pipefail` the pipeline reports 141 whatever it found. Reproduced by
    # review at 15 MB of YAML: three runs of three, a gate CI does invoke
    # reported as invoked by nothing. `grep -c` reads to the end, so nothing
    # upstream is ever signalled.
    ci_hits=0
    ci_hits="$(sed 's/#.*$//' .github/workflows/*.y*ml 2>/dev/null |
      grep -vE '^[[:space:]]*-?[[:space:]]*name:' |
      grep -cE "${gate//./\.}" || true)"
    (( ci_hits > 0 )) && invoked=1
  fi
  if (( invoked == 0 )); then
    fail "$gate is invoked by neither .pre-commit-config.yaml nor CI — a preference, not a gate"
  else
    ok "$gate is invoked"
  fi
  # ⚠️ A `run_case` line, not a mention: the file names each gate in prose too,
  # and a comment is not a case.
  # ⚠️ `^[[:space:]]*`, not `^`: the calls are indented inside the suite's
  # dispatch block, and an anchored `^run_case` reported two gates as never
  # watched to fail while their cases were sitting there passing. Found by this
  # gate's own first run.
  if grep -qE "^[[:space:]]*run_case +\"?${gate}" tests/gates/negative.sh 2>/dev/null; then
    ok "$gate has a case in tests/gates/negative.sh"
  else
    fail "$gate has no run_case in tests/gates/negative.sh — never watched to fail"
  fi
done

# ── 7. The negative suite itself ────────────────────────────────────────────
rc=0
run_gate tests/gates/negative.sh || rc=$?
report_gate "tests/gates/negative.sh: every gate fails on a broken artifact" "$rc" || true

# ⚠️ **A case that did not run is not a case that passed**, and the line above
# could not tell the difference: the suite exits 0 with a case skipped for a
# missing tool, which is correct for the suite and wrong for this claim. On a
# machine without `cargo-llvm-cov` and `cargo-mutants` this gate declared half
# of M0's four new gates complete having watched neither fail. That is the
# skip-is-a-pass shape this same script refuses for `cargo`, with a note saying
# a skip there "would report M0 complete having checked nothing". `M0.26`.
#
# ⚠️ An **absent** line is a failure too, not a pass: it means the suite is
# older than this contract and cannot answer, which is exactly the state that
# looked like success before.
if ! grep -q '^SKIPPED_CASES' <<< "$GATE_OUT"; then
  fail "tests/gates/negative.sh reported no SKIPPED_CASES line — it cannot say what did not run"
  note "the suite must print 'SKIPPED_CASES <gate.sh> ...', empty when none skipped"
else
  skipped="$(grep '^SKIPPED_CASES' <<< "$GATE_OUT" | head -1 | sed 's/^SKIPPED_CASES *//')"
  unproven=""
  for gate in "${M0_GATES[@]}"; do
    for sk in $skipped; do
      [[ "$sk" == "$gate" ]] && unproven+="$gate "
    done
  done
  if [[ -n "$unproven" ]]; then
    fail "these M0 gates were never watched to fail on this machine: ${unproven% }"
    note "their negative cases were skipped for a missing tool — install it and re-run"
    note "⚠️ a skipped case is not a passing one, and M0 is not complete from here"
  else
    ok "every M0 gate's negative case actually ran"
  fi
fi

# ── 8. The outer loop: every M0 commit read as a whole ──────────────────────
rc=0
run_gate scripts/check-milestone-review.sh --milestone M0 || rc=$?
report_gate "check-milestone-review.sh: every M0 commit is covered by a milestone review" "$rc" || true

finish
