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

# Windows Git Bash can expose the Microsoft Store `python3` execution-alias
# stub ahead of a working `python.exe`. Prove that gates recover through the
# real `python` command and that the fallback reaches child Bash processes.
python_path="$(command -v python || true)"
if [[ -z "$python_path" ]] || ! "$python_path" -c 'pass' >/dev/null 2>&1; then
  fail "OS smoke requires a working python command to test the python3 fallback"
  finish
fi
fallback_bin="$SMOKE_ROOT/python-fallback-bin"
mkdir -p "$fallback_bin"
printf '%s\n' '#!/usr/bin/env bash' 'exit 127' > "$fallback_bin/python3"
chmod +x "$fallback_bin/python3"
PATH="$fallback_bin:$PATH"
export PATH
unset PYTHONUTF8 || true
require_tool python3 "install Python 3" || finish
if ! python3 -c 'import sys; assert sys.flags.utf8_mode == 1' >/dev/null 2>&1; then
  fail "Python gates are not running in UTF-8 mode"
  finish
fi
if ! python3 -c 'pass' >/dev/null 2>&1; then
  fail "require_tool accepted a non-functional python3 alias"
  finish
fi
require_python || finish
bash -c 'python3 -c '\''import sys; assert sys.version_info.major == 3'\''' \
  || fail "python fallback was not available to a child Bash process"

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

# Python writes CRLF to a pipe on Windows. Exercise the milestone coverage
# helper with that output on every OS so Bash array parsing cannot keep the
# carriage return attached to each reviewed commit id.
review_root="$SMOKE_ROOT/milestone-review"
mkdir -p "$review_root/scripts" "$review_root/docs/internal/product" \
  "$review_root/reviews" "$review_root/bin"
cp "$ROOT/scripts/lib.sh" "$ROOT/scripts/milestone-review.sh" "$review_root/scripts/"
printf '%s\n' '| M-1.1 | a fixture task | coverage is reported correctly | done |' \
  > "$review_root/docs/internal/product/backlog.md"
printf '%s\n' 'milestone review coverage fixture' > "$review_root/README.md"
git -C "$review_root" init -q
git -C "$review_root" config core.autocrlf false
git -C "$review_root" config user.email smoke@example.test
git -C "$review_root" config user.name 'OS smoke'
git -C "$review_root" add -A
git -C "$review_root" commit -qm 'M-1.1: create coverage fixture'
reviewed_commit="$(git -C "$review_root" rev-parse HEAD)"
printf '{"milestone":"M-1","commits":["%s"],"verdict":"pass","findings":[]}\n' \
  "$reviewed_commit" > "$review_root/reviews/milestone-M-1-fixture.json"
git -C "$review_root" add reviews/milestone-M-1-fixture.json
git -C "$review_root" commit -qm 'record coverage fixture verdict'
cat > "$review_root/bin/python3" <<'PYWRAPPER'
#!/usr/bin/env bash
set -euo pipefail
command python "$@" | awk '{ sub(/\r$/, ""); printf "%s\r\n", $0 }'
PYWRAPPER
chmod +x "$review_root/bin/python3"
unset -f python3 2>/dev/null || true
review_output="$(cd "$review_root" && PATH="$review_root/bin:$PATH" \
  bash scripts/milestone-review.sh coverage --milestone M-1)"
if [[ "$review_output" != *'1 commit(s) naming a task; 0 not yet reviewed'* || \
      "$review_output" == *'uncovered '* ]]; then
  fail "milestone review coverage mishandled Python CRLF output"
  note "$review_output"
fi

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
