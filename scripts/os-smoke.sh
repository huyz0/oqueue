#!/usr/bin/env bash
# The cross-OS smoke tier for M12.18.
#
# Native Linux and macOS run this directly. Windows developers run it from
# WSL2, and the Docker wrapper remains the contained path for every Cargo
# command. The script deliberately exercises only shell/Python portability
# helpers, so it is safe on a runner without Docker or Rust.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
source "$ROOT/scripts/lib.sh"

SMOKE_ROOT="$ROOT/target/tmp/os-smoke.$$"
mkdir -p "$SMOKE_ROOT"
trap 'rm -rf "$SMOKE_ROOT"' EXIT

require_tool bash "install Bash 4+ or run through WSL2" || finish
require_python || finish

printf '%s\r\n' '---' 'name: smoke' 'description: line endings' '---' \
  > "$SMOKE_ROOT/crlf.md"
python3 - "$SMOKE_ROOT/crlf.md" <<'PY'
import pathlib
import re
import sys

text = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8")
if "\r" in text or not re.search(r"^---\nname: smoke\n", text):
    raise SystemExit("CRLF normalization did not produce parser-safe text")
PY

hash="$(printf 'oqueue-os-smoke\n' | sha256_stdin)"
[[ "$hash" =~ ^[0-9a-f]{64}$ ]] || fail "portable SHA-256 helper returned an invalid digest"

for script in "$ROOT"/scripts/*.sh "$ROOT"/scripts/gates/*.sh "$ROOT"/tests/gates/*.sh "$ROOT"/tests/harness/*.sh; do
  bash -n "$script" || fail "shell syntax check failed: $script"
done

bash "$ROOT/scripts/check-portability.sh"

if (( _FAILURES > 0 )); then
  finish
fi
printf 'ok  OS smoke (%s)\n' "${OSTYPE:-unknown}"
