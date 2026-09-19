#!/usr/bin/env bash
# No library crate names a concrete socket type, reads the real clock, or
# touches object storage outside the seams that exist to hold them. `M-1.8`.
#
#   scripts/check-sans-io.sh
#
# Non-negotiable 5, and `architecture.md`'s "Sans-I/O" section: "a concrete
# socket type named inside a library crate is a violation; a generic bound is
# not." This is a grep gate against **five** patterns — four project-wide-ish,
# and a fifth (`M5.38`, widened by `M5.47`) that runs against one crate's
# planning half. ⚠️ **Three are
# literal-identifier matches** where a false positive is rare — unlike
# `check-drift.sh`'s same-line heuristic, a concrete type name like
# `TcpStream` cannot also be a generic bound — and so is the fifth, the
# trigger's store-seam name, which is why a doc comment in `read_amp.rs` may
# not write it. ⚠️ **The fourth is not**:
# `REAL_CLOCK_RE` (`M10.5`) spans to a semicolon so it catches a braced import,
# which means it also matches the same text in a doc comment. It runs on every
# `oqueue-broker` file — `src/` and `tests/` alike, because `ADR-0027` point 3
# puts the harness that composes a broker in the second — with named
# exemptions. A prose false positive is real, and the remedy the message
# offers, a whole-file exemption, is a blunt one: a last resort rather than a
# first reach.
#
# ## The five patterns: three project-wide except two named crates, one that
# ## runs in exactly one crate, and one that runs against one crate's planning
# ## half
#
# - **A concrete socket type.** `TcpStream`/`TcpListener`/`UdpSocket`/
#   `UnixStream`/`UnixListener`, from `std::net`, `tokio::net`, or bare after a
#   `use`. The I/O shell is generic over `AsyncRead + AsyncWrite + Unpin` and
#   lives entirely in `oqueue-broker`, so that crate is exempt from this
#   pattern. Nothing else is: `bin/oqueue` is not scanned at all (it is a
#   binary composition root, not a library crate, and non-negotiable 5 names
#   only library crates).
# - **A real clock read.** `Instant::now()`, `SystemTime::now()`,
#   `Utc::now()`, `Local::now()`, `OffsetDateTime::now_utc()`. `Clock` is the
#   `pub trait` seam in `oqueue-core`; its real implementation is the one
#   place these calls belong, and that implementation lives in `oqueue-broker`
#   per the same reasoning as the socket exemption — so `oqueue-broker` is
#   exempt from this pattern too. ⚠️ **And `tokio::time::Instant::now()` is not
#   a real clock read** (`M10.6`): under a paused runtime it is virtual, so
#   that exact token is deleted from a line before the pattern runs — **in a
#   `tests/` tree only**, because production never pauses. The rest of the line
#   stays in scope, which is what stops
#   `tokio::time::sleep(SystemTime::now().elapsed()..)` from walking through.
# - **A clock a seeded run cannot advance** (`M10.5`), which applies to
#   `oqueue-broker` *because* it is exempt from the pattern above. The broker
#   is the I/O shell and legitimately holds timers; what it may not hold is a
#   clock `ADR-0028`'s paused-time runtime cannot control. `tokio::time`'s
#   reads are virtual there — measured, a 600 s timeout completing in 1.3 µs of
#   wall clock — so they are not flagged. Flagged: any `std::time::` path or
#   import naming `Instant` or `SystemTime` (braced and aliased forms
#   included), a bare `SystemTime::now()`, chrono's `Utc::now()` and
#   `Local::now()`, and the `time` crate's `OffsetDateTime::now_utc()`.
#   ⚠️ **Two files are exempt by name**,
#   `writer_id.rs`, whose own doc says why, and `tests/it/virtual_time.rs`,
#   which measures the premise this rule rests on and therefore must read the
#   wall clock to do it.
# - **An object-storage SDK call.** `aws_sdk_s3`, `aws_config`,
#   `google_cloud_storage`, `object_store::`, `opendal::` — ⚠️ ~~the leading
#   candidates named in `docs/researches/05` §1, pending the ADR M1's plan
#   still calls open~~. `ADR-0008` chose `object_store` (`M1.1`), so the list
#   is no longer a slate of candidates: `object_store::` is the one this
#   workspace actually imports and the others are kept so that reaching for a
#   provider SDK in a library crate still trips the gate. `oqueue-store` is the crate that implements
#   `ObjectStore` and is therefore exempt from this one pattern only; it is
#   still held to the socket and clock patterns above like every other
#   library crate, and `oqueue-broker` is exempt here too, for the same
#   reason as the other two.
#
# - **A store named in compaction's planning half.** No file under
#   `oqueue-compact/src` may name the store seam — the fake and the three
#   wrappers included, so the pattern is a substring rather than a word match
#   (`M5.38`) — **except** the executor's, which `EXECUTOR_FILES` names in
#   this script. ⚠️ **It was a rule about one file until `M5.47`**, and two
#   commits later `plan.rs` and `sweep.rs` were each claiming the property in
#   their own docs with nothing holding either: a rule scoped to one file is
#   one the next file does not inherit. The merge executor legitimately brings
#   a store into that crate, so it is listed rather than exempted by pattern,
#   and adding a file to that list is a diff with a reason. What may not have
#   one is everything evaluated per candidate partition per sweep (`ADR-0036`
#   decision 1). A moved directory, a sweep that finds no file, and an
#   exemption naming a file that does not exist each fail it,
#   because a rule that cannot read its own file is holding nothing.
#
# ## What this does not catch
#
# - **A generic bound named after the concrete type it will be instantiated
#   with**, if a future refactor ever aliases one — e.g. `type Sock =
#   TcpStream;` re-exported and then used generically elsewhere under the
#   alias `Sock`. The alias's own definition line still fails, which is where
#   the violation actually is.
# - **An object-storage SDK reached through a dependency's re-export under a
#   different path than the ones listed above.** ⚠️ ~~The candidate list is
#   fixed in the ADR doc 05 §1 has not yet settled; once M1's decision #3
#   lands, this list should be revisited against the crate actually chosen.~~
#   — that revisit is `M1.45`, and this is its result: `ADR-0008` landed at
#   `M1.1`, the crate is `object_store`, and the list is kept wider than the
#   one real import on purpose. A re-export under some other path is still
#   uncaught, which is the actual residue this bullet exists to name.
# - **`unsafe` or FFI calls that perform I/O without naming any of the five
#   patterns above.** Out of scope for this gate; `security.md` rule 18 and
#   `check-unsafe.sh` (M-1.11) are what bound `unsafe` to begin with.
# - **A store the compaction trigger reaches through a generic declared
#   elsewhere** (`M5.39`, naming what `M5.38` left unnamed here). The fifth
#   pattern forbids the store seam's name in the planning files; a
#   `Trigger<S>` declared in another module, with that file holding only a
#   method call on a bound it never spells, passes. A grep cannot see a type
#   it is not shown, and what closes it is a reviewer noticing the trigger
#   acquired a field.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# No `\b`: GNU and BSD grep disagree about it, the same portability reason
# `check-drift.sh` (M-1.7) already gives for avoiding it. A non-identifier
# character (or line start/end) on each side is the portable substitute.
SOCKET_RE='(^|[^A-Za-z0-9_])(TcpStream|TcpListener|UdpSocket|UnixStream|UnixListener)([^A-Za-z0-9_]|$)'
CLOCK_RE='(Instant::now\(\)|SystemTime::now\(\)|Utc::now\(\)|Local::now\(\)|OffsetDateTime::now_utc\(\))'
STORE_RE='(aws_sdk_s3|aws_config|google_cloud_storage|object_store::|opendal::)'

