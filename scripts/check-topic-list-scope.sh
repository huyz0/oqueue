#!/usr/bin/env bash
# No code path materializes the global topic list to answer a client
# request. `M9.11`, on `m3-complete.sh`'s own zero-LIST precedent — structural,
# not a runtime assertion.
#
#   scripts/check-topic-list-scope.sh
#
# Doc 15 §4's own architectural claim, `M9.8`-`M9.10`'s own reason to exist:
# a `Metadata` request must cost O(topics this principal can see), never
# O(topics that exist). `M9.9`/`M9.10` are the *requirement*; this is what
# stops it from holding by convention alone — a handler added later that
# reaches for `Cluster::topic_names()` because it is the obvious way to "see
# every topic" would undo `M9.8`'s whole point, silently, on the one path
# most likely to go untested (`M9.10`'s own row said so first).
#
# ## Why this is a call-site count, not a trait-shape check
#
# `m3-complete.sh`'s zero-LIST proves its property because `ObjectStore`
# *cannot express* a list operation — no method, no failing input, structural
# by the seam's own shape. `Cluster::topic_names()` is not like that: it has
# to exist (`all_topics_names`'s own fail-open branch legitimately calls it
# when authorization is not configured, `M9.7`'s own signal), so the
# structural claim here is narrower and just as falsifiable: **exactly one**
# call site, in the one function and the one branch that is allowed to make
# it. A second call site — a new handler, a refactor that moves the call
# out of its guard — turns this red on the commit that adds it, the same
# property `m3-complete.sh`'s own check names as the point.
#
# ⚠️ Each pipeline ends `|| true`. `lib.sh` sets `pipefail`, and a `grep` that
# matches nothing exits 1 — without this the gate dies mid-section with no
# FAIL line, on the exact refactor most likely to break the scan
# (`m3-complete.sh`'s own note, repeated here rather than relearned).

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

CLUSTER_SEAM="crates/oqueue-broker/src/cluster/topics.rs"
CALLER_SEAM="crates/oqueue-broker/src/metadata.rs"

# ── 0a. The seams this claim rests on exist ─────────────────────────────────
for f in "$CLUSTER_SEAM" "$CALLER_SEAM"; do
  if [[ ! -f "$f" ]]; then
    fail "$f not found -- the zero-global-list claim is asserted from this file's shape"
    finish
  fi
done
ok "the topic registry and its one scoped caller are where this check reads them"

# ── 0b. The definition itself: still the one place the global list is built ─
def_line="$(grep -n '^    pub async fn topic_names' "$CLUSTER_SEAM" | head -1 | cut -d: -f1 || true)"
if [[ -z "$def_line" ]]; then
  fail "Cluster::topic_names not found at its pinned shape in $CLUSTER_SEAM"
  note "if it was renamed or moved, this check's own patterns need the same edit"
else
  ok "Cluster::topic_names is defined once, at $CLUSTER_SEAM:$def_line"
fi

# ── 0c. Every call site of it, workspace-wide production code ──────────────
# `src/` only -- a test setting up a fixture or asserting against the
# registry directly is not "answering a client request" and is exempt by
# scope, the same way `check-sans-io.sh`'s REAL_CLOCK pattern scopes itself
# to the files whose claim it is actually making.
mapfile -t call_sites < <(
  grep -rn '\.topic_names(' \
    --include='*.rs' \
    crates/*/src/ bin/*/src/ 2>/dev/null |
    grep -v "^${CLUSTER_SEAM}:${def_line}:" || true
)

if (( ${#call_sites[@]} == 0 )); then
  fail "no call site of Cluster::topic_names found at all"
  note "M9.10's own all_topics_names should call it once, in the fail-open branch"
  note "if that changed, this check needs the same edit -- not a silent pass"
elif (( ${#call_sites[@]} > 1 )); then
  fail "${#call_sites[@]} call sites of Cluster::topic_names found, expected exactly one"
  for site in "${call_sites[@]}"; do
    note "$site"
  done
  note "a second call site is a second code path materializing the global topic list"
else
  site="${call_sites[0]}"
  site_file="${site%%:*}"
  site_line="${site#*:}"
  site_line="${site_line%%:*}"
  if [[ "$site_file" == "$CALLER_SEAM" ]]; then
    ok "exactly one call site, in $CALLER_SEAM:$site_line"
  else
    fail "the one call site is in $site_file:$site_line, not the pinned $CALLER_SEAM"
    note "a call site outside all_topics_names's own fail-open branch is not scoped"
  fi
fi

# ── 0d. That one call site is the guard's own next statement, not merely   ──
# ── present somewhere in the same function                                ──
#
# ⚠️ **Adjacency, not co-occurrence.** Counting the guard string and the call
# string anywhere in the function body — round 1 review's own finding —
# would still pass a refactor that hoists `cluster.topic_names()` above the
# `if`, making it run unconditionally: both strings are still present, just
# no longer nested. The check that actually rules that out is narrower: the
# guard's own line and the call's own line must be adjacent, `return
# cluster.topic_names();` the fail-open branch's first and only statement.
if (( ${#call_sites[@]} == 1 )) && [[ "${call_sites[0]%%:*}" == "$CALLER_SEAM" ]]; then
  fn_body="$(awk '/^async fn all_topics_names/,/^}/' "$CALLER_SEAM")"
  if [[ -z "$fn_body" ]]; then
    fail "all_topics_names not found at its pinned shape in $CALLER_SEAM"
  else
    fn_calls="$(printf '%s\n' "$fn_body" | grep -c '\.topic_names(' || true)"
    guard_lineno="$(printf '%s\n' "$fn_body" | grep -n '^    if !authz.credentials_configured {$' | head -1 | cut -d: -f1 || true)"
    next_line=""
    if [[ -n "$guard_lineno" ]]; then
      next_line="$(printf '%s\n' "$fn_body" | sed -n "$((guard_lineno + 1))p")"
    fi
    if [[ "$fn_calls" == "1" && -n "$guard_lineno" && "$next_line" == *'.topic_names('* ]]; then
      ok "the one call site is the guard's own next statement in all_topics_names"
    else
      fail "all_topics_names's shape does not match: $fn_calls call(s), guard at line ${guard_lineno:-none}, next line: '${next_line}'"
      note "the call must be the fail-open branch's first statement, not merely present somewhere in the function"
      note "a call hoisted above the guard would run unconditionally -- exactly the regression this section exists to catch"
    fi
  fi
fi

finish
