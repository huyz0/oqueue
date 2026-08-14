#!/usr/bin/env bash
# Every crate depends on oqueue-core and nothing else in the workspace. `M-1.8`.
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
# - **`[target.'cfg(...)'.dependencies]`.** A platform-specific dependency
#   section is a distinct header this parser does not special-case; it falls
#   into "other" and is silently unscanned. Worth adding if this project ever
#   grows real platform-specific dependencies — `portability.md` is the
#   standard that would motivate it.
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
out="$(python3 - <<'PYEOF'
import re, sys, pathlib

root = pathlib.Path.cwd()
COMPOSERS = {"oqueue-broker", "oqueue"}   # "oqueue" is bin/oqueue's package name
DEV_ONLY = {"oqueue-testkit"}

def package_name(toml_path):
    section = None
    for raw in toml_path.read_text().splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        m = re.match(r'^\[([A-Za-z0-9_.\-]+)\]$', s)
        if m:
            section = m.group(1)
            continue
        if section == "package":
            m2 = re.match(r'^name\s*=\s*"([^"]+)"', s)
            if m2:
                return m2.group(1)
    return None

def runtime_deps(toml_path):
    """Dependency names under [dependencies], flow or table form. Never
    [dev-dependencies] or [build-dependencies] -- see the header."""
    deps = set()
    section = None
    for raw in toml_path.read_text().splitlines():
        s = raw.strip()
        if not s or s.startswith("#"):
            continue
        m = re.match(r'^\[([A-Za-z0-9_.\-]+)\]$', s)
        if m:
            header = m.group(1)
            if header == "dependencies":
                section = "deps_flow"
            elif header.startswith("dependencies."):
                deps.add(header[len("dependencies."):])
                section = "deps_table"
            else:
                section = "other"
            continue
        if section == "deps_flow":
            # `name = ...` and Cargo's dotted-key shorthand `name.workspace =
            # true` both name a dependency called `name` -- the part before
            # the first dot, if any.
            m2 = re.match(r'^([A-Za-z0-9_\-]+)(?:\.[A-Za-z0-9_.\-]+)?\s*=', s)
            if m2:
                deps.add(m2.group(1))
        # section == "deps_table": a field of the dependency named by the
        # header, e.g. `path = "..."` -- never a new dependency name.
    return deps

def build():
    manifests = sorted((root / "crates").glob("*/Cargo.toml"))
    bin_manifest = root / "bin/oqueue/Cargo.toml"
    if bin_manifest.exists():
        manifests.append(bin_manifest)

    if not manifests:
        print("SKIP no crate manifests found under crates/ or bin/oqueue/")
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

    checked = 0
    for m, name in names_by_manifest.items():
        checked += 1
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
  ok "crate layering (${checked:-0} manifest(s) hold)"
elif [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "layering violation: $p"
  done <<< "$problems"
  note "every crate depends on oqueue-core and nothing else in the workspace"
  note "the named exceptions are oqueue-broker and bin/oqueue (package: oqueue)"
else
  fail "layering check died (exit $rc); the tree was not checked"
fi

finish
