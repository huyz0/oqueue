#!/usr/bin/env bash
# M12.14's real-listener role smoke: one artifact, three responsibility modes.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

python3 scripts/harness/role_smoke.py
