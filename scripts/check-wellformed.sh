#!/usr/bin/env bash
# Every tracked text file is the file someone meant to commit. `M5.70`.
#
#   scripts/check-wellformed.sh
#
# ## Why this exists
#
# ⚠️ **A commit landed twelve merge-conflict markers in `negative.sh` and the
# whole suite passed.** `bash -n` on those bytes exits 2, so **zero** of 175
# gate cases ran — a test suite that proves nothing, inside the suite that
# exists to catch exactly that shape — and nothing noticed, because no gate
# lints or executes a shell script and `check-crate.sh` covers Rust only.
# `M5.65`'s second round read the markers by eye.
#
# ⚠️ **Rust was never exposed to this**: `cargo fmt --check` refuses a file
# with conflict markers, and `cargo test` refuses one that does not compile. It
# is shell, Markdown and TOML that have no reader, and shell is where this
# repository keeps its enforcement.
#
# Two legs, and they are different questions:
#
#   1. **No tracked file carries a conflict marker at line start.** Cheap,
#      total, and it catches the marker wherever it lands — including in prose,
#      where nothing would ever execute it.
#   2. **Every tracked `*.sh` parses**, by `bash -n`. A script that does not
#      parse runs nothing, and a *gate* that runs nothing reports success.
#
# ⚠️ **Parsing is not running.** `bash -n` says the file is syntactically a
# shell script; it says nothing about whether the script is right. That is what
# `tests/gates/negative.sh` is for, and this is the floor beneath it.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

# ⚠️ **Anchored, and all three markers.** An unanchored match would fire on
# this script's own documentation and on any diff quoted in a commit body; a
# marker only means a conflict when it starts the line, which is the only way
# git writes one.
#
# ⚠️ **`=======` needs its own exact-length test.** A Markdown setext heading
# underlines with `=` too, and this repository's docs use them — so the pattern
# is seven, no more and no fewer, which is what git writes and what a heading
# almost never is.
CONFLICT_RE='^(<<<<<<< |>>>>>>> |=======$)'

mapfile -t tracked < <(git ls-files)
if (( ${#tracked[@]} == 0 )); then
  skip "well-formedness (nothing tracked yet)"
  finish
fi

marker_violations=0

# ⚠️ **One `git grep` over the index, not a loop of greps.** ⚠️ **Binary files
# are skipped rather than read**: `-I` is the same judgement `git diff` makes,
# and a PNG whose bytes happen to spell a marker is not a conflict. It reads
# what is *staged* rather than what is on disk — which is the half that matters, since
# the bytes that land in history are the staged ones — and `-I` skips binaries
# the way `git diff` does.
hits="$(git grep -I -n --cached -E "$CONFLICT_RE" -- . || true)"
if [[ -n "$hits" ]]; then
  while IFS= read -r hit; do
    [[ -n "$hit" ]] || continue
    fail "merge-conflict marker: ${hit%%:*}:$(cut -d: -f2 <<< "$hit")"
    marker_violations=$((marker_violations + 1))
  done <<< "$hits"
  note "a file with a marker in it is not the file anyone meant to commit"
  note "resolve it; a script with one parses to nothing and reports success"
fi
if (( marker_violations == 0 )); then
  ok "no tracked file carries a merge-conflict marker (${#tracked[@]} tracked)"
fi

mapfile -t shells < <(git ls-files -- '*.sh')
parse_violations=0
parsed=0
for f in "${shells[@]}"; do
  # ⚠️ **No `[[ -f "$f" ]]` guard, and that is the point.** A script deleted
  # from the working tree without the deletion being staged is still in the
  # index, still lands in history, and still runs nothing if it does not parse
  # — so a guard on the *disk* would skip exactly the file the commit carries.
  # The first draft had one, reported `ok` on that tree, and printed a count
  # larger than the number it had checked. `M5.70`'s first round measured it.
  #
  # ⚠️ **`bash -n` on stdin, reading the *staged* bytes**, for the same reason
  # the marker scan reads the index: the working tree is not what is being
  # committed. ⚠️ **`-` as the file argument**, because `bash -n file` and
  # `bash -n < file` differ in what they report as the name, and a diagnostic
  # naming `-` beside the path this loop prints is clearer than one naming a
  # temporary.
  #
  # ⚠️ **The diagnostic is captured, not spooled to a file.** `build.md`
  # rule 19 keeps scratch out of the system temp directory, and the shape of
  # that mistake here was worse than untidy: with anything in the way of
  # `/tmp/wellformed-parse.$$` the redirection fails, the `if !` branch fires,
  # and a perfectly good script is reported unparsable — a gate that blocks
  # every commit for a reason that has nothing to do with the tree.
  err=""
  if ! err="$(git show ":$f" | bash -n - 2>&1 >/dev/null)"; then
    fail "$f does not parse as a shell script"
    while IFS= read -r line; do
      [[ -n "$line" ]] && note "${line/#-: /}"
    done <<< "$err"
    note "a script that does not parse runs nothing, and a gate that runs"
    note "nothing reports success"
    parse_violations=$((parse_violations + 1))
    continue
  fi
  parsed=$((parsed + 1))
done
if (( parse_violations == 0 )); then
  ok "every tracked shell script parses ($parsed script(s))"
fi

finish
