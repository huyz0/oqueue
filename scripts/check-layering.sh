#!/usr/bin/env bash
# Crate manifests: layering, lints, profiles, overflow-checks. `M-1.8`, `M0.21`.
#
#   scripts/check-layering.sh
#
# Cargo already forbids a dependency *cycle* — that is a hard compile error,
# so cycles are structurally impossible and this script does not check for
# them. What Cargo does **not** prevent is a **layering violation**: a
# low-level crate depending on a high-level one is perfectly acyclic and
# perfectly wrong. See `docs/researches/19` §2 and `architecture.md`'s
# dependency rule.
#
# ⚠️ **`M0.21` widened this past layering**, because three more properties are
# read off the same manifests this script already parses and none had a gate:
#
#   * `lints.workspace = true` in every member — `rust-style.md` rule 3
#   * no `[profile]` in a member — cargo ignores it silently
#   * `overflow-checks = true` in the root's `[profile.release]` —
#     `security.md` rule 4, which names a gate that did not exist
#
# The name is now narrower than what it does. Renaming a script every gate,
# hook and standards table references is a bigger change than adding three
# assertions, so the header carries the truth instead. ⚠️ A reader grepping for
# `security.md` rule 4's gate will not find "overflow" in a filename.
#
# ## The rule
#
# 1. A crate's `[dependencies]` (runtime, never `[dev-dependencies]`) may name
#    another workspace crate only if that other crate is `oqueue-core`, or the
#    depending crate is one of the two named composers below.
# 2. `oqueue-testkit` may never appear in *any* crate's `[dependencies]`,
#    composer or not — it is dev-only, and a runtime dependency on it would
#    ship test helpers in the deployed binary. `[dev-dependencies]` is exactly
#    where it belongs.
#
# ## The two named exceptions
#
# `oqueue-broker` (the I/O shell) and `bin/oqueue`'s package, `oqueue` (the
# composition root), are allowed to depend on any workspace crate. Both are
# named here, in the script, not expressed as a pattern —
# `docs/researches/19` §2: "an exception you have to type is an exception
# someone had to justify."
#
# ## The parser, and why it stops at key names
#
# This does not use a real TOML parser. It only needs to know *which
# dependency names appear under `[dependencies]`*, not their values — path,
# version, or `workspace = true` are all irrelevant to this rule, so a
# hand-rolled section-and-key scanner is enough and a dependency this
# repository does not otherwise need is not. It tracks the current `[section]`
# header and, while inside a bare `[dependencies]` block, treats each
# `name = ...` line as a dependency name — including Cargo's dotted-key
# shorthand `name.workspace = true`, which names the same dependency `name`
# as `name = { workspace = true }` would — while inside a `[dependencies.name]`
# sub-table, the header itself supplies the name and the field lines below it
# (`path = ...`, `version = ...`) are correctly *not* mistaken for further
# dependency names, because they belong to a different parser state.
#
# ## What this does not catch
#
# - ⚠️ ~~**`[target.'cfg(...)'.dependencies]`.** A platform-specific dependency
#   section is a distinct header this parser does not special-case; it falls
#   into "other" and is silently unscanned.~~ — **`M1.32` made it scanned**, and
#   deliberately: a dependency that exists only on one platform still ships, so
#   NFR-52's rule must see it. It was previously included only by accident, when
#   the header failed to match and the section before it stayed in force.
# - **A crate whose directory name and `[package] name` disagree.** The
#   dependency-name-to-crate mapping assumes they match, which is the
#   convention `code-structure.md` states (`crates/oqueue-foo/` holds
#   `oqueue-foo`) and not otherwise enforced by this script.
# - **A dependency renamed via Cargo's `package` key**, e.g.
#   `sideways = { package = "oqueue-other", path = "../oqueue-other" }`. This
#   parser records the local alias (`sideways`) as the dependency name, never
#   resolves `package = "..."`, and the alias will not appear in
#   `all_names` — so the dependency is silently invisible to every rule
#   below rather than checked under its real identity. Nothing in this
#   workspace's stated conventions calls for renaming an internal crate, so
#   this is a real gap rather than an expected one, and closing it needs the
#   value-parsing this script deliberately does not do (see above). Flag it
#   in review if a `package =` key ever appears on a workspace-internal
#   dependency line.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "crate layering (no Cargo.toml yet)"
  finish
