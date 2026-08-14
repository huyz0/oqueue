#!/usr/bin/env bash
# No library crate names a concrete socket type, reads the real clock, or
# touches object storage outside the seams that exist to hold them. `M-1.8`.
#
#   scripts/check-sans-io.sh
#
# Non-negotiable 5, and `architecture.md`'s "Sans-I/O" section: "a concrete
# socket type named inside a library crate is a violation; a generic bound is
# not." This is a grep gate against three literal-identifier patterns, each
# unambiguous enough that a false positive is rare — unlike `check-drift.sh`'s
# same-line heuristic, a concrete type name like `TcpStream` cannot also be a
# generic bound; the two are syntactically different things, not two readings
# of the same text.
#
# ## The three patterns, and why each is checked project-wide except two
# ## named crates
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
# - **An object-storage SDK call.** `aws_sdk_s3`, `aws_config`,
#   `google_cloud_storage`, `object_store::`, `opendal::` — the leading
#   candidates named in `docs/researches/05` §1, pending the ADR M1's plan
#   still calls open. `oqueue-store` is the crate that implements
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
#   different path than the ones listed above.** The candidate list is fixed
#   in the ADR doc 05 §1 has not yet settled; once M1's decision #3 lands,
#   this list should be revisited against the crate actually chosen.
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

# Named exceptions, not patterns -- see the header.
BROKER_DIR="crates/oqueue-broker"
STORE_DIR="crates/oqueue-store"

mapfile -t files < <(git ls-files -- 'crates/*.rs' 2>/dev/null)

if (( ${#files[@]} == 0 )); then
  skip "sans-I/O (no crate sources tracked yet)"
  finish
fi

violations=0
scanned=0

report() {
  local kind="$1" f="$2" lineno="$3" content="$4"
  content="$(printf '%s' "$content" | sed -E 's/^[[:space:]]+//')"
  fail "$kind in a library crate: $f:$lineno"
  note "$content"
  violations=$((violations + 1))
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
  fi
done

if (( violations == 0 )); then
  ok "no library crate touches a socket, the real clock, or object storage ($scanned file(s) scanned)"
else
  note "business logic is sans-I/O -- non-negotiable 5"
  note "inject Clock/ObjectStore, or move the code to oqueue-broker/oqueue-store"
fi

finish
