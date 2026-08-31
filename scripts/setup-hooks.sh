#!/usr/bin/env bash
# Points this clone's git hooks at the tracked ones. Run once per clone.
#
#   scripts/setup-hooks.sh          install
#   scripts/setup-hooks.sh --check  report, change nothing (exit 1 if wrong)
#
# ⚠️ **`core.hooksPath`, not `pre-commit install`, and the reason is that
# `.git/hooks` is not tracked.** `AGENTS.md` already says a fresh clone
# enforces nothing until somebody runs a command — but the sharper problem is
# what a clone that *did* run one ends up with. This repository was committing
# through a hand-written `.git/hooks/pre-commit` written before
# `.pre-commit-config.yaml` existed, which ran **12** of the config's **17**
# pre-commit gates and silently omitted `check-crate`, `check-coverage`,
# `check-mutants`, `check-conformance-matrix` and `check-budget` — so every
# commit passed a subset while the tree claimed the whole suite. An untracked
# hook cannot be reviewed, cannot be updated by a commit, and cannot be seen to
# have gone stale. `M10.24` found that one; a tracked `.githooks/` is what
# stops the next.
#
# ⚠️ **It does not replace `.pre-commit-config.yaml`** — the hooks in
# `.githooks/` delegate straight back to `pre-commit run --hook-stage <stage>`,
# so that file remains the single definition CI reads too. What the tracked
# hooks add is *where* the pre-commit stage runs: inside `docker-test.sh`'s
# resource caps (`M10.23`), because on WSL2 an uncontained `cargo test` on the
# commit path can take the machine down.
#
# ⚠️ **`core.hooksPath` wins over `.git/hooks`**, so a clone that also ran
# `pre-commit install` is not broken by this and is not running two suites —
# git simply stops looking in `.git/hooks`. Nothing here deletes anything.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

WANT=".githooks"
CHECK=0
[ "${1:-}" = "--check" ] && CHECK=1

have="$(git config --get core.hooksPath || true)"

if [ "$have" = "$WANT" ]; then
  echo "setup-hooks: core.hooksPath is already $WANT"
else
  if [ "$CHECK" = 1 ]; then
    echo "setup-hooks: core.hooksPath is '${have:-unset}', expected '$WANT'" >&2
    echo "setup-hooks: run scripts/setup-hooks.sh to install the tracked hooks" >&2
    exit 1
  fi
  git config core.hooksPath "$WANT"
  echo "setup-hooks: core.hooksPath set to $WANT"
fi

# ⚠️ Executable bits are tracked by git, but a clone made with a umask or a
# filesystem that drops them leaves a hook git will not run — and git says
# nothing when a hook is present and not executable. Checked rather than
# assumed.
missing=0
for h in pre-commit commit-msg; do
  if [ ! -x "$WANT/$h" ]; then
    echo "setup-hooks: $WANT/$h is not executable" >&2
    missing=1
  fi
done
[ "$missing" = 0 ] || exit 1

# ⚠️ **An untracked hook left behind is now dead code that looks live**, which
# is how the stale one survived: it was named `.git/hooks/pre-commit` and did
# something, so nobody asked whether it did everything. Named, not deleted —
# removing files under `.git/` on someone's behalf is not this script's call.
stale=""
for h in pre-commit commit-msg; do
  [ -e ".git/hooks/$h" ] && stale="$stale .git/hooks/$h"
done
if [ -n "$stale" ]; then
  echo "setup-hooks: git now ignores these — they are no longer running:" >&2
  for f in $stale; do echo "  $f" >&2; done
  echo "setup-hooks: remove them once you have checked nothing local lived there" >&2
fi

echo "setup-hooks: pre-commit runs in the container (M10.23); commit-msg runs natively"