fi

require_python || finish

# `|| rc=$?` rather than a bare call: lib.sh's `set -e` would otherwise kill
# this script the moment python exits non-zero, before the PROBLEM lines below
# it could ever be read — the exact defect class M-1.9's retrospective
# catalogued for `build-index.sh` and every sibling script since.
rc=0
# ⚠️ `$REPO_ROOT`, which `lib.sh` resolved to an absolute path before it
# `cd`-ed there. Two wrong answers were measured first: a cwd-relative
# "scripts/lib" breaks when `negative.sh` runs a copied gate from a scratch
# tree, and `dirname "${BASH_SOURCE[0]}"` breaks when `review.sh` invokes the
# gate by a *relative* path from a different cwd — that one failed only inside
# the review harness, so every packet reported two gates FAILED.
export OQUEUE_SCRIPTS_DIR="$REPO_ROOT/scripts"
out="$(python3 - <<'PYEOF'
import os, re, sys, pathlib

root = pathlib.Path.cwd()
COMPOSERS = {"oqueue-broker", "oqueue"}   # "oqueue" is bin/oqueue's package name
DEV_ONLY = {"oqueue-testkit"}

# ⚠️ **One reader for dependencies and the package name**, shared with
# `check-readmes.sh` — `M1.32` consolidated the copies. ⚠️ They had not drifted from
# *each other* on any input this repository has: both were wrong the same way, on `[[bench]]`,
# target-scoped dependency tables, and either kind of quote. What they had
# drifted from is `sections()` below, already fixed for the header defects they
# still had. That module's header records the whole shape.
#
# ⚠️ **`sections()` and `table_names()` below are still their own readers**, and
# saying so is the point: they answer a different question (which table is this
# line under, for the `[profile]`/`[lints]` assertions) and they hold the
# *stronger* notion of a table boundary, which `manifest.py` now adopts rather
# than the other way round. They agree on every spelling measured, but a change
# to one is not a change to the other — the drift this task closed, still
# possible one level down.
# ⚠️ Relative to the repo root, which `lib.sh` has already `cd`-ed to. A
# heredoc has no `__file__` to resolve against.
sys.path.insert(0, os.environ["OQUEUE_SCRIPTS_DIR"] + "/lib")
from manifest import package_name, runtime_deps  # noqa: E402

def sections(text):
    r"""Yield `(table, line)` for every non-header line, `table` being the name
    of the table it sits under or None at top level.

    This file reads manifests without a TOML library on purpose (`M-1.8`), so
    what counts as a section boundary has to be decided once, here.

    ⚠️ **Any line starting with `[` ends the previous table**, even one this
    parser cannot name. That is the whole point, and getting it wrong is a
    *silent pass* rather than a crash: an earlier version matched headers with
    `^\[([A-Za-z0-9_.\-]+)\]$` and simply skipped anything else, so a quoted
    sub-table left the previous section **sticky** and its keys were read as
    the parent's. Measured: a root manifest whose `[profile.release]` sets only
    `lto`, followed by `[profile.release.package."*"]` setting
    `overflow-checks = true`, passed the assertion below — `security.md`
    rule 4's gate reporting ok on exactly the configuration it exists to
    reject. The real root manifest already writes `[profile.dev.package."*"]`.
    Found by review.

    ⚠️ **And a header may carry a trailing comment.** `[profile.release]  #
    tuned` is legal TOML and this repository comments densely; an anchored
    `$` match rejected it, which failed in the other direction — the gate
    refusing every commit while naming a key that was present."""
    section = None
    for raw in text.splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        if s.startswith("["):
            close = s.find("]")
            # An unterminated header is not a header this parser can trust
            # either; drop to top level rather than carry the old one forward.
            section = s[1:close].strip() if close > 0 else None
            continue
        yield section, s


def table_names(text):
    """Every table header in `text`, including ones holding no keys.

    ⚠️ Separate from `sections()`, which yields *keys*: a present-but-empty
    `[profile.release]` yields none, so asking `sections()` about it reported
    "no [profile.release] section" against a manifest that has one. The two
    questions — does this table exist, does it say this — are genuinely
    different and one traversal cannot answer both."""
    names = []
    for raw in text.splitlines():
        s = raw.strip()
        if not s.startswith("["):
            continue
        close = s.find("]")
        if close > 0:
            names.append(s[1:close].strip())
    return names


