#!/usr/bin/env bash
# No secret- or key-material-shaped field derives `Debug` unless it is
# wrapped in `oqueue_core::Redacted`. `M9.14`, `security.md` rules 6-7's
# static half — the type-level check that catches construction, not the
# hand-rolled interpolation `M9.15`'s log scan is the second net for.
#
#   scripts/check-secrets.sh
#
# ## What this checks
#
# Every `struct`/`enum` field declaration (`name: Type`, one per line — this
# codebase's own consistent style) whose name matches a secret-shaped
# pattern: `password`, `passwd`, `passphrase`, `secret`, `token`, `bearer`,
# `credential`, `proof`, `dek`, `auth_bytes`, or `key_material` by name (word-boundary, substring
# match on the field name itself, not the type). A match whose type text
# does not contain one of the safe carriers (`Redacted`, `Dek`,
# `WrappedKey` — see `SAFE_CARRIERS` below for why each one qualifies) fails. `oqueue_core::Redacted<T>`'s own
# invariant — neither `Debug` nor `Display` can reach `T`, by a missing
# bound rather than an override — is what makes wrapping sufficient; a
# struct with a `Redacted` field may derive `Debug` safely (`Redacted`'s own
# doc comment says so, and this gate treats it as satisfying the rule
# rather than as a violation, matching that doc exactly).
#
# ⚠️ **A name heuristic, not a type analyzer** — the same honest limit
# `check-drift.sh`'s own same-line heuristic names for itself. A field
# genuinely holding a secret under a name this pattern does not cover (or a
# secret smuggled through a type alias) is invisible to it; `security.md`'s
# own "what has no gate" section is where that gap is recorded rather than
# implied fixed.
#
# ⚠️ **Struct/enum field declarations only, not function parameters.** The
# rule this gate checks is "nothing holding a secret derives `Debug`" — a
# bare function parameter cannot itself carry a derive, so `fn verify(&self,
# authcid: &str, password: &str)` is not in scope even though the name
# matches; only a type that could carry `#[derive(Debug)]` is. Scoped by a
# brace-depth heuristic: a name-matched line only counts while inside a
# `struct { ... }` or `enum { ... }` body, tracked from the declaration
# line to the `}` that closes it at the same depth — this codebase's own
# consistent rustfmt style keeps every top-level item's own closing brace at
# column 0, which is what makes a simple depth counter reliable rather than
# a real parser being required.
#
# ⚠️ **Production sources only** (`crates/*/src/`), the same scope
# `check-sans-io.sh` uses for its own risk-shaped patterns — a test fixture
# building a throwaway struct to drive an assertion is not the shipped
# broker, and scanning it would flag patterns this gate has no reason to
# police.
#
# ## The escape hatch
#
# A field the heuristic flags that is not actually secret-shaped goes in
# `NOT_A_SECRET` below, `check-drift.sh`'s own `NOT_A_BOUND` map's exact
# idiom: a `path|field_name` key naming why the substring match is a false
# positive, not a violation. Recorded rather than the pattern loosened,
# because a looser name pattern is a wider hole for a real secret to fall
# through unnoticed.

source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ⚠️ **Every entry here is a name-heuristic false positive, argued, not a
# real secret this gate is choosing to ignore.** A `path|field` key that
# stops being true (the field starts actually holding secret material)
# needs its entry removed, not its reason trusted forever.
declare -A NOT_A_SECRET=(
  ["crates/oqueue-broker/src/authz.rs|credentials_configured"]="a bool flag naming whether a credential source exists, not a credential"
  ["crates/oqueue-broker/src/dispatch.rs|credentials"]="a container (Arc<PlainCredentials>) whose own element type (PlainCredential) already wraps its password in Redacted -- the secret bytes are Redacted one level down, not unwrapped here"
  ["crates/oqueue-core/src/rate_governor.rs|tokens"]="a rate-limiter's token-bucket count (NFR-12), unrelated to authentication -- 'token' collides with the credential sense only by English, not by meaning"
  ["crates/oqueue-core/src/object_meta.rs|precondition_token"]="ObjectStore's own conditional-write opaque token (an ETag/generation-style value the backend hands back and later compares), not an authentication credential"
)

mapfile -t files < <(git ls-files -- 'crates/*/src/*.rs' 2>/dev/null)

