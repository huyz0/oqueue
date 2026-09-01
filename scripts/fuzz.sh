#!/usr/bin/env bash
# Every decoder's fuzz target, run to its corpus — security.md rule 5 and
# testing.md rule 24, deferred into M2 by M0's checkpoint review and
# discharged by M2.26.
#
# ## What a run means
#
# Each target under crates/oqueue-codec/fuzz/fuzz_targets/ runs for a
# bounded number of executions, seeded from the checked-in corpus — which
# includes the real librdkafka frames the golden corpus captured, so the
# fuzzer starts from bytes a real client actually sent rather than from
# nothing. A crash, a sanitizer report, or a leak fails the run and leaves
# the reproducing input under fuzz/artifacts/<target>/ for replay with
# `cargo +nightly fuzz run <target> <artifact>`.
#
# ⚠️ Bounded, not exhaustive: RUNS_PER_TARGET below is a smoke depth chosen
# so the whole script stays in the nightly tier's minutes, not a claim of
# coverage. Raising it is always safe; the direction that would weaken this
# gate is lowering it (non-negotiable 2).
#
# ## Tooling
#
# Needs a nightly toolchain and cargo-fuzz (libFuzzer). Both missing-tool
# cases are skips with remedies, lib.sh's contract — but a skip here means
# NO fuzzing ran, and the nightly tier should treat that as its own alarm.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

FUZZ_DIR="crates/oqueue-codec/fuzz"
# ⚠️ A constant, deliberately not an environment override: this is a gate
# threshold, and a threshold an environment can move is non-negotiable 2's
# canonical violation (check-drift.sh's own worked example is exactly the
# `${VAR:-default}` shape). Raising it is an edit under review like any
# other; lowering it is the weakening direction.
RUNS_PER_TARGET=100000

if ! has_rust; then
  skip "fuzzing (no Rust workspace here)"
  finish
fi
require_tool cargo "install the Rust toolchain (rustup.rs)" || finish
if ! command -v cargo-fuzz >/dev/null 2>&1; then
  skip "fuzzing (cargo-fuzz not installed; cargo install cargo-fuzz)"
  finish
fi
if ! rustup toolchain list 2>/dev/null | grep -q '^nightly'; then
  skip "fuzzing (no nightly toolchain; rustup toolchain install nightly)"
  finish
fi