def section_says(text, table, pattern):
    """True if `pattern` matches a line inside the `[table]` table."""
    return any(sec == table and re.match(pattern, ln) for sec, ln in sections(text))


def build():
    manifests = sorted((root / "crates").glob("*/Cargo.toml"))
    # ⚠️ `bin/*/`, matching `check-readmes.sh`. Hardcoding `bin/oqueue` meant a
    # second binary would be README-gated and not lints-gated — and since
    # `M0.21`, `rust-style.md` rule 3 names this script as its only
    # enforcement. Found by review.
    manifests += sorted((root / "bin").glob("*/Cargo.toml"))

    if not manifests:
        print("SKIP no crate manifests found under crates/ or bin/")
        sys.exit(0)

    names_by_manifest = {}
    all_names = set()
    problems = []
    for m in manifests:
        name = package_name(m)
        if not name:
            problems.append(f"{m.relative_to(root)}: no [package] name")
            continue
        names_by_manifest[m] = name
        all_names.add(name)

    # ⚠️ **The root manifest, which is not a member and has no `[package]`.**
    # `security.md` rule 4 names a gate for `overflow-checks` -- "→ gate on the
    # profile setting" -- and until `M0.21` none existed. Dropping that line
    # leaves every test green while a wrapping length offset ships: the debug
    # build panics and the release build wraps, which is verbatim the
    # RUSTSEC-2026-0007 shape rule 4 cites.
    #
    # ⚠️ **`[profile.release]` and not every profile**, which is narrower than
    # `build.md` rule 7's wording and is the profile that matters: measured, a
    # crate with no `overflow-checks` line anywhere panics on `u8` 255+1 under
    # `dev` and prints `0` under `release`, and the root's other profiles reach
    # it by inheritance (`bench` via `dist`, not via `release` directly).
    # `build.md` rule 7 records what this does not cover.
    root_toml = (root / "Cargo.toml").read_text(encoding="utf-8")  # crash -> CRASH, below
    # ⚠️ Both halves through `sections()`, not a scan of its own. The first
    # version tested presence with an anchored regex and the value with the
    # helper, and the two disagreed about what a header is in *both*
    # directions — see `sections()`. Found by review, twice.
    if "profile.release" not in table_names(root_toml):
        problems.append("Cargo.toml: no [profile.release] section")
    elif not section_says(root_toml, "profile.release", r"overflow-checks\s*=\s*true"):
        problems.append(
            "Cargo.toml: [profile.release] does not set overflow-checks = true "
            "-- security.md rule 4"
        )

    checked = 0
    for m, name in names_by_manifest.items():
        checked += 1
        text = m.read_text(encoding="utf-8")

        # ⚠️ **`lints.workspace = true`, in every member.** `rust-style.md`
        # rule 3 promised this gate "once Cargo.toml exists, M0". Without it a
        # crate silently opts out of pedantic, nursery and the `unwrap_used`
        # ban by omitting one line -- and `M0.8` added ten manifests at once.
        # ⚠️ Section-scoped, not two independent searches. `[lints]` present
        # anywhere plus a bare `workspace = true` under some *other* table --
        # `[dependencies.foo]` has exactly that shape -- would otherwise pass a
        # manifest whose lints table is empty.
        if not (
            section_says(text, "lints", r"workspace\s*=\s*true")
            # ⚠️ **And the dotted shorthand**, which is the spelling
            # `rust-style.md` rule 3 uses in its prose. No manifest here writes
            # it today; `runtime_deps` already handles Cargo's dotted keys, and
            # rejecting the form the standard states would fail closed.
            # ⚠️ **At top level only.** The first version of this scanned every
            # line of the file regardless of table, which is the bug the
            # `[lints]` branch above exists to avoid, reintroduced beside it:
            # measured, `lints.workspace = true` misplaced inside `[package]`
            # makes cargo say `unused manifest key: package.lints`, exit 0, and
            # drop every workspace lint for that crate — and this gate said ok.
            # Found by review.
            or any(
                sec is None and re.match(r"^lints\.workspace\s*=\s*true", ln)
                for sec, ln in sections(text)
            )
        ):
            problems.append(
                f"{m.relative_to(root)} ({name}): no [lints] workspace = true "
                f"-- rust-style.md rule 3"
            )

        # ⚠️ **No `[profile]` in a member.** Cargo **ignores** it, warns on
        # stderr and exits 0, so a profile setting written here is invisible
        # rather than wrong -- worse than being wrong, because the file says
        # one thing and the build does another. `build.md`'s Profiles section
        # and ADR-0001.
        # ⚠️ Through `sections()` like the rest. `re.search` with `^` was the
        # one check in this loop reading raw lines, and an **indented**
        # `  [profile.release]` — which `tomllib` parses as a real
        # `profile.release` table and cargo warns-and-ignores — walked past it.
        # Measured by review.
        if any(t == "profile" or t.startswith("profile.") for t in table_names(text)):
            problems.append(
                f"{m.relative_to(root)} ({name}): has a [profile] section, which "
                f"cargo ignores in a member -- profiles live in the root manifest"
            )

        deps = runtime_deps(m) & all_names
        dev_only_hit = deps & DEV_ONLY
        if dev_only_hit:
            problems.append(
                f"{m.relative_to(root)} ({name}): {', '.join(sorted(dev_only_hit))} "
                f"in [dependencies] -- dev-only, belongs in [dev-dependencies]"
            )
        if name in COMPOSERS:
            continue
        stray = deps - {"oqueue-core"} - DEV_ONLY
        if stray:
            problems.append(
                f"{m.relative_to(root)} ({name}): depends on "
                f"{', '.join(sorted(stray))}, not oqueue-core -- not a named composer"
            )

    for p in problems:
        print(f"PROBLEM {p}")
    print(f"CHECKED {checked}")
    sys.exit(1 if problems else 0)

