#!/usr/bin/env bash
# `M11.9`, `M11.md` task 12: `M2`'s `enable.idempotence=false` client-config
# workaround stays retired.
#
#   scripts/check-idempotence-enabled.sh
#
# `M2.md`'s own risk list forced both real-client harness scripts
# (`scripts/harness/rdkafka_roundtrip.py`, `scripts/harness/RoundTrip.java`)
# to disable idempotent produce explicitly, since `InitProducerId` (key 22)
# did not exist yet — librdkafka and the Java client both enable it by
# default, so this was the one setting standing between "produces" and
# "produces the way a client actually configured would". `M11.4`-`M11.8`
# built the mechanism the disable was covering for; this is the gate that
# notices if the disable ever comes back, by accident or by a merge that
# resurrects it.
#
# ⚠️ **Fails closed on the *setting*, not on the milestone's own name for
# it.** A prose reference to "the enable.idempotence=false workaround" —
# this file's own header, a backlog row recording what `M11.9` removed — is
# a historical record, not a live disable, so the pattern below matches the
# actual key/value pair a config dict or properties file would set, never
# the word "idempotence" alone.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# Matches `"enable.idempotence": False` (Python dict, double- or
# single-quoted key), `enable.idempotence", "false"` (Java Properties), or
# the same shape with `=` — case-insensitive on the boolean literal, since
# Python's is capitalized and Java's is not.
#
# ⚠️ **Both quote characters, not just double** — review found the first
# version of this pattern only matched a double-quoted key, so
# `'enable.idempotence': False` (perfectly idiomatic Python, and exactly
# the shape a copy-paste or a merge could reintroduce) passed silently.
PATTERN="enable[._]idempotence['\"]?[[:space:]]*[:=,][[:space:]]*['\"]?[Ff]alse"

mapfile -t hits < <(
  grep -rlE "$PATTERN" \
    --include='*.rs' --include='*.py' --include='*.java' \
    crates/ bin/ scripts/harness/ 2>/dev/null | sort
)

if (( ${#hits[@]} > 0 )); then
  fail "M2's enable.idempotence=false workaround still exists"
  for f in "${hits[@]}"; do
    note "$f"
  done
  note "M11.4-M11.8 built InitProducerId and sequence admission; a real"
  note "client's default (idempotence on) should work against this broker"
  finish
fi
ok "no enable.idempotence=false workaround remains in client code"
finish