# Every target the fuzz crate declares, discovered rather than listed here
# — a hardcoded list is how a sixth decoder ships without a target while
# this script stays green. The check below closes the other direction.
mapfile -t targets < <(find "$FUZZ_DIR/fuzz_targets" -name '*.rs' -exec basename {} .rs \; | sort)
if (( ${#targets[@]} == 0 )); then
  fail "no fuzz targets found under $FUZZ_DIR/fuzz_targets"
  finish
fi

# ⚠️ Every module in a crate that holds decoders must have a fuzz target or a
# recorded reason not to — derived from the source tree, so the check FAILS
# CLOSED: a new module without a target turns this red rather than staying
# invisible.
#
# ⚠️ **Two crates, not one** (`M3.28`). It scanned `oqueue-codec` only, on the
# assumption that a decoder is a *wire* decoder — and `parse_footer` is a
# decoder over bytes an object store returned, in `oqueue-core`, invisible to
# the gate that exists to notice exactly that. `security.md` rule 3 does not
# distinguish: nothing reachable from stored bytes may panic either, and `M5`
# rewrites the objects `M3` wrote.
DECODER_CRATES=(oqueue-codec oqueue-core)
# ⚠️ **`M10.18`: every workspace crate is triaged, not just the two that
# happened to be decoders when this list was written.** `DECODER_CRATES` used
# to be the whole of it — a crate that started parsing untrusted bytes and was
# never added here was invisible to this gate, with no failure and no note.
# `NOT_A_DECODER_CRATE` is the other half of the same enumeration
# `NOT_A_PARSER` already does per-module: every crate this workspace declares
# must be in exactly one of the two, checked against `Cargo.toml`'s own member
# list below rather than a second hand-written count that could itself drift.
declare -A NOT_A_DECODER_CRATE=(
  [oqueue-buf]="refcounted slices and pooling; moves bytes, does not parse their structure"
  [oqueue-checksum]="CRC-32C over caller-provided bytes -- a fold with one code path per input length, not a decoder with a shape to get wrong"
  [oqueue-index]="MemoryIndex and its applier fold typed MetadataRecord values this process built or a MetadataLog handed it -- no byte-level decoder yet. ⚠️ testing.md rule 24 names this crate as the *next* target once index search over corrupt blobs exists; this entry is why that addition will not be silently missed -- see the check below"
  [oqueue-store]="S3/GCS backends over object_store; the only parsing here is ObjectStorePath::parse on keys this process constructed, not bytes a remote party sent -- the HTTP response parsing is object_store's own, outside this workspace"
  [oqueue-coordinator]="sequencing and recovery over typed MetadataRecord values; MetadataLog is a trait with only in-memory fakes today, so there is no wire form to decode"
  [oqueue-crypto]="AEAD and envelope encryption scaffolding; no decrypt/decode path is wired to anything yet (grep confirms zero decrypt/from_bytes/parse calls)"
  [oqueue-compact]="compaction planning arithmetic over sizes and counts this process already holds"
  [oqueue-broker]="the I/O shell. Its one byte-level read is connection.rs's 4-byte frame-length prefix (i32::from_be_bytes, which cannot panic) followed by an explicit bound check against max_frame *before* the body is allocated -- security.md rules 1-2's discipline, already in place. Everything past that bound is oqueue-codec's own decoders, covered by the request target"
  [oqueue-testkit]="test harness and data generators; runs in tests, never on a production input path"
  [oqueue]="bin/oqueue's package name, not its directory name -- the composition root; every parse it needs is a downstream crate's"
)
# `Cargo.toml`'s own `members` list, so a workspace member this array does not
# yet know about is a mismatch the loop below reports rather than a crate
# neither array mentions and nobody notices.
#
# ⚠️ **`[a-z0-9_-]+`, not `[a-z-]+`.** Review measured the narrower class
# silently drops any member whose directory name carries a digit or
# underscore -- `"crates/oqueue-v2"` matches neither alternative and produces
# zero output, so that crate never reaches `workspace_crates` and the triage
# loop never iterates over it. The gate would print its "every crate is
# triaged" `ok` having never considered the new one -- the exact
# invisible-crate failure this row exists to close, reintroduced one level up
# by the pattern meant to close it. Cargo crate-name characters are the full
# set here.
mapfile -t workspace_crates < <(
  grep -oE '"crates/[a-z0-9_-]+"|"bin/[a-z0-9_-]+"' "$REPO_ROOT/Cargo.toml" |
    tr -d '"' | sed 's#.*/##' | sort
)
for crate in "${workspace_crates[@]}"; do
  in_decoder=0
  for d in "${DECODER_CRATES[@]}"; do [[ "$d" == "$crate" ]] && in_decoder=1; done
  if (( in_decoder )); then
    continue
  fi
  if [[ -z "${NOT_A_DECODER_CRATE[$crate]:-}" ]]; then
    fail "workspace crate '$crate' is in neither DECODER_CRATES nor NOT_A_DECODER_CRATE"
    note "a new crate is unaccounted for until one of the two says why"
  fi
done
ok "every workspace crate is triaged as a decoder or has a stated reason it is not"
# ⚠️ **Every module, not a clever subset, and `M3.28` tried the subset first.**
# Its first version selected modules by grepping each file for a `pub fn`
# taking `&[u8]`, on the theory that a decoder is a function handed bytes it
# did not produce. Review measured it **unsound in the weakening direction**:
# `compress.rs`'s `decompress_records` has a rustfmt-wrapped signature so the
# name and the `&[u8]` sit on different lines, `varint`'s entry points take a
# `&mut Cursor<'_>`, and `wire.rs`'s takes `&'a [u8]` — so deleting the
# `compress` or `varint` target left this gate GREEN where the hand-written
# list had failed it. A heuristic that silently un-covers the
# decompression-bomb path is worse than a long list, and non-negotiable 2 is
# about that direction exactly.
#
# So: enumerate. Each entry is a judgement somebody made once and a reader can
# check, and what it buys is that a new module cannot be missed by a pattern
# nobody re-derived.
declare -A NOT_A_PARSER=(
  # --- oqueue-codec
  [oqueue-codec::lib]="the module root; declares, parses nothing"
  [oqueue-codec::wire]="Cursor primitives -- every target drives them transitively"
  [oqueue-codec::decode_error]="the refusals wire.rs raises and re-exports; an error type, reads no input"
  [oqueue-codec::versions]="a static advertised-versions table; parses nothing"
  [oqueue-codec::attributes]="a bitfield over an i16 the batch decoder already produced"
  [oqueue-codec::apikey]="an i16-to-enum lookup over a value frame.rs's decoder already extracted, not its own byte stream"
  [oqueue-codec::emit]="the byte writers wire.rs re-exports; nothing here reads untrusted input"
  [oqueue-codec::error_codes]="protocol constants; parses nothing"
  [oqueue-codec::apiversions]="encode-only by design -- the ApiVersions request body is informational and never decoded (see the module doc)"
  [oqueue-codec::fetch::tests]="M10.17's split-out test module for fetch.rs; #[cfg(test)] only, no production build carries it"
  [oqueue-codec::records::tests]="M10.19's split-out test module for records.rs; #[cfg(test)] only, no production build carries it"
  # --- oqueue-core. The object format's reader is bundle_footer, which has a
  # target; everything else is a newtype, a seam, a fake, or a policy.
  [oqueue-core::lib]="the module root; declares, parses nothing"
  [oqueue-core::bundle]="assembles a payload from records it never reads; bundle_footer.rs reads one back"
  [oqueue-core::bundle_name]="builds object keys from a writer identity; parses none"
  [oqueue-core::byte_range]="a validated (offset, length) pair; its constructor refuses, it does not decode"
  [oqueue-core::chunk]="splits an owned payload for multipart; reads no untrusted bytes"
  [oqueue-core::chunk::single_flight]="deduplicates concurrent gets for one key; the bytes pass through opaque"
  [oqueue-core::clock]="a time seam and its fake"
  [oqueue-core::commit_version]="a u64 newtype with checked arithmetic"
  [oqueue-core::coordinator_epoch]="a u64 newtype"
  [oqueue-core::error]="the crate's error enum"
  [oqueue-core::fault]="fault-injection config and a delay future; test support"
  [oqueue-core::fault_metadata_log]="a MetadataLog decorator for tests; delegates, parses nothing"
  [oqueue-core::index_reader]="a read-only view over a MaterializedIndex; forwards four methods"
  [oqueue-core::index_state]="folds MetadataEntry values this process built, never bytes off a wire"
  [oqueue-core::index_state::page]="M10.20's split-out read surface: pages IndexedBatch values the fold already produced, parses no bytes of its own"
  [oqueue-core::key]="key material types"
  [oqueue-core::key_layout]="object-key naming rules; builds strings, parses none"
  [oqueue-core::materialized_index]="a trait and its fake; the fold is over typed entries"
  [oqueue-core::merge]="coalesces byte ranges into fewer gets; no entry point takes bytes, so there is nothing to hand a fuzzer -- ⚠️ but get_many slices a store-returned buffer, and what stops that panicking is the backends' own truncated-range refusal in oqueue-store, not anything here"
  [oqueue-core::metadata_log]="a trait and its in-memory fake; entries are typed, not bytes"
  [oqueue-core::metadata_record]="the record enum the coordinator builds and the index folds; no byte form of its own"
  [oqueue-core::multipart]="part-size policy arithmetic"
  [oqueue-core::object_key]="a validated String newtype"
  [oqueue-core::object_meta]="size and etag a store reported"
  [oqueue-core::object_ref]="an index entry this process built"
  [oqueue-core::offset]="an i64 newtype with checked arithmetic"
  [oqueue-core::op_counts]="an operation counter and its store decorator"
  [oqueue-core::partition]="an i32 newtype"
  [oqueue-core::precondition]="a conditional-write mode enum"
  [oqueue-core::producer_epoch]="an i16 newtype with a non-negative invariant"
  [oqueue-core::producer_id]="an i64 newtype with a non-negative invariant"
  [oqueue-core::producer_identity]="groups an id, epoch and sequence this process already validated or decoded elsewhere; parses no bytes of its own"
  [oqueue-core::rate_governor]="admission arithmetic over counts"
  [oqueue-core::read_mode]="a read-mode enum"
  [oqueue-core::redacted]="a Debug wrapper that withholds"
  [oqueue-core::retry]="retry classification and backoff arithmetic"
  [oqueue-core::staleness]="cache-freshness policy over typed versions and epochs"
  [oqueue-core::store]="the ObjectStore trait and its fake; bytes pass through opaque"
  [oqueue-core::test_executor]="a cooperative executor for tests"
  [oqueue-core::topic]="a validated String newtype"
)
# ⚠️ Message-body decoders exercised through the `request` target's full
# dispatch path (header, `supports()` gate, this decoder, the handler) rather
# than a same-named standalone target — `request` is what found the M2.26 DoS
# in the first place, by driving exactly this path. Named individually, not a
# blanket exemption, so a new message module still fails closed until it is
# added here or grows its own target.
declare -A COVERED_BY_REQUEST=(
  [oqueue-codec::metadata]="request"
  [oqueue-codec::produce]="request"
  [oqueue-codec::fetch]="request"
  [oqueue-codec::listoffsets]="request"
)
# ⚠️ **Which crate each target speaks for** (`M3.28`), because a bare module
# name stopped being unique the moment a second crate was scanned: without
# this, an `oqueue-core::records` would be reported covered by the codec's
# `records` target, which drives a different decoder entirely.
#
# ⚠️ **A target missing from this map is its own failure**, not a silent
# mismatch: the target list is discovered and this map is written, so the two
# drift, and the drift's symptom is a message saying a target does not exist
# when it plainly does. Better to say which of the two is missing.
declare -A TARGET_CRATE=(
  [batch]="oqueue-codec"
  [compress]="oqueue-codec"
  [count_records]="oqueue-codec"
  [flex]="oqueue-codec"
  [frame]="oqueue-codec"
  [records]="oqueue-codec"
  [request]="oqueue-codec"
  [varint]="oqueue-codec"
  [bundle_footer]="oqueue-core"
)
# ⚠️ **`M10.18`: which module a target actually covers, when the bin name
# cannot say it.** The matching loop below used to compare a target's own name
# against the module path directly, which works only because every target so
# far covers a *top-level* module. A nested one -- `chunk::single_flight`, or
# `testing.md` rule 24's `oqueue-index` search-over-corrupt-blobs target,
# which would live under a `search` module -- has a `::` in its path, and a
# Cargo `[[bin]]` name cannot hold one; `cargo fuzz new` would refuse to
# create a target literally named `chunk::single_flight`. So a nested module
# could be *covered* by a real target and this script would still report it
# missing, because `$t == $module` can never be true when `$module` contains
# `::` and `$t` structurally cannot. Unset here means "covers the module named
# after the target itself", which is every target so far; an entry lets a
# future target's bin name differ from the module path it drives.
declare -A TARGET_MODULE=(
  # `M10.20`: `count_records` moved to its own `records::count` submodule in
  # the same commit that gave it this target, so the bin name and the module
  # path it covers differ from the start -- the exact case `M10.18`'s comment
  # above named as a future need, not a hypothetical one.
  [count_records]="records::count"
)
for t in "${targets[@]}"; do
  if [[ -z "${TARGET_CRATE[$t]:-}" ]]; then
    fail "fuzz target '$t' names no crate in TARGET_CRATE, so no module can be matched to it"
  fi
done
for crate in "${DECODER_CRATES[@]}"; do
  # ⚠️ **`find`, not a `*.rs` glob** (`M3.28` round 2). The glob was a
  # top-level *file* listing wearing the word "module": `chunk/single_flight.rs`
  # existed the whole time, in a scanned crate, with no target and no
  # allowlist row, and the scan passed in silence. `testing.md` rule 24's next
  # target is index search over corrupt blobs, which would land one directory
  # deep for the same reason `chunk` did.
  while IFS= read -r src; do
    rel="${src#"$REPO_ROOT/crates/$crate/src/"}"
    module="${rel%.rs}"
    # `chunk/single_flight` becomes `chunk::single_flight`, which is what it
    # is called in Rust and what an allowlist row can be read against.
    module="${module//\//::}"
    [[ -n "${NOT_A_PARSER[$crate::$module]:-}" ]] && continue
    [[ -n "${COVERED_BY_REQUEST[$crate::$module]:-}" ]] && continue
    found=0
    for t in "${targets[@]}"; do
      # `TARGET_MODULE[$t]:-$t`: unset means the target covers the module
      # named after itself -- every target today -- and set is the escape
      # hatch a nested module needs, since its own path cannot be the bin name.
      covers="${TARGET_MODULE[$t]:-$t}"
      [[ "$covers" == "$module" && "${TARGET_CRATE[$t]}" == "$crate" ]] && found=1
    done
    if (( ! found )); then
      fail "decoder module '$crate::$module' has no fuzz target and no allowlist reason"
    fi
  done < <(find "$REPO_ROOT/crates/$crate/src" -name '*.rs' | sort)
done

# Build first, separately: a target that does not compile is a build
# failure, not a crash, and the two need different operators. This is also
# the compile coverage the root workspace cannot give a crate deliberately
# outside it -- an oqueue-codec or oqueue-broker rename that breaks the
# targets turns THIS red rather than nothing.
mkdir -p target
if ! (cd "$FUZZ_DIR" && cargo +nightly fuzz build) > target/fuzz-build.log 2>&1; then
  fail "the fuzz targets do not build"
  note "$(tail -5 target/fuzz-build.log 2>/dev/null || true)"
  finish
fi
ok "all ${#targets[@]} fuzz targets build"

for target in "${targets[@]}"; do
  # Two corpus dirs: the first (scratch, gitignored) receives what the
  # fuzzer discovers; the second is the committed seed set -- the real
  # librdkafka frames among them -- which stays read-only and reviewed.
  mkdir -p "$FUZZ_DIR/corpus/$target"
  if (cd "$FUZZ_DIR" && cargo +nightly fuzz run "$target" \
      "corpus/$target" "seeds/$target" -- \
      -runs="$RUNS_PER_TARGET" -max_len=65536 -print_final_stats=0) \
      > "target/fuzz-$target.log" 2>&1; then
    ok "fuzz target '$target' survived $RUNS_PER_TARGET runs from its corpus"
  else
    fail "fuzz target '$target' crashed -- reproducer under $FUZZ_DIR/artifacts/$target/"
    note "$(tail -5 "target/fuzz-$target.log" 2>/dev/null || true)"
  fi
done

finish
