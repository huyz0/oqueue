#!/usr/bin/env bash
# No library crate names a concrete socket type, reads the real clock, or
# touches object storage outside the seams that exist to hold them. `M-1.8`.
#
#   scripts/check-sans-io.sh
#
# Non-negotiable 5, and `architecture.md`'s "Sans-I/O" section: "a concrete
# socket type named inside a library crate is a violation; a generic bound is
# not." This is a grep gate against **four** patterns. ⚠️ **Three are
# literal-identifier matches** where a false positive is rare — unlike
# `check-drift.sh`'s same-line heuristic, a concrete type name like
# `TcpStream` cannot also be a generic bound. ⚠️ **The fourth is not**:
# `REAL_CLOCK_RE` (`M10.5`) spans to a semicolon so it catches a braced import,
# which means it also matches the same text in a doc comment. It runs on every
# `oqueue-broker` file — `src/` and `tests/` alike, because `ADR-0027` point 3
# puts the harness that composes a broker in the second — with named
# exemptions. A prose false positive is real, and the remedy the message
# offers, a whole-file exemption, is a blunt one: a last resort rather than a
# first reach.
#
# ## The four patterns: three project-wide except two named crates, and one
# ## that runs in exactly one crate
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
#   exempt from this pattern too.
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
# - **`unsafe` or FFI calls that perform I/O without naming any of the three
#   patterns above.** Out of scope for this gate; `security.md` rule 18 and
#   `check-unsafe.sh` (M-1.11) are what bound `unsafe` to begin with.
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
  "crates/oqueue-broker/src/writer_id.rs"
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

for f in "${files[@]}"; do
  [[ -f "$f" ]] || continue
  scanned=$((scanned + 1))

  if [[ "$f" != "$BROKER_DIR"/* ]]; then
    scan_pattern "$SOCKET_RE" "a concrete socket type" "$f"
    scan_pattern "$CLOCK_RE" "a real clock read" "$f"
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

if (( violations == 0 && real_clock_violations == 0 )); then
  ok "no library crate touches a socket, the real clock, or object storage ($scanned file(s) scanned)"
else
  if (( violations > 0 )); then
    note "business logic is sans-I/O -- non-negotiable 5"
    note "inject Clock/ObjectStore, or move the code to oqueue-broker/oqueue-store"
  fi
  if (( real_clock_violations > 0 )); then
    note "a seeded run replays only if every clock it reads is one it can advance -- ADR-0028"
  fi
fi

finish