# ⚠️ **The clock the *harness* cannot control**, which is a narrower thing than
# `CLOCK_RE` and is why it is a second pattern rather than a widening of the
# first. `M10.4` makes every seeded run a paused-time runtime, so
# `tokio::time`'s `Instant::now`, `sleep_until` and `timeout` are virtual —
# measured, a 600 s timeout completes in 1.3 us of wall clock — and flagging
# them would be flagging determinism. What breaks a replay is a clock tokio
# does not own.
#
# ⚠️ **`std::time::Instant` anywhere, including a `use` line**, and the first
# version of this matched only the fully-qualified `std::time::Instant::now()`
# — a form essentially nobody writes. The realistic one is
# `use std::time::Instant;` followed by a bare `Instant::now()`, which walked
# straight through, so the alternative this pattern added over `CLOCK_RE` was
# close to dead text. A text scan cannot tell a bare `Instant::now()` apart
# from `tokio::time`'s, but it does not have to: the *import* is unambiguous,
# and a broker file has no other reason to name `std::time::Instant`.
# ⚠️ **Three spans, because two earlier versions each missed the spelling a
# real author would write.** The first matched only the fully-qualified
# `std::time::Instant::now()`; the second added the braced
# `use std::time::{Duration, Instant};`, which is one comma from what `park.rs`
# and `session.rs` already write and is `writer_id.rs`'s own idiom; and the
# third adds the *nested* `use std::{time::Instant, ..}` — what rustfmt emits
# under `imports_granularity=Crate` — and the module import `use std::time;`
# with or without an alias, which is the commoner of those two and escaped a
# version that covered only the aliased form.
# Every one of them was found by running the gate against the spelling, not by
# reading the regex.
# ⚠️ `std::time::Duration` alone is untouched — a duration reads no clock.
REAL_CLOCK_RE='(std::time::[^;]*(Instant|SystemTime)|std::\{[^;]*time::[^;]*(Instant|SystemTime)|std::time( as [A-Za-z_]+)? *;|SystemTime::now\(\)|Utc::now\(\)|Local::now\(\)|OffsetDateTime::now_utc\(\))'