# Wrapped so an unexpected exception becomes a distinct exit code (3) rather
# than landing on 1 -- backported by `M-1.45` from `build-index.sh` and
# `check-unsafe.sh`. Without this, a crash in `package_name`/`runtime_deps`
# (a non-UTF-8 byte in a scanned Cargo.toml) happens today to be caught by
# the `else: fail "...died..."` branch below only because every `PROBLEM`
# line is printed after both loops finish -- an accident of statement order,
# not a guarantee. Streaming a `PROBLEM` line as it is found, instead of
# collecting and printing them all at the end, would put partial output on
# stdout before a later crash and route it to the "problems found" branch
# instead -- reporting only the violations found before the crash as though
# they were the whole story. A distinct exit code makes the crash case
# checkable regardless of where in this function it happens, instead of by
# where a print statement happens to sit.
try:
    build()
except SystemExit:
    raise
except Exception as exc:
    import traceback
    traceback.print_exc()
    print(f"CRASH the scanner raised {type(exc).__name__}: {exc}")
    sys.exit(3)
PYEOF
)" || rc=$?

problems="$(printf '%s\n' "$out" | grep '^PROBLEM ' | sed 's/^PROBLEM //' || true)"
skipped="$(printf '%s\n' "$out" | grep -c '^SKIP ' || true)"
checked="$(printf '%s\n' "$out" | grep '^CHECKED ' | sed 's/^CHECKED //' || true)"
crashed="$(printf '%s\n' "$out" | grep -c '^CRASH ' || true)"

if (( crashed > 0 )); then
  fail "the layering scanner crashed; the tree was not fully checked"
  note "$(printf '%s\n' "$out" | grep '^CRASH ')"
elif (( skipped > 0 )); then
  skip "crate layering (no crate manifests found)"
elif (( rc == 0 )) && [[ -z "$problems" ]]; then
  ok "crate manifests (${checked:-0} manifest(s) hold)"
elif [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "manifest violation: $p"
  done <<< "$problems"
  note "manifests must: depend only on oqueue-core, carry lints.workspace = true,"
  note "hold no [profile] section, and (root) set overflow-checks in [profile.release]"
  note "the named exceptions are oqueue-broker and bin/oqueue (package: oqueue)"
else
  fail "layering check died (exit $rc); the tree was not checked"
fi

finish
