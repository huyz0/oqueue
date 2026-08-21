#!/usr/bin/env bash
# Every crate has a README.md and an AGENTS.md, and the README's stated
# dependencies match Cargo.toml. `M-1.27`, `code-structure.md` rules 13-15.
#
#   scripts/check-readmes.sh
#
# ## Scope
#
# Every directory under `crates/*/` **and `bin/*/`** with a `Cargo.toml`.
#
# ⚠️ `bin/oqueue` was excluded until `M0.21`, on the reasoning that the
# composition root is not a library crate in `architecture.md`'s crate map. That
# was written before it had documents. `M0.12` gave it a `README.md` and an
# `AGENTS.md`, `M0.13` added two dependencies to its manifest, and `M0.14`'s
# gate census made the gap visible: rule 15's README-matches-manifest property
# was ungated for exactly the crate whose dependency list was changing, and
# changes again at `M1`.
#
# ## The "Upstream" convention this gate enforces
#
# No crate exists yet, so this gate is also the first place the shape of
# "the README's stated dependencies" is made concrete rather than left as
# prose. rule 13 asks for an `## Upstream` section naming what the crate
# depends on and why; a bullet list, one dependency per item, the dependency
# name **backtick-quoted at the start of the line**, is the shape this
# script can check without also flagging every incidental `` `Result` `` or
# `` `lib.rs` `` a prose explanation happens to mention elsewhere in the
# section:
#
#   ## Upstream
#
#   - `oqueue-core` — the trait seam every fake and implementation share
#   - `tokio` — the async runtime this crate's I/O is written against
#
# Only the first backtick-quoted token on a line starting with `-` is read
# as a dependency name; anything later on that line, or on a non-bullet
# line, is prose and ignored.
#
# ## What this checks against Cargo.toml
#
# `[dependencies]` only — never `[dev-dependencies]` or `[build-dependencies]`,
# the same runtime-only scope `check-layering.sh` uses and for the same
# reason: a dev-only tool is not something the shipped crate depends on.
# **All** runtime dependencies, workspace-internal and external alike —
# `check-layering.sh` already narrows to internal crates for the layering
# rule; this gate is checking documentation completeness, which the "why
# each dependency is needed" language in rule 13 does not limit to internal
# ones.
#
# ## What this does not catch
#
# One limit `check-layering.sh` also names: a dependency renamed via Cargo's
# `package =` key is recorded under its local alias, not its real identity.
# ⚠️ ~~`[target.'cfg(...)'.dependencies]` is unscanned~~ — **`M1.32` made it
# scanned.** ⚠️ ~~This script's dependency parser is a second copy of that
# logic rather than a shared one, because no shared Python module exists
# across `scripts/*.sh` yet … flag it in review if the two ever drift on the
# same TOML shape.~~ — **`M1.32` created the module** (`scripts/lib/manifest.py`)
# and deleted the copy. ⚠️ **That note had nothing to fire on**: the two copies were
# behaviourally identical on every input this repository has — one
# `read_text(encoding=...)` apart, which distinguishes them only under
# `python3 -X utf8=0` with `LC_ALL=C`, on a manifest whose comments carry
# non-ASCII, as every manifest here does. What had diverged is these
# two against another reader in `check-layering.sh`, `sections()`, which
# was already fixed for the header defects both copies still had. That is why
# the first consolidation picked the weaker reader.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "crate documents (no Cargo.toml yet)"
  finish
fi

require_python || finish

# `|| rc=$?` rather than a bare call: the same reason every other Python-backed
# gate in this repository needs it -- lib.sh's `set -e` would otherwise kill
# this script the moment python exits non-zero, before the PROBLEM lines
# below could ever be read.
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
sys.path.insert(0, os.environ["OQUEUE_SCRIPTS_DIR"] + "/lib")
from manifest import runtime_deps  # noqa: E402

def build():
    root = pathlib.Path.cwd()

    # ⚠️ Was a near-copy of `check-layering.sh`'s, with a docstring saying
    # "Mirrors check-layering.sh's parser -- see this file's header for why it
    # is not shared" — a comment asking to drift. `M1.32` moved both to
    # `scripts/lib/manifest.py`; that module's header records what all three
    # copies got wrong the same way, and what they had drifted from.

    def stated_deps(readme_text):
        """Names read from `## Upstream`'s bullet list -- see this file's
        header for the exact shape required."""
        m = re.search(r'^## Upstream\n(.*?)(?=^## |\Z)', readme_text, re.S | re.M)
        if not m:
            return None
        names = set()
        for line in m.group(1).splitlines():
            line = line.strip()
            if not line.startswith("-"):
                continue
            bm = re.match(r'^-\s*`([A-Za-z0-9_\-]+)`', line)
            if bm:
                names.add(bm.group(1))
        return names

    manifests = sorted((root / "crates").glob("*/Cargo.toml")) + sorted(
        (root / "bin").glob("*/Cargo.toml")
    )
    if not manifests:
        print("SKIP no crate manifests found under crates/ or bin/")
        return

    problems = []
    checked = 0
    for manifest in manifests:
        crate_dir = manifest.parent
        rel = crate_dir.relative_to(root)
        checked += 1

        readme = crate_dir / "README.md"
        agents = crate_dir / "AGENTS.md"
        if not readme.exists():
            problems.append(f"{rel}: no README.md")
        if not agents.exists():
            problems.append(f"{rel}: no AGENTS.md")
        if not readme.exists():
            continue  # nothing left to check without it

        stated = stated_deps(readme.read_text(encoding="utf-8"))
        if stated is None:
            problems.append(f"{rel}/README.md: no ## Upstream section")
            continue

        actual = runtime_deps(manifest)
        missing = actual - stated
        extra = stated - actual
        if missing:
            problems.append(
                f"{rel}/README.md: Upstream section does not mention "
                f"{', '.join(sorted(missing))}, which {rel}/Cargo.toml depends on"
            )
        if extra:
            problems.append(
                f"{rel}/README.md: Upstream section mentions "
                f"{', '.join(sorted(extra))}, which {rel}/Cargo.toml does not depend on"
            )

    for p in problems:
        print(f"PROBLEM {p}")
    print(f"CHECKED {checked}")
    sys.exit(1 if problems else 0)

try:
    build()
except SystemExit:
    raise                      # the deliberate verdict above, not a crash
except Exception as exc:
    import traceback
    traceback.print_exc()
    print(f"PROBLEM the readme checker raised {type(exc).__name__}: {exc}")
    sys.exit(3)
PYEOF
)" || rc=$?

problems="$(printf '%s\n' "$out" | grep '^PROBLEM ' | sed 's/^PROBLEM //' || true)"
skipped="$(printf '%s\n' "$out" | grep -c '^SKIP ' || true)"
checked="$(printf '%s\n' "$out" | grep '^CHECKED ' | sed 's/^CHECKED //' || true)"

if (( skipped > 0 )); then
  skip "crate documents (no crate manifests found)"
elif [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "$p"
  done <<< "$problems"
  note "rule 13: every crate has a README.md naming what it is, why it exists, upstream, downstream"
  note "rule 14: every crate has an AGENTS.md"
  note "rule 15: the README's Upstream section and Cargo.toml's [dependencies] must agree"
elif (( rc != 0 )); then
  # ⚠️ Fails closed on anything the checker itself did not choose to report
  # (a traceback, an interpreter crash) -- the same reason build-index.sh
  # and check-unsafe.sh treat an unexpected exit as a failure rather than a
  # silent pass.
  fail "readme checker died (exit $rc); crate documents were not checked"
else
  ok "crate documents (${checked:-0} crate(s) checked)"
fi

finish