# Named exceptions, not patterns -- see the header.
BROKER_DIR="crates/oqueue-broker"
STORE_DIR="crates/oqueue-store"

# ⚠️ **One file, named, and its own doc says why** (`M10.5`). `writer_id.rs`
# mints an identity from the wall clock *deliberately*: a faked clock is shared
# by every broker a test builds, so minting through the `Clock` seam would
# collide two `WriterId`s in exactly the configuration written to prove them
# distinct. ⚠️ **`ADR-0028` states the consequence rather than hiding it** — two
# runs of one seed produce different object keys, so a replay claim is over the
# schedule and not over key names.
# ⚠️ **A list, because the failure message tells an author to add to it.** It
# was a scalar compared with `!=`, so a second entry made the string match
# neither path and started scanning the first file again — the documented
# remedy making things worse.
REAL_CLOCK_EXEMPT=(
  # `M6.13`: NFR-22 is a recovery *time*, so its measurement reads the wall
  # clock by definition; a virtual clock would time nothing (`ADR-0048`).
  "crates/oqueue-broker/tests/it/rto.rs"
  "crates/oqueue-broker/src/writer_id.rs"
  # `M11.4`: `InitProducerId`'s minting path, on `WriterId::mint`'s own
  # exemption above -- a faked clock is shared by every broker a test
  # builds, which would make two identities collide exactly where a test
  # means to prove they do not.
  "crates/oqueue-broker/src/init_producer_id.rs"
  # ⚠️ The test that measures the premise the rule rests on: proving a 600 s
  # virtual timeout costs no wall clock *is* a wall-clock measurement.
  "crates/oqueue-broker/tests/it/virtual_time.rs"
)

mapfile -t files < <(git ls-files -- 'crates/*.rs' 2>/dev/null)

