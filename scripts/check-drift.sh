#!/usr/bin/env bash
# A threshold that was a fixed constant does not become settable. `M-1.7`.
#
#   scripts/check-drift.sh
#
# Non-negotiable 2: "thresholds are constants no environment can move." This
# is the half of that rule about *how* a threshold could move — the other
# half, a value silently **weakened**, is `check-tests-kept.sh`'s sibling
# concern for tests. ⚠️ Not "lowered": for **three** of the seven thresholds
# `m0-complete.sh` pins — `FILE_LINE_LIMIT`, `BUDGET_MS`, `EXEMPT_RUNS_FLOOR`
# — raising is the weakening move, and `TIMINGS_KEEP_DAYS` silences its
# warning in *both* directions. ⚠️ `EXEMPT_RATE_THRESHOLD` is **not** in that
# list: it is the rate's denominator, so raising it tightens. Read
# `m0-complete.sh`'s per-entry table rather than generalising from a suffix —
# two of its seven rows were wrong across two drafts. ⚠️ Non-negotiable 2's
# canonical wording now names the weakening *direction* rather than "lower"
# (`M2.7`, closing `M1.49`), so this header and `AGENTS.md` finally say the
# same thing — rust-style.md rule 7's raising-a-clippy-ceiling example was
# already the new wording's shape. ⚠️ ~~see "What this does not catch" below~~ — **no such section
# exists in this file** (`M1.35`). The half-gate it pointed at now has a real
# answer: `m0-complete.sh`'s `NFR_CONSTANTS` pins each threshold's *value*, so
# moving one in either direction fails a gate. ⚠️ **Two limitations, and the second matters more.**
# The map is hand-maintained (see `THRESHOLD_RE` below). And `m0-complete.sh` is
# a *milestone-boundary* gate — invoked by neither `.pre-commit-config.yaml` nor
# `gates.yml` — so moving a value lands green on the commit path and is caught
# whenever someone next runs that gate. This script is on the commit path; the
# one it points at is not.
#
# ## What "settable" means here
#
# A threshold (`too-many-lines-threshold`, a coverage floor, a time budget, a
# regression percentage — anything `performance.md`, `testing.md`, or
# `build.md` names as a constant) stops being a constant the moment its value
# can be supplied from outside the commit that sets it: an environment
# variable, `option_env!`, or a shell parameter expansion with a fallback
# (`${VAR:-default}`). A constant an environment can move is a constant in
# name only — CI, a laptop, and a release build would each see a different
# number, and non-negotiable 2 stops meaning anything.
#
# ## The mechanism, and its honest limits
#
# This is a grep gate, not a parser: a line naming a threshold-shaped
# identifier (matched by *substring*, not a Rust or shell identifier
# boundary — portable across GNU and BSD grep, which disagree about `\b`) and
# an environment-read call **on the same line** fails. That catches the
# straightforward case:
#
#   let cognitive_threshold = std::env::var("COGNITIVE_THRESHOLD")...
#   BUDGET_SECONDS="${OQUEUE_BUDGET_SECONDS:-120}"
#
# It does **not** catch a threshold assigned from a variable that is *itself*
# set from the environment several lines away, or one passed through a config
# struct populated elsewhere. Closing that gap needs a real parser and is not
# this task's job; a grep gate that is honest about its blind spot is more
# useful than one that pretends to be exhaustive and is disabled the first
# time it overclaims — the lesson `docs/researches/19` §3.2 states for
# `check-core-contract.sh`.
#
# ⚠️ **This file is the one named exception**, because the two worked examples
# a few lines up put both signals on one line on purpose, to show a reader
# what the gate looks for. Scanning itself would fail forever on its own
# documentation — measured, not assumed: it does, unconditionally, the moment
# those two lines exist. It defines no threshold of its own to protect, so
# excluding it costs no real coverage. Every other file, including every
# other gate script, is still scanned.
#
# ## Why the whole tree, not just the staged diff
#
# A threshold made settable three commits ago is exactly as much a violation
# today as one made settable in this commit. Non-negotiable 2 is a property
# of the tree, not of a diff, so this scans every tracked file every run.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# Case-insensitive. ⚠️ **Mostly substring, and the unit suffixes are anchored** —
# never with `\b`, whose meaning differs across platforms, but with an explicit
# `([^a-z0-9]|$)`, which does not. `threshold`, `_limit`, `_ceiling` and `_floor`
# stay unanchored because a false positive there is a word somebody chose; `_ms`
# unanchored matched `_msg`, which is a word everybody chooses.
#
# ⚠️ **And a false positive is *not* cheap to dismiss**, which this comment used
# to claim — ⚠️ and `M1.35` removed the last place that still quoted the retracted
# phrase as though it were current wording, further down this same file. There is no suppression mechanism in this script — no baseline, no
# per-line escape — so the only exits are renaming a legitimate variable or
# widening the regex, and widening it is editing a non-negotiable-2 gate to make
# a check pass. `_limit` is the live one: `rate_limit_header = env::var(...)` is
# refused and the remedy offered ("hard-code the value") is wrong for it. Adding
# a baseline is the honest fix and belongs to whoever hits it; ⚠️ until then the
# cost of a false positive here is a rename, and the comment says so.
#
# ⚠️ **A name-based matcher only sees what somebody named conventionally**, and
# `M0` demonstrated the failure twice in three commits. `M0.15` found
# `MIN_CRATE_COVERAGE` matched nothing — its gate's acceptance said
# "`check-drift.sh` passes" and it did, having looked at no constant at all —
# renamed it `COVERAGE_FLOOR`, and recorded that **the name is load-bearing**.
# `M0.16`, the next commit, wrote `BUDGET_MS` and `COMPILING_GATE_MS`, and this
# regex saw neither. `_ms` and `_seconds` are here because a duration is the
# other shape a threshold takes; ⚠️ **the class is still open**, and
# ~~`m0-complete.sh` is what makes it not depend on someone choosing the right
# word: it asserts that every constant a requirement names is matched by this
# regex, so a new one that is invisible here fails a gate rather than passing
# quietly.~~ `M0.23`.
#
# ⚠️ **False, measured by `M1.35`.** `m0-complete.sh` asserts that only for the
# constants *listed in its own `NFR_CONSTANTS` literal*, and that list is
# hand-maintained: delete all four of `M1.35`'s entries and the gate still
# exits 0. So a threshold whose name matches neither this regex nor that map is
# unenforced for non-negotiable 2 while every gate reports `ok`. ⚠️ Of the four
# `M1.35` found, **`LIMIT` and `TIMINGS_KEEP_DAYS` were invisible to both** —
# the latter matches this regex only since `M1.35` widened it below.
# `EXEMPT_RATE_THRESHOLD` and `EXEMPT_RUNS_FLOOR` already matched, because
# `M1.30` chose names that would. All four were absent from the map, so none
# was pinned by value.
# **Adding a threshold means making its name visible to this regex *and*
# listing it in that map — the map does not substitute for the regex, since
# `m0-complete.sh` asserts both — and nothing will tell you that you did
# neither.**
# ⚠️ `_ms` and `_seconds` are **suffix-anchored**; the rest stay substrings.
# Unanchored, `_ms` matches `_msg` and `_msvc` — ⚠️ **not `_message`, which this
# line claimed until `M1.24` checked it**: `_message` has no `_ms` in it at all
# (`_me`…), so it never matched, anchored or not. The two that do are enough to
# make the point, and an example that does not match weakens it. ⚠️ Measured
# **before the anchoring below existed** — a plain
# `let err_msg = std::env::var("OQUEUE_BANNER")...` was rejected by the gate,
# with a remedy telling the developer to hard-code a banner string. Against the
# pattern as it stands, `err_msg` matches nothing and no such rejection is
# reachable; the sentence records why the anchoring was added, not what the
# gate does now. ⚠️ **And this script has
# no suppression mechanism**, so dismissing a false positive is not actually
# available — ⚠️ this sentence quoted "cheap to dismiss on sight" as the
# header's wording, which the header itself retracted (`M1.35`): the only exits from a false positive are renaming a
# legitimate variable or widening this regex, and the second is editing a
# non-negotiable-2 gate to make a check pass. A duration constant ends in its
# unit; a message variable does not — ⚠️ **except that `msec` is also a unit
# spelling**, which the first anchoring dropped: `POLL_MSEC="${OQUEUE_POLL_MSEC:-500}"`
# matched before and matched neither branch after, so narrowing to fix a false
# positive opened a false negative on the same gate. Measured by review. The
# optional `ec`/`ecs` and the digit-tolerant tail keep both.
# ⚠️ `_days?` added by `M1.35`. `check-budget.sh`'s `TIMINGS_KEEP_DAYS=30` is a
# retention window, which is the same kind of threshold as one in seconds or
# milliseconds — both of which this regex already matched — so widening is the
# right remedy here rather than the rename `FILE_LINE_LIMIT` took. The choice
# is per-constant: rename when the name is simply wrong — `LIMIT` became
# `FILE_LINE_LIMIT` — and widen when the naming convention has a genuine gap.
#
# ⚠️ **`_timeouts?` added by `M10.18a`.** `SERVE_LIMITS.idle_timeout` is this
# gap's own named example: a `Duration` field named for exactly what it is,
# missing from a pattern that already had `_ms`, `_secs` and `_days`. The same
# widening `M1.35` made for `_days` applies here — a threshold is a threshold
# whatever unit or field shape names it.
THRESHOLD_RE='threshold|_limit|_budget|_ceiling|_floor|_timeouts?([^a-z0-9]|$)|_ms(ecs?)?[0-9]*([^a-z0-9]|$)|_secs?(onds?)?[0-9]*([^a-z0-9]|$)|_days?([^a-z0-9]|$)'