if (( ${#files[@]} == 0 )); then
  skip "secret-field scan (no crate sources tracked yet)"
  finish
fi

require_python || finish

rc=0
out="$(FILES="$(printf '%s\n' "${files[@]}")" \
       EXEMPT_KEYS="$(printf '%s\n' "${!NOT_A_SECRET[@]}")" \
       python3 - <<'PYEOF'
import os, re, sys

files = [l for l in os.environ["FILES"].splitlines() if l]
exempt = {l for l in os.environ["EXEMPT_KEYS"].splitlines() if l}

# ⚠️ **A component match, not a leading-word match.** `\bWORD\w*\b` (round 1
# review's own finding) only fires when the secret word *starts* the
# identifier — `\b` cannot see a boundary before `_`, which is a word
# character, so `client_secret`/`db_password`/`auth_token` were all
# invisible to it, the compound-name shape a real field is more likely to
# use than the bare root. Splitting on `_` and checking each component
# catches the prefix, the suffix, and a bare plural (`credentials`,
# `tokens`) alike.
SECRET_ROOTS = (
    "password", "passwd", "passphrase", "secret", "bearer", "credential",
    "proof", "token",
    # `M8.1`: the first plaintext key material in the codebase. `dek` as a
    # component catches `dek`, `dek_bytes`, `wrapped_dek` and `cached_dek`
    # alike. ⚠️ **`key` is deliberately not a root** — `key_id`, `key_layout`,
    # `object_key` and `partition_key` are ordinary names throughout this
    # codebase and a bare `key` root would flag nearly all of them, which is
    # the same flood the `bytes` note below rejects. `key_material` is caught
    # by exact substring instead, beside `auth_bytes`.
    "dek",
)
# ⚠️ `bytes` is **not** a root — this is a Kafka broker; `record_bytes`,
# `batch_bytes`, `header_bytes` and the like are ordinary field names
# everywhere in this codebase, and a bare `bytes` root would flag nearly
# all of them. `auth_bytes` is checked by exact substring instead, below,
# so the one field this gate is actually looking for is still caught
# without that flood.


def name_is_secret_shaped(name: str) -> bool:
    if "auth_bytes" in name or "key_material" in name:
        return True
    return any(
        part.startswith(root)
        for part in name.split("_")
        for root in SECRET_ROOTS
    )


# ⚠️ **Three safe carriers, not one** (`M8.1`). `Redacted<T>` is the original
# and the one `security.md` rule 7's carve-out names. `Dek` — `oqueue-core`'s
# plaintext data encryption key — earns the same standing for a stronger
# reason: it holds its bytes privately, its `Debug` is a fixed string with no
# route to them at any specifier, and it additionally zeroizes on drop
# (rule 8), which `Redacted` does not. `WrappedKey` holds *ciphertext* and
# holds it inside a `Redacted` one level down, so a field carrying one is not
# exposing material either.
#
# ⚠️ **Whole words, not substrings** (`M8.1`'s review): `Redacted` is not a
# plausible part of an unrelated type's name, but `Dek` is the prefix of
# nearly every type M8 adds — `DekEntry`, `DekCache`, `CachedDek` — and a
# substring match would have passed `dek: DekEntry` holding `[u8; 32]` behind
# a derived `Debug`, in the very milestone that introduces such types. A
# carrier still counts wherever it appears as a word, so `Arc<Dek>` and
# `Option<Redacted<Vec<u8>>>` are safe as before. ⚠️ A type *alias* hiding one
# of these is as invisible here as it always was.
SAFE_CARRIERS = ("Redacted", "Dek", "WrappedKey")
SAFE_CARRIER_RE = re.compile(r"\b(?:" + "|".join(SAFE_CARRIERS) + r")\b")


def type_is_safe(field_type: str) -> bool:
    return SAFE_CARRIER_RE.search(field_type) is not None


ITEM_OPEN_RE = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(struct|enum)\s+\w")
# ⚠️ **`finditer`, not `match` anchored at line start** (round 1 review's
# own second finding) — a struct-literal enum variant written on one line
# (`Refused { auth_token: String, other: u8 },`, the exact shape
# `read.rs`'s own `Bounded { offset: u64, length: u64 }` already uses in
# this codebase) has more than one field per line, and only the first
# token on the line is the variant name, not a field. Scanning the whole
# line for every `name: Type` occurrence, bounded by the next `,`/`{`/`}`,
# catches every field regardless of position on the line.
FIELD_RE = re.compile(
    r"(?:^\s*|[{,]\s*)(?:pub(?:\([^)]*\))?\s+)?([a-z_][a-z0-9_]*)\s*:\s*([^,{}]+?)(?=,|\{|\}|$)"
)
COMMENT_RE = re.compile(r"^\s*(?:///?|//!)")

problems = []
for path in files:
    try:
        with open(path, encoding="utf-8") as f:
            lines = f.readlines()
    except OSError:
        continue

    in_item = False
    depth = 0
    for lineno, line in enumerate(lines, start=1):
        stripped = line.rstrip("\n")
        if COMMENT_RE.match(stripped):
            continue
        if not in_item:
            if ITEM_OPEN_RE.search(stripped) and "{" in stripped:
                in_item = True
                depth = stripped.count("{") - stripped.count("}")
                if depth <= 0:
                    in_item = False
            continue

        depth += stripped.count("{") - stripped.count("}")
        for m in FIELD_RE.finditer(stripped):
            name, field_type = m.group(1), m.group(2)
            key = f"{path}|{name}"
            if (
                name_is_secret_shaped(name)
                and not type_is_safe(field_type)
                and key not in exempt
            ):
                problems.append(f"{path}:{lineno}: {stripped.strip()}")
        if depth <= 0:
            in_item = False

for p in problems:
    print(f"PROBLEM {p}")
print(f"COUNT {len(files)}")
PYEOF
)" || rc=$?

if (( rc != 0 )); then
  fail "check-secrets.sh's own scan crashed (exit $rc)"
  note "$out"
  finish
fi

problems=()
while IFS= read -r line; do
  [[ "$line" == PROBLEM* ]] && problems+=("${line#PROBLEM }")
done <<< "$out"
scanned="$(grep -c '^COUNT ' <<< "$out" || true)"
count="$(grep '^COUNT ' <<< "$out" | head -1 | sed 's/^COUNT //')"

if (( ${#problems[@]} > 0 )); then
  fail "${#problems[@]} secret-shaped field(s) carry material nothing redacts"
  for p in "${problems[@]}"; do
    note "$p"
  done
  note "wrap the field's type in oqueue_core::Redacted<T>, security.md rules 6-7"
  note "⚠️ key material belongs in oqueue_core::Dek instead: Redacted hides a"
  note "   value from Debug but does not zeroize it on drop, which rule 8 asks for"
else
  ok "check-secrets.sh: no unredacted secret-shaped field found (${count:-0} file(s) scanned)"
fi

finish