if (( ${#files[@]} == 0 )); then
  skip "sans-I/O (no crate sources tracked yet)"
  finish
fi

violations=0
real_clock_violations=0
scanned=0

real_clock_exempt() {
  local f="$1" e
  for e in "${REAL_CLOCK_EXEMPT[@]}"; do
    [[ "$f" == "$e" ]] && return 0
  done
  return 1
}

report() {
  local kind="$1" f="$2" lineno="$3" content="$4"
  content="$(printf '%s' "$content" | sed -E 's/^[[:space:]]+//')"
  fail "$kind in a library crate: $f:$lineno"
  note "$content"
  violations=$((violations + 1))
}

# ⚠️ **Its own reporter, because the sans-I/O remedy is wrong for it.** A
# `REAL_CLOCK_RE` hit is *already* in `oqueue-broker`, which is the I/O shell
# and is exempt from non-negotiable 5's clock rule — telling its author to
# "move the code to oqueue-broker" names the crate the file is in, and
# "inject `ObjectStore`" names something unrelated. The actionable fix is to
# use a clock a seeded run can advance.
scan_real_clock() {
  local f="$1" matches
  matches="$(grep -nE "$REAL_CLOCK_RE" "$f" 2>/dev/null || true)"
  [[ -n "$matches" ]] || return 0
  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    local content="${m#*:}"
    content="$(printf '%s' "$content" | sed -E 's/^[[:space:]]+//')"
    fail "a clock no seeded run can control: $f:${m%%:*}"
    note "$content"
    note "use \`tokio::time\` (virtual under a paused runtime) or oqueue-core's \`Clock\`"
    note "if it genuinely must be the wall clock, name the file in REAL_CLOCK_EXEMPT and say why"
    real_clock_violations=$((real_clock_violations + 1))
  done <<< "$matches"
}

# ⚠️ **`tokio::time::Instant::now()` is not a real clock read**, and `CLOCK_RE`
# cannot say so on its own: it matches the bare `Instant::now()`, which is the
# same text. Under `ADR-0028`'s paused runtime a `tokio::time` read is virtual
# — `M10.5` measured it — so flagging one is flagging determinism, and `M10.6`
# hit exactly that writing a test that reads the simulated clock.
# ⚠️ **The token is deleted, not the line.** A first version filtered out any
# line containing `tokio::time::`, which suppressed every other alternative on
# it — `tokio::time::sleep(std::time::SystemTime::now().elapsed()...)`, a
# library crate deriving a sleep from the wall clock, walked straight through
# a gate it had been failing. Removing just the virtual read and matching what
# is left keeps the rest of the line in scope.
#
# ⚠️ **The bare form too, when a `use` brings it into scope** (`M10.27`).
# Deleting only `tokio::time::Instant::now()` leaves `use tokio::time::Instant;`
# plus a bare `Instant::now()` flagged under a remedy ("inject `Clock`") that
# does not apply to a construct `ADR-0028`'s paused runtime already makes
# deterministic — the same gap `REAL_CLOCK_RE`'s own header describes for the
# `std::time::Instant` side, mirrored here rather than re-discovered. The
# import regex covers the same three spans that one names: a plain `use`, a
# braced list, and a nested `use tokio::{time::Instant, ..}` — ⚠️ **checked by
# running it against all three rather than by reading it**, because `M5.39`
# broke two of them while leaving this sentence saying otherwise.
# ⚠️ **No `\b` here either** (`M5.39`). This file's own header rules it out for
# GNU/BSD disagreement, and these two were the residue: on a grep that ignores
# it the strip below is never appended, a virtual `tokio::time` read is flagged
# as a real clock, and that is exactly the false positive `M10.27` removed.
#
# ⚠️ **The class is optional, and that is the whole difficulty.** `\b` is
# zero-width; `[^A-Za-z0-9_]` consumes a character, so the obvious substitution
# `tokio::time::[^;]*(^|[^A-Za-z0-9_])Instant` cannot match the plain
# `use tokio::time::Instant;` at all — after the literal prefix the next
# character is `I`, `[^;]*` must be empty, and there is nothing left for the
# class to eat. `M5.39`'s first attempt did exactly that and its own review
# caught it: two of the three spans stopped matching, so a plain `use` plus a
# bare `Instant::now()` under a paused runtime was flagged as a real clock read
# — the regression the replacement was written to prevent. `([^;]*[^A-Za-z0-9_])?`
# — an optional run ending in a non-identifier character — is the portable form
# that keeps all three spans and still rejects `Instants`.
TOKIO_INSTANT_USE_RE='(tokio::time::([^;]*[^A-Za-z0-9_])?Instant([^A-Za-z0-9_]|$)|tokio::\{[^;]*time::([^;]*[^A-Za-z0-9_])?Instant([^A-Za-z0-9_]|$))'
scan_clock() {
  local f="$1" matches lineno strip rest crate tail
  # ⚠️ **The exemption is for a `tests/` tree only, anchored at the crate
  # root** (`M10.27` narrowed this from `*/tests/*`, which matches `tests` as
  # *any* path component: `crates/oqueue-core/src/tests/foo.rs` — a nested
  # directory compiled into the shipped library unless `#[cfg(test)]`-gated —
  # took the same exemption a genuine integration-test tree gets. A paused
  # runtime is a *test* construct; production never pauses, so a
  # `tokio::time::Instant::now()` in a library crate's `src/` is a real clock
  # read and stays flagged. A first version applied the deletion everywhere,
  # which let a coordinator read the runtime clock in shipped code and pass —
  # a strict weakening of non-negotiable 5, for a problem whose only instance
  # was one test file.
  #
  # ⚠️ **Parameter expansion, not `[^/]*` in the glob.** The obvious-looking
  # `crates/[^/]*/tests/*` still matched `crates/oqueue-core/src/tests/x.rs`:
  # a bracket expression is exactly one character in glob syntax, and the
  # bare `*` right after it is its own, separately unrestricted token — so
  # `[^/]*` means "one non-slash character, then anything", not "a run of
  # non-slash characters", and the trailing `*` swallows the `/src` a reader
  # would expect the class to exclude. Found by testing the pattern directly
  # rather than trusting it read correctly. Stripping the one `crates/<crate>/`
  # component by parameter expansion instead has no such ambiguity to read.
  rest="${f#crates/}"
  crate="${rest%%/*}"
  tail="${rest#"$crate"/}"
  if [[ "$tail" == tests/* ]]; then
    strip='s/tokio::time::Instant::now\(\)//g'
    if grep -qE "$TOKIO_INSTANT_USE_RE" "$f" 2>/dev/null; then
      # ⚠️ Excluding a leading `:` as well, so `std::time::Instant::now()` is
      # not stripped by the rule that exists for the tokio one.
      strip="$strip"'; s/(^|[^A-Za-z0-9_:])Instant::now\(\)/\1/g'
    fi
    matches="$(sed -E "$strip" "$f" 2>/dev/null | grep -nE "$CLOCK_RE" || true)"
  else
    matches="$(grep -nE "$CLOCK_RE" "$f" 2>/dev/null || true)"
  fi
  [[ -n "$matches" ]] || return 0
  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    lineno="${m%%:*}"
    # ⚠️ The *file's* line, not the sed-mutated one: printing `let x = ;` sends
    # an author looking for a syntax error the file does not have.
    report "a real clock read" "$f" "$lineno" "$(sed -n "${lineno}p" "$f")"
  done <<< "$matches"
}

scan_pattern() {
  local re="$1" kind="$2" f="$3"
  local matches
  matches="$(grep -nE "$re" "$f" 2>/dev/null || true)"
  [[ -n "$matches" ]] || return 0
  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    report "$kind" "$f" "${m%%:*}" "${m#*:}"
  done <<< "$matches"
}

# ⚠️ **The fifth pattern: compaction's planning half may name no store at all**
# (`M5.38`, widened by `M5.47`). `read_amp` is evaluated for every candidate
# partition on every sweep, so one store call there is a per-partition cost at
# sweep cadence and `ADR-0036`'s whole cost argument is about not paying it.
# The four patterns above cannot hold that: `STORE_RE` looks for SDK paths, and
# the store this crate could reach is `oqueue_core::ObjectStore`, a `pub trait`
# the seam exists to make injectable. ⚠️ **Nor does the dependency set hold
# it** -- `oqueue-core` exports `ObjectStore` and `FakeObjectStore`, so
# depending on `oqueue-core` alone leaves a store one `use` away, and the merge
# executor legitimately brings one into this crate.
#
# ⚠️ **Scoped to the crate with the executor named as the exception, rather
# than to one file** (`M5.47`). `M5.38` wrote this leg for `read_amp.rs` alone,
# and two commits later `plan.rs` and `sweep.rs` were each asserting in their
# own docs that they cost no object-storage operation with nothing holding
# either -- the convention abandoned by the next task that used it, which is
# the shape `M5.38` exists to stop. A default of "no store" with a named list
# of files that may have one inverts that: a new planner file is covered the
# moment it exists, and a new *executor* file has to be added here, in a diff,
# with a reason.
PLANNING_DIR="crates/oqueue-compact/src"
# The executor: the half that reads and writes objects by design. Each entry is
# a path relative to the repository root.
EXECUTOR_FILES=(
  "crates/oqueue-compact/src/merge.rs"
  "crates/oqueue-compact/src/merge/outcome.rs"
  "crates/oqueue-compact/src/layout.rs"
  # ⚠️ `compose` writes a manifest and reads each component's footer, so it is
  # an executor for the same reason `merge` is -- and `M5.8` had to add it
  # here, in a diff, which is the property the named-exception form buys.
  "crates/oqueue-compact/src/compose.rs"
  # ⚠️ The object lifecycle deletes what the index stopped naming (`M5.21`),
  # so it names the store's delete by design; deciding *what* is due reads
  # only the index, and that half stays sans-I/O inside the same file.
  "crates/oqueue-compact/src/lifecycle.rs"
)
# Whether the workspace rooted here has a member package of this name, with
# every `members` pattern expanded. `scripts/lib/manifest.py` owns the parse,
# because a second reading of a manifest is a second thing to keep correct --
# `M1.32`'s own reason for that module.
#
# ⚠️ **Three-valued, like `scan_one_planning_file`'s grep and for the same
# reason.** 0 claims it, 1 does not, 2 could not tell -- and a caller that
# folded the third into the second would skip the whole leg whenever the
# manifest could not be read, which is the silent-skip this task exists to
# close, one level in. No `Cargo.toml` at all is **1**, not 2: a scratch tree
# with no workspace is the ordinary case `M5.57` restored, and failing there
# reds every converse fixture in `negative.sh`.
workspace_claims() {
  # ⚠️ Exported here, as `check-layering.sh` and `check-readmes.sh` do: a
  # heredoc has no `__file__` to resolve `scripts/lib` against, and an unset
  # variable would make python raise — which this function would read as "the
  # workspace does not claim it" and silently skip the whole leg. The failure
  # this task exists to close, one level in.
  export OQUEUE_SCRIPTS_DIR="$REPO_ROOT/scripts"
  python3 - "$1" <<'MANIFEST_PY'
import os
import sys

try:
    # ⚠️ **Inside the `try`, all of it.** The import and the environment lookup
    # sat above it until `M5.58`'s second round, so `ModuleNotFoundError` from
    # a missing `scripts/lib/manifest.py` and `KeyError` from an unset
    # `OQUEUE_SCRIPTS_DIR` never reached the handler: python exited 1, the
    # caller read "the workspace does not claim it", and the leg skipped a
    # violating tree in silence. Measured — with the module removed, the gate
    # printed ok and exited 0 on a planner naming a store. Reachable from
    # `negative.sh` itself, whose `copy_gate` copies the lib with `|| true`.
    sys.path.insert(0, os.environ["OQUEUE_SCRIPTS_DIR"] + "/lib")
    from manifest import workspace_claims

    if not os.path.isfile("Cargo.toml"):
        sys.exit(1)
    sys.exit(0 if workspace_claims(".", sys.argv[1]) else 1)
except SystemExit:
    raise
except BaseException as why:  # noqa: BLE001
    # ⚠️ **Every exception, not `OSError`.** A non-UTF-8 manifest raises
    # `UnicodeDecodeError`, which is a `ValueError`; an absolute member pattern
    # raises `NotImplementedError` from `Path.glob`; a missing module raises
    # `ModuleNotFoundError`. None is an `OSError`, so all three exited 1 and
    # the caller read "the workspace does not claim it" — the silent skip this
    # task closes, reinstated inside the fix twice.
    #
    # ⚠️ `KeyboardInterrupt` lands here too and becomes a 2, which fails the
    # gate loudly rather than skipping it. That is the safe direction: an
    # interrupted check is not a passed one.
    print(f"cannot read the workspace manifest: {why!r}", file=sys.stderr)
    sys.exit(2)
MANIFEST_PY
}

# ⚠️ **A substring, deliberately, and no `\b`.** Four types in `oqueue-core`
# end in this name -- the seam itself, the fake beside it, and the chunked,
# merging and counting wrappers -- so a word-boundary match on the bare
# identifier reads green while `FakeObjectStore` sits in the signature, which
# is the exact type this rule's own row names as the reachable one. And this
# file's header forbids `\b` outright, because GNU and BSD grep disagree about
# it: a pattern that silently matches nothing is the fail-open this leg was
# written against.
TRIGGER_RE='ObjectStore'
trigger_violations=0
planning_scanned=0

is_executor() {
  local candidate="$1" known
  for known in "${EXECUTOR_FILES[@]}"; do
    [[ "$candidate" == "$known" ]] && return 0
  done
  return 1
}

# ⚠️ **`grep`'s status is three-valued and this reads all three.** 0 matched,
# 1 clean, 2+ could not read -- and an unreadable file answering "clean" is the
# class `M4.50`, `M4.68` and `M5.37` each found in a gate that wrote
# `|| true`. Here the third value fails, because a rule that cannot read its
# own file is holding nothing.
scan_one_planning_file() {
  local file="$1" matches status
  matches="$(grep -nE "$TRIGGER_RE" "$file")" && status=0 || status=$?
  if (( status >= 2 )); then
    fail "cannot read $file (grep exit $status)"
    note "an unreadable file is not a clean one"
    trigger_violations=$((trigger_violations + 1))
    return 0
  fi
  (( status == 1 )) && return 0
  while IFS= read -r m; do
    [[ -n "$m" ]] || continue
    fail "compaction's planning half names a store: $file:${m%%:*}"
    note "$(printf '%s' "${m#*:}" | sed -E 's/^[[:space:]]+//')"
    note "planning is evaluated per candidate partition per sweep -- ADR-0036 decision 1"
    note "a store belongs in the executor; if this file IS the executor, add it"
    note "to EXECUTOR_FILES in this script, in the same commit, with a reason"
    trigger_violations=$((trigger_violations + 1))
  done <<< "$matches"
}

scan_trigger() {
  local file found=0
  # ⚠️ **The directory must exist and must hold files.** An empty sweep is how
  # a gate reports success while checking nothing -- the failure class this
  # whole script's header names -- so zero planning files is a failure, not a
  # clean run.
  # ⚠️ **Absent crate, absent rule — but only when the workspace agrees it is
  # absent** (`M5.57` correcting `M5.47`). The first version failed outright on
  # a missing directory, reasoning that a rule whose directory moved is a rule
  # nobody is keeping. That is right for *this* repository and wrong for every
  # other tree this script runs in: `tests/gates/negative.sh` builds scratch
  # repositories holding one crate and this gate, and all of them started
  # failing — including the converse cases that exist to prove the gate
  # *allows* something, which is how a false positive hides. So the question is
  # not "does the directory exist" but "does the workspace claim this crate",
  # and only the mismatch between those two fails.
  # ⚠️ **The resolved member list, not the manifest's text** (`M5.58`). The
  # grep this replaces was switched off by `members = ["crates/*"]` — an
  # ordinary tidy-up nobody associates with this gate — and a violating tree
  # then exited 0 with no line about the planning leg at all. A gate a manifest
  # edit can turn off is the shape `M10.24` found in a hook and `M5.47` found
  # in a scope.
  local claimed=0
  workspace_claims oqueue-compact || claimed=$?
  if (( claimed >= 2 )); then
    fail "cannot tell whether the workspace claims oqueue-compact"
    note "a manifest this gate cannot read is not a workspace without that crate"
    trigger_violations=$((trigger_violations + 1))
    return 0
  fi
  if (( claimed == 1 )); then
    return 0
  fi
  if [[ ! -d "$PLANNING_DIR" ]]; then
    fail "$PLANNING_DIR does not exist, but the workspace names that crate"
    note "a rule whose directory moved is a rule nobody is keeping -- point it at the new path"
    trigger_violations=$((trigger_violations + 1))
    return 0
  fi
  while IFS= read -r file; do
    [[ -n "$file" ]] || continue
    is_executor "$file" && continue
    found=$((found + 1))
    scan_one_planning_file "$file"
  done < <(find "$PLANNING_DIR" -name '*.rs' -type f | sort)
  planning_scanned="$found"
  if (( found == 0 )); then
    fail "no planning files under $PLANNING_DIR, so this leg checked nothing"
    note "every .rs file there is listed as an executor, or the crate moved"
    trigger_violations=$((trigger_violations + 1))
  fi
  # ⚠️ **An executor entry that names no file is a stale exemption**, and it
  # would exempt nothing while reading as though it does -- the same fail-open
  # in the other direction.
  for file in "${EXECUTOR_FILES[@]}"; do
    [[ -f "$file" ]] && continue
    fail "EXECUTOR_FILES names $file, which does not exist"
    note "an exemption for an absent file exempts nothing and hides a moved one"
    trigger_violations=$((trigger_violations + 1))
  done
}

for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  scanned=$((scanned + 1))

  if [[ "$f" != "$BROKER_DIR"/* ]]; then
    scan_pattern "$SOCKET_RE" "a concrete socket type" "$f"
    scan_clock "$f"
    if [[ "$f" != "$STORE_DIR"/* ]]; then
      scan_pattern "$STORE_RE" "an object-storage SDK call" "$f"
    fi
  elif [[ "$f" == "$BROKER_DIR"/* ]] && ! real_clock_exempt "$f"; then
    # ⚠️ **The broker is exempt from `CLOCK_RE` and not from this one.** It is
    # the I/O shell, so it legitimately holds timers; what it may not hold is a
    # clock a seeded run cannot advance, because that is what stops a schedule
    # replaying (`M10.md`'s first risk).
    #
    # ⚠️ **Every broker file, `tests/` included, with named exemptions.** A
    # `src/`-only split was the wrong axis: `ADR-0027` point 3 puts the harness
    # that composes a broker in `tests/`, so that is the half a seeded run
    # actually executes — while the broker's *unit* tests live in
    # `#[cfg(test)]` modules inside `src/` and were scanned either way.
    scan_real_clock "$f"
  fi
done

scan_trigger

if (( violations == 0 && real_clock_violations == 0 && trigger_violations == 0 )); then
  ok "no library crate touches a socket, the real clock, or object storage ($scanned file(s) scanned)"
  if (( planning_scanned > 0 )); then
    ok "compaction's planning half names no store ($planning_scanned file(s), ${#EXECUTOR_FILES[@]} executor file(s) excepted)"
  fi
else
  if (( violations > 0 )); then
    note "business logic is sans-I/O -- non-negotiable 5"
    note "inject Clock/ObjectStore, or move the code to oqueue-broker/oqueue-store"
  fi
  if (( real_clock_violations > 0 )); then
    note "a seeded run replays only if every clock it reads is one it can advance -- ADR-0028"
  fi
  if (( trigger_violations > 0 )); then
    note "the trigger's cost is what ADR-0036 decision 1 buys -- M5.38"
  fi
fi

finish