# Rust environment reads, plus the shell idiom for reading one with a
# fallback default. `\$\{[A-Za-z_][A-Za-z0-9_]*:[-=]` matches `${FOO:-...}`
# and `${FOO:=...}`, the two forms that read an environment variable if the
# caller's shell has one set.
ENV_RE='env::var|env::var_os|option_env!|\$\{[A-Za-z_][A-Za-z0-9_]*:[-=]'

# SELF is excluded — see the ⚠️ note above the header's worked examples.
SELF="scripts/$(basename "${BASH_SOURCE[0]}")"

mapfile -t files < <(git ls-files -- '*.rs' '*.sh' 'clippy.toml' '*/clippy.toml' 2>/dev/null \
  | grep -vxF "$SELF" || true)

if (( ${#files[@]} == 0 )); then
  skip "threshold settability (no .rs, .sh, or clippy.toml files tracked yet)"
  finish
fi

violations=0
scanned=0

for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  scanned=$((scanned + 1))

  # Two passes rather than one combined regex: grep -E has no portable
  # same-line "and" without a lookaround GNU and BSD grep both support, and
  # piping grep -n output through a second grep keeps the "lineno:content"
  # shape intact for the loop below.
  matches="$(grep -nEi "$THRESHOLD_RE" "$f" 2>/dev/null | grep -E "$ENV_RE" || true)"
  [[ -n "$matches" ]] || continue

  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    lineno="${m%%:*}"
    content="${m#*:}"
    content="$(printf '%s' "$content" | sed -E 's/^[[:space:]]+//')"
    fail "threshold made settable: $f:$lineno"
    note "$content"
    violations=$((violations + 1))
  done <<< "$matches"
done

if (( violations == 0 )); then
  ok "no threshold reads from the environment ($scanned file(s) scanned)"
else
  note "a threshold is a constant no environment can move — non-negotiable 2"
  note "hard-code the value; if it should differ by environment, that is a"
  note "config value, not a threshold, and belongs to a different standard"
  # ⚠️ **The second remedy, for the case the first one misdiagnoses.** The
  # matcher is by name, so a variable that merely *contains* one of the words
  # above is refused too — `rate_limit_header` is the live one — and telling
  # its author to hard-code an HTTP header name is nonsense. There is no
  # suppression mechanism here, so rename is the exit, and widening the regex
  # to pass is the thing non-negotiable 2 forbids. Found by review, which
  # noted the remedy had been corrected in a comment and not in the output.
  note "⚠️ if the name merely contains one of those words and is not a"
  note "threshold at all, rename it — this gate matches by name and has no"
  note "suppression list, and widening its regex to pass is what rule 2 forbids"
fi

# ── clippy.toml thresholds are pinned by value — `M2.9`, closing `M1.50` ────
#
# `m0-complete.sh`'s NFR_CONSTANTS greps `NAME=` in shell and cannot read
# `key = value` TOML — and that gate runs at milestone boundaries anyway.
# These five sit here, on the commit path: `rust-style.md` rule 7 says a
# clippy threshold is a constant whose raising is the move non-negotiable 2
# forbids, and until this section nothing on any path pinned one. Three
# checks, each with its own reason: a changed value (the move rule 7 names),
# a key this map does not know (a list nobody grows quietly), and a key
# deleted from clippy.toml (which silently re-enables clippy's weaker
# default — for `too-many-lines-threshold`, 100 against the pinned 50).
declare -A CLIPPY_THRESHOLDS=(
  ["too-many-lines-threshold"]="50"
  ["cognitive-complexity-threshold"]="20"
  ["too-many-arguments-threshold"]="5"
  ["type-complexity-threshold"]="250"
  ["enum-variant-size-threshold"]="200"
)
# Guarded: a scratch tree or fork without a clippy.toml has nothing to pin,
# and failing there would make every fixture in negative.sh plant one.
#
# ⚠️ **Exact value-shape matching, not parse-then-compare.** A first draft
# scanned `= [0-9]+` lines and compared parsed values, and review measured
# the hole: `= +100` is 100 to TOML (and so to clippy) but matched the scan
# grep nowhere, so the doubled threshold printed `ok`. Requiring each pinned
# key's line to carry *exactly* the pinned digits closes value changes, sign
# and underscore spellings, and deletion in one check — anything else on that
# line is a fail, which is the right default for a canonical five-line file.
clippy_violations=0
if [[ -f clippy.toml ]]; then
  for key in "${!CLIPPY_THRESHOLDS[@]}"; do
    if ! grep -qE "^${key}[[:space:]]*=[[:space:]]*${CLIPPY_THRESHOLDS[$key]}([[:space:]]|#|$)" clippy.toml; then
      fail "clippy.toml: ${key} must read exactly '= ${CLIPPY_THRESHOLDS[$key]}'"
      note "changed, respelled, or deleted — each re-enables a weaker bound;"
      note "moving it is a decision with an ADR (rust-style.md rule 7), and"
      note "the pin in scripts/check-drift.sh moves in the same commit"
      clippy_violations=$((clippy_violations + 1))
    fi
  done
  # Any threshold-shaped key the map does not know — a list nobody grows
  # quietly. ⚠️ Unpinned by a negative case (only the value check has one);
  # stated here rather than implied covered.
  while IFS= read -r line; do
    key="${line%%=*}"; key="${key//[[:space:]]/}"
    if [[ -z "${CLIPPY_THRESHOLDS[$key]+x}" ]]; then
      fail "clippy.toml: $key is not in check-drift.sh's pin map"
      note "add it to CLIPPY_THRESHOLDS in the same commit"
      clippy_violations=$((clippy_violations + 1))
    fi
  done < <(grep -E '^[a-z-]+[[:space:]]*=' clippy.toml || true)
  if (( clippy_violations == 0 )); then
    ok "clippy.toml thresholds match the pin map (${#CLIPPY_THRESHOLDS[@]} pinned)"
  fi
fi

# ── Rust bounds: a `pub const` this project treats as a threshold ───────────
#
# ⚠️ **`m0-complete.sh`'s `NFR_CONSTANTS` cannot see these** (`M3.36`). It
# resolves a shell `NAME=` assignment, and a Rust `pub const NAME: T = value;`
# matches nothing it greps — so `M3` added eight bounds that decide how much
# object storage one request may cost, and *not one* was pinned by value
# anywhere. Non-negotiable 2 says a threshold is a constant no environment can
# move; the half this closes is that it must also be a constant no *edit* moves
# without a gate noticing.
#
# ⚠️ **Here rather than in `m0-complete.sh`, and that is the point.** That gate
# is a milestone boundary, invoked by neither the pre-commit hook nor
# `gates.yml` — this header's own second limitation. A bound moved on the
# commit path would land green and be caught whenever somebody next ran a
# milestone gate. This script is on the commit path.
#
# ⚠️ **The direction that weakens differs per row**, so each row says which and
# there is no blanket rule. A first draft asserted one — "every one of these is
# a ceiling, so raising is the weakening move" — and review found it wrong for
# three of eight: raising `TAIL_WINDOW_ENTRIES` keeps *more* entries in the
# priceable tail tier and makes a read *cheaper*, `COMMIT_QUEUE_DEPTH` is a
# channel capacity rather than a per-request cost, and raising
# `MAX_METADATA_STALENESS_MS` makes requests cheaper while widening hazard H4's
# window. This is the generalisation this file's own header warns against, two
# hundred lines up, where two of `m0-complete.sh`'s seven rows were wrong across
# two drafts for the same reason.
declare -A RUST_BOUNDS=(
  # ⚠️ `M10.18a`: the type widening that added `f32|f64|Duration` to the
  # candidate scan found this one on the first run. `object_store`'s backoff
  # multiplier, kept equal to `RetryPolicy`'s own doubling by the comment
  # beside it -- neither direction is safe: raising it makes the vendor retry
  # faster than the policy's own retry-budget arithmetic assumes, and
  # lowering it makes the two curves diverge the other way. A change here is
  # a decision about `ADR-0008`'s translation, not a tuning knob.
  ["crates/oqueue-store/src/retry.rs|BACKOFF_BASE"]="2.0"
  # How many records a compacted object is written to hold -- the denominator
  # read amplification is measured against (`M5.1`, `ADR-0036`). Raising it
  # raises every partition's measured amplification, so it is a threshold in
  # both directions: raising makes compaction fire on ranges that are already
  # fine and lowering makes it never fire. ⚠️ UNDERIVED, and the doc comment
  # says so -- `M14` replaces it with a measurement.
  ["crates/oqueue-compact/src/read_amp.rs|COMPACTED_OBJECT_RECORDS"]="524_288"
  # The amplification a range must strictly exceed before compaction plans it
  # (`M5.2`, `ADR-0036`). ⚠️ Weakening is *lowering*: a lower threshold plans
  # ranges that were fine, spending PUTs and rewriting acknowledged data for
  # no measured read benefit. Raising it is a different failure -- reads stay
  # amplified -- which is why it is pinned rather than bounded on one side.
  # UNDERIVED: `M5.md` calls the 8-16 band synthesis, and `M14` replaces it.
  ["crates/oqueue-compact/src/plan.rs|COMPACTION_READ_AMP_THRESHOLD"]="12"
  # The most records one compaction plan may rewrite (`M5.3`). ⚠️ Weakening is
  # *raising*: the budget exists so a plan cannot grow past what one round can
  # finish, and a bigger ceiling is a longer window in which a half-run merge
  # could lose acknowledged data (NFR-20). Lowering it only defers more work.
  # ⚠️ One compacted object's worth (`M5.4` lowered it from eight): the merge
  # writes one object, so a larger budget lets a plan be costed at two outputs
  # and run as one. `M5.6`'s multipart writer is what earns the raise.
  ["crates/oqueue-compact/src/cost.rs|COMPACTION_PLAN_RECORDS_BUDGET"]="524_288"
  # How often compaction looks for candidates (`M5.40`, `ADR-0036` decision 1).
  # ⚠️ Weakening is *lowering*: a shorter interval repeats every candidate's
  # window walk more often for a detection latency nothing has measured as too
  # long, and the ADR names that as the alternative to revisit first rather
  # than a knob.
  # Raising it is a different failure -- amplified ranges sit longer -- so it is
  # pinned rather than bounded on one side.
  ["crates/oqueue-compact/src/sweep.rs|COMPACTION_SWEEP_INTERVAL"]="Duration::from_mins(30)"
  # How many object-storage reads one parked `Fetch` may make. Raising it lets
  # a client's read volume be set by somebody else's write rate.
  ["crates/oqueue-broker/src/fetch/target.rs|MAX_READS_PER_REQUEST"]="4"
  # How many of a request's object reads may fail before it stops asking.
  # Raising it lets a frame's read rate against a sick store rise with the
  # fan-out of the client's own subscription.
  ["crates/oqueue-broker/src/read.rs|MAX_FAILED_FETCHES_PER_REQUEST"]="2"
  # What one response may pull off the store, whatever `max_bytes` says.
  ["crates/oqueue-broker/src/fetch/partition.rs|READ_BUDGET_BYTES"]="1024 * 1024"
  # The longest a fetch may park. ⚠️ Must stay under whatever
  # `ConnectionLimits::idle_timeout` a composer picks, so raising it is
  # weakening in a second way: it can cross that ceiling silently.
  ["crates/oqueue-broker/src/fetch/deadline.rs|MAX_PARK_MS"]="60_000"
  # The longest a JoinGroup round may stay open. `M4.7`'s own ceiling on a
  # member's rebalance_timeout_ms, fetch::deadline::MAX_PARK_MS's own
  # argument applied to a second wire field: raising it holds every member's
  # connection that much longer against a client's own (bounded, but large)
  # number.
  ["crates/oqueue-broker/src/join_group/deadline.rs|MAX_BARRIER_MS"]="3_000_000"
  # The longest a follower's own SyncGroup may wait for the leader's.
  # `M4.8`'s own ceiling, not derived from any client-supplied value
  # (SyncGroupRequest carries no timeout field) -- raising it holds a
  # follower's connection open that much longer against a leader that
  # never submits.
  ["crates/oqueue-broker/src/sync_group/deadline.rs|MAX_SYNC_WAIT_MS"]="3_000_000"
  # ⚠️ A floor, so lowering is the weakening direction -- it is what stops a
  # member asking for nothing from answering every other member's follower at
  # once (`M4.66`). Its value is `heartbeat::deadline::MIN_SESSION_TIMEOUT_MS`
  # below, and the two must move together or not at all.
  ["crates/oqueue-broker/src/sync_group/deadline.rs|MIN_SYNC_WAIT_MS"]="6_000"
  # ⚠️ A ceiling on what one member contributes to a group's sync fold, so
  # raising it is the weakening direction — it is what stops a single
  # `JoinGroup` pinning every later follower of that group at
  # `MAX_SYNC_WAIT_MS` for the life of the process (`M4.76`). Its value is
  # `heartbeat::deadline::MAX_SESSION_TIMEOUT_MS` below, tied by a
  # `const _: () = assert!(..)`, and the two must move together or not at all.
  ["crates/oqueue-broker/src/sync_group/deadline.rs|MAX_MEMBER_SYNC_WAIT_MS"]="1_800_000"
  # How short/long a session timeout this broker honours from a Heartbeat
  # member, whatever it asks for -- real Kafka's own
  # group.min/max.session.timeout.ms defaults. Lowering the minimum lets a
  # slow GC pause or scheduling hiccup evict a healthy member; raising the
  # maximum lets a stuck member hold its own seat that much longer.
  ["crates/oqueue-broker/src/heartbeat/deadline.rs|MIN_SESSION_TIMEOUT_MS"]="6_000"
  ["crates/oqueue-broker/src/heartbeat/deadline.rs|MAX_SESSION_TIMEOUT_MS"]="1_800_000"
  # `M4.14`: how many `GroupMetadataLog` entries `CommittedOffsets::open`
  # reads per page while replaying. Lowering it means more round trips to
  # replay the same log (a real cost once `M6` gives the log a real,
  # network-crossing engine); raising it means more memory held per page.
  ["crates/oqueue-broker/src/offset_commit.rs|REPLAY_PAGE_SIZE"]="256"
  # `M4.14`: how many version conflicts `CommittedOffsets::commit` retries
  # against a racing writer before giving up. Lowering it makes a
  # well-formed commit fail under ordinary concurrent contention it would
  # otherwise have won by retrying; raising it risks a longer hold on one
  # request under pathological contention.
  ["crates/oqueue-broker/src/offset_commit.rs|MAX_COMMIT_RETRIES"]="8"
  # `M4.15a`, retimed by `M4.17`: how many times `Cluster::wait_until_replayed`
  # polls before panicking rather than spinning forever. ⚠️ **The bound is
  # this count times `REPLAY_WAIT_POLL` below**, ~2 s; neither number means
  # anything alone. It was 10_000 *yields*, which is no bound in time at all
  # on a `multi_thread` runtime — under gate load ten thousand of them elapse
  # in milliseconds without the replay task being scheduled, which is how
  # `api_versions_round_trips_through_a_connection` failed 8 of 25 runs under
  # CPU saturation and 0 of 300 in isolation. Lowering the product risks a
  # false failure on a legitimately slow replay; raising it delays noticing a
  # truly stuck gate and costs `check-mutants` wall clock, which waits the
  # product out for every mutant that stalls replay (~93 s at a 10 s ceiling,
  # ~72 s at 2 s, ~40 s before). ⚠️ That is a wall-clock trade and **not** an
  # NFR-56 question: `check-budget.sh` removes every gate over 5 s from the
  # total it judges, so `check-mutants` is invisible to it at any of these
  # durations — `M4.21`'s row records the same.
  ["crates/oqueue-broker/src/cluster/replay.rs|MAX_REPLAY_WAIT_POLLS"]="2_000"
  # `M4.17`: how long each of those polls waits. A sleep rather than a
  # `yield_now`, so the loop is bounded in time; still instant under
  # `start_paused = true`, since tokio auto-advances only once every task is
  # idle and a runnable replay task therefore runs first.
  ["crates/oqueue-broker/src/cluster/replay.rs|REPLAY_WAIT_POLL"]="Duration::from_millis(1)"
  # `M4.15d`: how many times `GroupJoins::join` re-plans after losing a race
  # to another joiner's own in-flight transition. ⚠️ **A retry here re-plans
  # and never re-applies** — the pass that loses the race writes nothing — so
  # this bounds how often one task may be overtaken, not how many durable
  # records it can produce. Lowering it refuses a well-formed join under
  # ordinary concurrent membership churn, which is the normal case for a
  # group whose consumers all start at once; raising it risks a longer hold
  # on one `JoinGroup` under pathological contention. `MAX_COMMIT_RETRIES`'s
  # own reasoning, its own constant rather than shared code.
  ["crates/oqueue-broker/src/join_group/round/state.rs|MAX_JOIN_REPLANS"]="8"
  # `M4.15d`: the longest a deadline-driven close waits for whoever already
  # holds the group's own in-flight slot. ⚠️ A liveness bound, not a tuning
  # knob — it exists so a slot that is never released cannot hang a request
  # forever. The wait it bounds is a durable object-store append behind one
  # actor: tens of milliseconds at best, seconds under contention. Lowering
  # it makes a waiter give up on a close that was genuinely coming and answer
  # `REBALANCE_IN_PROGRESS` — retriable, so the client rejoins, but a wasted
  # round trip; raising it lengthens how long one request can be held.
  ["crates/oqueue-broker/src/join_group/round/state.rs|SLOT_WAIT"]="Duration::from_secs(5)"
  # `M4.29`: how long a round with no previous roster waits before closing on
  # whoever has arrived — Kafka's own `group.initial.rebalance.delay.ms`,
  # which defaults to the same 3 s. ⚠️ **Without it a brand-new group holds
  # its first joiner for the client's whole `rebalance_timeout_ms`**, which
  # both librdkafka and the Java consumer seed from `max.poll.interval.ms`:
  # 300 s by default, measured against real librdkafka rather than derived.
  # The same `None` covers a restarted broker, since `GroupJoins` is
  # per-node. Raising it delays every first assignment; lowering it makes a
  # fleet starting together more likely to need a second rebalance, because
  # members arriving after it has elapsed join the next round instead.
  ["crates/oqueue-broker/src/join_group/round/state.rs|INITIAL_REBALANCE_DELAY"]="Duration::from_secs(3)"
  # `M4.18`: how long a TLS handshake may take before the connection is
  # dropped. ⚠️ **The one hold that precedes every other limit** —
  # `ConnectionLimits` governs a session that exists, this governs getting one
  # at all, and until it completes the peer has authenticated nothing. Raising
  # it lengthens how long an unauthenticated peer can hold a socket and a task
  # (measured unbounded before this: still open at 145 s, where the cleartext
  # path is closed at 120 s by `idle_timeout`); lowering it risks refusing a
  # real client on a slow link.
  ["bin/oqueue/src/serve.rs|HANDSHAKE_TIMEOUT"]="Duration::from_secs(10)"
  # `M4.15c`: how many `GroupMetadataLog` entries `GroupTransitionsTask::replay`
  # reads per page — `CommittedOffsets::replay`'s own identical reasoning
  # (`REPLAY_PAGE_SIZE` below), a separate constant rather than shared code.
  ["crates/oqueue-broker/src/group_transitions.rs|REPLAY_PAGE_SIZE"]="256"
  # `M4.15c`: how many version conflicts `append_durably` retries against a
  # racing writer (offset commits and group transitions share one log)
  # before giving up — `MAX_COMMIT_RETRIES`'s own identical reasoning, its
  # own constant rather than shared code (`FaultGroupMetadataLog`'s own
  # "two small things, no shared code" precedent).
  ["crates/oqueue-broker/src/group_transitions.rs|MAX_APPEND_RETRIES"]="8"
  # How many batches one index page may name, and how many entries a partition
  # keeps in the cheap tier. Both bound work on paths NFR-2 and NFR-3 bound.
  # `M10.20`: moved from `index_state.rs` to its own `index_state/page.rs`
  # when `Page` (which this bounds) got its own file, the natural cut from
  # the fold beside it.
  ["crates/oqueue-core/src/index_state/page.rs|MAX_BATCHES_PER_PAGE"]="64"
  ["crates/oqueue-core/src/index_state.rs|TAIL_WINDOW_ENTRIES"]="128"
  # How many commits may queue behind the single serialization point. Raising
  # it turns a durable engine's commit latency into seconds of queueing against
  # NFR-1's 500 ms p99.
  ["crates/oqueue-coordinator/src/coordinator.rs|COMMIT_QUEUE_DEPTH"]="1024"
  # `ADR-0021`'s staleness limit: how long an agent may serve from a cache
  # nothing has arrived for. Raising it widens hazard H4's window.
  ["crates/oqueue-core/src/staleness.rs|MAX_METADATA_STALENESS_MS"]="5_000"
  # `M11.8`, `ADR-0031` point 6: how many distinct producer lines one
  # shard's allocator tracks before evicting the least recently touched.
  # ⚠️ **Weakens by lowering** — the opposite of most rows here — since a
  # smaller cap shortens the window a retry has before its producer's state
  # might be gone.
  ["crates/oqueue-coordinator/src/allocator/expiry.rs|MAX_TRACKED_PRODUCERS"]="100_000"
  # ⚠️ **Three more, found by this row's own review.** Each says in its own doc
  # comment that it is "a constant rather than a knob, per non-negotiable 2",
  # and each was pinned by nothing: measured, all three raised a thousandfold
  # in one edit and this gate printed `ok`.
  # How much of a log one replay folds at a time. ⚠️ **Weakens by rising** —
  # `applier.rs` calls it the bound on replay after a crash, so it is `M6`'s
  # RTO with another name.
  ["crates/oqueue-index/src/applier.rs|APPLY_BATCH_ENTRIES"]="1_000"
  # How many deltas a follower may fall behind before it is dropped. ⚠️
  # **Weakens by rising**: it is per-shard memory under NFR-11, and structurally
  # the same kind of queue depth as `COMMIT_QUEUE_DEPTH` above.
  ["crates/oqueue-coordinator/src/subscribe.rs|DELTA_BUFFER_ENTRIES"]="1_024"
  # How much of the log one rebuild page holds. ⚠️ **Weakens by rising** — it
  # is sized to bound the memory one page costs, on the path a dropped cache
  # takes back to service.
  ["crates/oqueue-coordinator/src/serve.rs|REBUILD_PAGE_ENTRIES"]="1024"
)
# ⚠️ **`M10.18a`: no more file-wide exemptions.** This used to be
# `NOT_A_BOUND_FILE`, a *file* on the left of `=`, and `M3.42`'s finding was
# exact: a file exempted because its one constant is a protocol sentinel
# stays exempted the moment a second, genuine threshold is added to it,
# because the check below only ever asked "is this file exempt", never "is
# this constant". `produce.rs` is the row's own example -- one sentinel
# today, and nothing would have stopped a retry count or a buffer size
# shipping beside it unreviewed. Every entry below is now `file|CONST_NAME`,
# the same key shape a file that holds both real bounds and sentinels
# already used -- there is no longer a second, coarser mechanism to fall
# back to.
#
# ⚠️ **Still measured before writing, on the old file-wide map's own
# precedent**: deleting every row below and running the scan is what
# enumerated exactly which constants exist in each of these files, rather
# than trusting the old file-level comment's word for "one constant" --
# `oqueue-codec/src/error_codes.rs` alone has 14.
declare -A NOT_A_BOUND=(
  ["crates/oqueue-broker/src/fetch/partition.rs|OFFSET_UNSET"]="the protocol's unset-offset sentinel"
  ["crates/oqueue-broker/src/find_coordinator.rs|GROUP"]="the wire schema's own key_type value for a consumer-group lookup, fixed by the protocol"
  # `M10.20`: moved from `records.rs` to its own `records/count.rs` when
  # `count_records` (the only reader of this constant) got its own file.
  ["crates/oqueue-codec/src/records/count.rs|MIN_RECORD_BODY_LEN"]="the shortest body the record format can express -- derived from the fields, not chosen, so it moves only if the format does"
  ["crates/oqueue-core/src/bundle.rs|BUNDLE_FORMAT_VERSION"]="this object format's version number"
  ["crates/oqueue-core/src/bundle.rs|TRAILER_LEN"]="the trailer's own width, fixed by the format"
  ["crates/oqueue-core/src/bundle.rs|MAX_TOPIC_NAME_LEN"]="what the footer's u16 name-length field can express, not a policy"
  # --- error_codes.rs: Kafka's own error codes, the protocol fixes every value
  ["crates/oqueue-codec/src/error_codes.rs|NONE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|OFFSET_OUT_OF_RANGE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|CORRUPT_MESSAGE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNKNOWN_TOPIC_OR_PARTITION"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|INVALID_REQUIRED_ACKS"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNSUPPORTED_VERSION"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNSUPPORTED_FOR_MESSAGE_FORMAT"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|INVALID_RECORD"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNKNOWN_TOPIC_ID"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|NOT_ENOUGH_REPLICAS"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|LEADER_NOT_AVAILABLE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNKNOWN_SERVER_ERROR"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|OFFSET_NOT_AVAILABLE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNSUPPORTED_COMPRESSION_TYPE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|INVALID_REQUEST"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|OUT_OF_ORDER_SEQUENCE_NUMBER"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|DUPLICATE_SEQUENCE_NUMBER"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|INVALID_PRODUCER_EPOCH"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNSUPPORTED_SASL_MECHANISM"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|SASL_AUTHENTICATION_FAILED"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|TOPIC_AUTHORIZATION_FAILED"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|INCONSISTENT_GROUP_PROTOCOL"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|REBALANCE_IN_PROGRESS"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|UNKNOWN_MEMBER_ID"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|ILLEGAL_GENERATION"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|NOT_COORDINATOR"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|COORDINATOR_NOT_AVAILABLE"]="Kafka's own error code, the protocol fixes it"
  ["crates/oqueue-codec/src/error_codes.rs|COORDINATOR_LOAD_IN_PROGRESS"]="Kafka's own error code, the protocol fixes it"
  # --- batch.rs: RecordBatch v2's fixed layout
  ["crates/oqueue-codec/src/batch.rs|BATCH_HEADER_LEN"]="RecordBatch v2's fixed header length"
  ["crates/oqueue-codec/src/batch.rs|CRC_COVERAGE_START"]="RecordBatch v2's fixed CRC coverage offset"
  ["crates/oqueue-codec/src/batch.rs|CRC_OFFSET"]="RecordBatch v2's fixed CRC field offset"
  ["crates/oqueue-codec/src/batch.rs|MAGIC_V2"]="the format's magic byte, fixed by the version it names"
  # --- listoffsets.rs (codec): the protocol's timestamp sentinels
  ["crates/oqueue-codec/src/listoffsets.rs|LATEST_TIMESTAMP"]="the protocol's latest-timestamp sentinel"
  ["crates/oqueue-codec/src/listoffsets.rs|EARLIEST_TIMESTAMP"]="the protocol's earliest-timestamp sentinel"
  ["crates/oqueue-codec/src/metadata.rs|AUTHORIZED_OPERATIONS_OMITTED"]="the protocol's authorized-operations sentinel"
  ["crates/oqueue-codec/src/produce.rs|LOG_APPEND_TIME_UNSET"]="the protocol's log-append-time sentinel"
  # --- varint.rs: fixed by the encoding
  ["crates/oqueue-codec/src/varint.rs|MAX_VARINT_BYTES"]="the widest legal varint, fixed by the encoding"
  ["crates/oqueue-codec/src/varint.rs|MAX_VARLONG_BYTES"]="the widest legal varlong, fixed by the encoding"
  # --- key_layout.rs: FNV-1a's own constants
  ["crates/oqueue-core/src/key_layout.rs|OFFSET_BASIS"]="FNV-1a's basis, fixed by the algorithm"
  ["crates/oqueue-core/src/key_layout.rs|PRIME"]="FNV-1a's prime, fixed by the algorithm"
  # --- oqueue-broker/src/listoffsets.rs: a sentinel, and a test's own version literal
  ["crates/oqueue-broker/src/listoffsets.rs|UNSET"]="a protocol sentinel"
  ["crates/oqueue-broker/src/listoffsets/tests.rs|VERSION"]="a test's own advertised-version literal -- M9.12 split listoffsets.rs's inline mod tests to listoffsets/tests.rs, moving this constant with it; not a production bound"
  ["crates/oqueue-broker/src/init_producer_id.rs|VERSION"]="a test's own advertised-version literal, inside #[cfg(test)] mod tests -- same shape as listoffsets.rs's own entry above"
  ["crates/oqueue-broker/src/sasl_handshake/tests.rs|VERSION"]="a test's own advertised-version literal, split into its own tests.rs file at the 500-line boundary's own precedent -- same shape as listoffsets.rs's own entry above"
  ["crates/oqueue-broker/src/sasl_authenticate/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/find_coordinator/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/find_coordinator/tests.rs|BATCHED_VERSION"]="a test's own advertised-version literal for the KIP-699 batched shape (M4.4), same shape as VERSION above"
  ["crates/oqueue-broker/src/join_group/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/join_group/tests.rs|REBALANCE_TIMEOUT_MS"]="a test's own fixture value (how long the T1 acceptance test waits under paused time), not a production bound"
  ["crates/oqueue-broker/src/join_group/tests.rs|V7"]="a test's own advertised-version literal, same shape as VERSION above"
  ["crates/oqueue-broker/src/sync_group/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/sync_group/tests.rs|V5"]="a test's own advertised-version literal, same shape as VERSION above"
  # ⚠️ Visible to this gate only since `M4.66`, which moved the `JoinGroup`
  # helpers up out of `sync_group/tests/reopened.rs`: the skip below is for a
  # path containing `/tests/`, and `tests.rs` is not one. A wire version a
  # test encodes at, not a threshold this project chose.
  ["crates/oqueue-broker/src/sync_group/tests.rs|JOIN_VERSION"]="a test's own advertised-version literal, same shape as VERSION above"
  ["crates/oqueue-broker/src/heartbeat/tests.rs|JOIN_VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/heartbeat/tests.rs|SYNC_VERSION"]="a test's own advertised-version literal, same shape as JOIN_VERSION above"
  ["crates/oqueue-broker/src/heartbeat/tests.rs|HEARTBEAT_VERSION"]="a test's own advertised-version literal, same shape as JOIN_VERSION above"
  ["crates/oqueue-broker/src/heartbeat/tests.rs|REBALANCE_TIMEOUT_MS"]="a test's own fixture value (how long the acceptance test's own join round waits under paused time), not a production bound"
  ["crates/oqueue-broker/src/leave_group/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/leave_group/tests.rs|V1"]="a test's own advertised-version literal, same shape as VERSION above"
  ["crates/oqueue-broker/src/offset_commit/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/offset_fetch/tests.rs|VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/cluster/tests.rs|HEARTBEAT_VERSION"]="a test's own advertised-version literal, same shape as sasl_handshake/tests.rs's own entry above"
  ["crates/oqueue-broker/src/cluster/tests.rs|OFFSET_FETCH_VERSION"]="a test's own advertised-version literal, same shape as HEARTBEAT_VERSION above"
  ["crates/oqueue-broker/src/produce/answer.rs|UNASSIGNED"]="the unassigned-offset sentinel a refusal answers with"
  ["crates/oqueue-coordinator/src/commit.rs|UNASSIGNED_OFFSET"]="the unassigned-offset sentinel, beside the type that returns it"
)
rust_violations=0
# ⚠️ **Every numeric constant in the tree is a candidate**, so a new bound
# cannot ship unpinned and unmentioned. Tests are excluded: a fixture's step
# count is nobody's threshold, and requiring a reason for each would make this
# list noise.
#
# ⚠️ **`Duration|f32|f64` added by `M10.18a`.** The type list was integers
# only, so `oqueue-store/src/retry.rs`'s `BACKOFF_BASE: f64 = 2.0` -- a real
# tuning constant -- was invisible to this scan the whole time it existed,
# found only once the type list was widened to look. `Duration` closes the
# other half: a bare `const X: Duration = ...` now counts as a candidate the
# same way an integer one does.
while IFS= read -r decl; do
  file="${decl%%:*}"
  name="${decl##*:}"
  [[ "$file" == *"/tests/"* || "$file" == *"/fuzz/"* ]] && continue
  [[ -n "${NOT_A_BOUND[$file|$name]:-}" ]] && continue
  [[ -n "${RUST_BOUNDS[$file|$name]+x}" ]] && continue
  fail "$file: const $name is neither pinned in RUST_BOUNDS nor recorded as not a bound"
  note "a numeric constant this project chose is a threshold; add its value, or say why it is not one"
  rust_violations=$((rust_violations + 1))
done < <(git ls-files '*.rs' 2>/dev/null | xargs grep -HE \
  "^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?const [A-Z][A-Z0-9_]*[[:space:]]*:[[:space:]]*(u8|u16|u32|u64|usize|i8|i16|i32|i64|isize|f32|f64|Duration|std::time::Duration|core::time::Duration)[[:space:]]*=" 2>/dev/null \
  | sed -E 's|^([^:]+):.*const ([A-Z0-9_]+).*|\1:\2|' | sort -u || true)
for key in "${!RUST_BOUNDS[@]}"; do
  file="${key%%|*}"
  name="${key##*|}"
  want="${RUST_BOUNDS[$key]}"
  if [[ ! -f "$file" ]]; then
    fail "$file does not exist, so $name cannot be pinned"
    note "a bound whose file moved is a bound nothing is holding"
    rust_violations=$((rust_violations + 1))
    continue
  fi
  # ⚠️ Tolerates `pub`, `pub(crate)` and a leading doc-comment-free blank: what
  # is asserted is the *assignment*, not the visibility, because a bound that
  # became private is still a bound.
  mapfile -t decls < <(grep -E "^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?const[[:space:]]+${name}[[:space:]]*:" "$file" 2>/dev/null || true)
  # ⚠️ **More than one declaration is a failure, not a first-wins pick.** A
  # `#[cfg(feature = "x")]` pair compiles, reads as an ordinary edit, and lets
  # the value that is actually compiled differ from the one pinned — measured:
  # a first draft took `head -1` and passed while the real constant was
  # sixteen times the pinned number.
  if (( ${#decls[@]} > 1 )); then
    fail "$file declares const $name ${#decls[@]} times, so which one is pinned is not decidable"
    note "a cfg-gated pair is the reachable shape; pin the value in one place"
    rust_violations=$((rust_violations + 1))
    continue
  fi
  line="${decls[0]:-}"
  if [[ -z "$line" ]]; then
    fail "$file does not declare a const $name"
    note "renaming a bound is fine; renaming it without moving this entry is not"
    rust_violations=$((rust_violations + 1))
    continue
  fi
  got="${line#*=}"
  got="${got%%;*}"
  got="${got%%//*}"
  got="$(sed -E 's/^[[:space:]]+|[[:space:]]+$//g' <<< "$got")"
  if [[ "$got" != "$want" ]]; then
    fail "$file: $name is '$got'; check-drift.sh holds '$want'"
    note "the weakening direction is per row -- read this entry's own comment, not a rule of thumb (non-negotiable 2)"
    note "changing it means changing this entry in the same commit, which is a diff someone reviews"
    rust_violations=$((rust_violations + 1))
  fi
done
if (( rust_violations == 0 )); then
  ok "Rust bounds match the pin map (${#RUST_BOUNDS[@]} pinned)"
fi

# ── Struct-literal fields: `SERVE_LIMITS.idle_timeout`'s own gap ───────────
#
# ⚠️ **`M10.18a`'s named example, and widening the type list above does not
# reach it.** `const SERVE_LIMITS: ConnectionLimits = ConnectionLimits {
# idle_timeout: ..., ... };` declares a *struct*-typed const, so no entry in
# the scalar type alternation above will ever match its opening line — the
# threshold is not the const's own type, it is a field three lines inside the
# literal. `MAX_PARK_MS`'s own pin row says the file's `idle_timeout` must
# stay above it, and until this section existed nothing connected that
# sentence to a gate a reader could run.
#
# ⚠️ **A second, narrower scan rather than a general parser** — this file's
# own header already says why: "closing that gap needs a real parser and is
# not this task's job." What this catches is the shape this workspace
# actually writes: a `const NAME: Type = Type {` opening line, one field per
# line until a bare `};` closes it. A struct literal written any other way
# (nested braces, multiple fields per line) is outside what this can see, and
# that limit is a fact about this section rather than a promise about every
# shape Rust allows.
# ⚠️ **Empty today, deliberately kept rather than omitted.** `SERVE_LIMITS`'s
# other two fields, `max_frame` and `max_in_flight`, do not match
# `THRESHOLD_RE` at all -- they never reach this map, so an entry for either
# would claim a check happened where the field-name filter above already
# decided the question. The map exists for the day a struct field's name
# *does* look like a threshold and genuinely is not one.
declare -A NOT_A_STRUCT_FIELD_BOUND=()
declare -A STRUCT_FIELD_BOUNDS=(
  # ⚠️ **`idle_timeout`, not exempted.** This is the row's own example: a
  # `Duration` field whose value must stay above `MAX_PARK_MS`
  # (`crates/oqueue-broker/src/fetch/deadline.rs`'s own pinned `60_000`).
  # Raising it is safe; lowering it below that ceiling reintroduces the
  # defect `the_idle_timeout_outlasts_the_longest_park` exists to catch --
  # this pin is what makes that relationship visible to `check-drift.sh`
  # itself, not only to a `cargo test` run.
  # ⚠️ Spelled `Duration::from_mins(2)` since `M4.18`, not because the bound
  # moved — it is the same two minutes — but because that commit imported
  # `Duration` into `serve.rs` and clippy's `unused_qualifications` then
  # refused the fully-qualified form. The pin tracks the bytes, so a spelling
  # change has to be recorded here even when the value does not move.
  ["bin/oqueue/src/serve.rs|SERVE_LIMITS|idle_timeout"]="Duration::from_mins(2)"
)
struct_violations=0
while IFS= read -r open_line; do
  file="${open_line%%:*}"
  rest="${open_line#*:}"
  const_name="$(sed -E 's/^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?const ([A-Z][A-Z0-9_]*).*/\3/' <<< "$rest")"
  [[ "$file" == *"/tests/"* || "$file" == *"/fuzz/"* ]] && continue
  # Everything from the opening `{` to the first bare `};` after it.
  body="$(awk -v start="$const_name" '
    $0 ~ ("const " start ":") { found=1 }
    found { print }
    found && /^[[:space:]]*};[[:space:]]*$/ { exit }
  ' "$file" 2>/dev/null || true)"
  while IFS= read -r field_line; do
    field="$(sed -E 's/^[[:space:]]*([a-z_][a-z0-9_]*):.*/\1/' <<< "$field_line")"
    [[ "$field" == "$field_line" ]] && continue # no field: value shape on this line
    printf '%s\n' "$field" | grep -qEi "$THRESHOLD_RE" || continue
    key="$file|$const_name|$field"
    [[ -n "${NOT_A_STRUCT_FIELD_BOUND[$key]:-}" ]] && continue
    [[ -n "${STRUCT_FIELD_BOUNDS[$key]+x}" ]] && continue
    fail "$file: const $const_name field '$field' looks like a threshold and is neither pinned nor recorded as not a bound"
    note "a struct-literal const can hide a threshold behind a type alternation cannot see -- name it here"
    struct_violations=$((struct_violations + 1))
  done <<< "$body"
done < <(git ls-files '*.rs' 2>/dev/null | xargs grep -HE \
  "^[[:space:]]*(pub(\([a-z]+\))?[[:space:]]+)?const [A-Z][A-Z0-9_]*[[:space:]]*:[[:space:]]*[A-Za-z][A-Za-z0-9_:]*[[:space:]]*=.*\{[[:space:]]*\$" 2>/dev/null || true)
for key in "${!STRUCT_FIELD_BOUNDS[@]}"; do
  file="${key%%|*}"
  rest="${key#*|}"
  const_name="${rest%%|*}"
  field="${rest##*|}"
  want="${STRUCT_FIELD_BOUNDS[$key]}"
  if [[ ! -f "$file" ]]; then
    fail "$file does not exist, so $const_name.$field cannot be pinned"
    struct_violations=$((struct_violations + 1))
    continue
  fi
  got="$(sed -nE "/const ${const_name}:/,/^[[:space:]]*\};/p" "$file" \
    | grep -E "^[[:space:]]*${field}[[:space:]]*:" | head -1 || true)"
  got="${got#*:}"
  got="${got%%,*}"
  got="$(sed -E 's/^[[:space:]]+|[[:space:]]+$//g' <<< "$got")"
  if [[ -z "$got" ]]; then
    fail "$file does not declare a field '$field' on const $const_name"
    note "renaming a field is fine; renaming it without moving this entry is not"
    struct_violations=$((struct_violations + 1))
  elif [[ "$got" != "$want" ]]; then
    fail "$file: $const_name.$field is '$got'; check-drift.sh holds '$want'"
    note "changing it means changing this entry in the same commit, which is a diff someone reviews"
    struct_violations=$((struct_violations + 1))
  fi
done
if (( struct_violations == 0 )); then
  ok "struct-literal bounds match the pin map (${#STRUCT_FIELD_BOUNDS[@]} pinned)"
fi

finish
