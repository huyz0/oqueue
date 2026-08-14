#!/usr/bin/env bash
# A pub trait's method set never changes without every implementor and an
# ADR in the same commit. `M-1.10`.
#
#   scripts/check-core-contract.sh
#
# Non-negotiable 6, and `contracts.md` rules 12-15. Doc 19 §3.2 names the two
# design decisions that carry the weight, both kept here:
#
# - **It compares the `fn` signature set inside each `pub trait` block**
#   between HEAD and the staged index — not a file's mtime, not "did this
#   file change at all." Editing a doc comment on a trait is not a contract
#   change, and a rule that fires on doc comments becomes noise and gets
#   disabled.
# - **It checks only two mechanical obligations**: every file that currently
#   implements the changed trait is *also* part of this commit's changed
#   files, and an ADR file is in the same commit. It does **not** try to
#   verify a call site was updated correctly, or that an implementor's body
#   actually matches the new signature — those are not mechanically
#   distinguishable from ordinary edits, and a gate that overclaims gets
#   disabled the first time it is wrong. That is review's job.
#
# ## The parser, and its honest limits
#
# No `syn`, no real Rust parser — a brace-depth-tracking scanner in Python,
# the same "a real parser is a dependency this repository does not need for
# five keys" argument `build-index.sh` already makes, extended to something
# harder than TOML front matter. For each `pub trait Name { ... }` block,
# found between the trait's `{` and its matching `}` by depth, every `fn `
# occurrence at depth 1 relative to that brace is one signature, captured up
# to its terminating `;` (a required method) or `{` (a provided method with a
# default body — its body is skipped by depth, never treated as more
# signatures). Two trait blocks are "the same contract" if their **set** of
# normalized signatures is unchanged; order and comment/whitespace changes
# are not a contract change.
#
# What this does not catch:
# - **A macro-generated `impl`** (`impl_trait_for!(Foo)`) is invisible; only
#   a literal `impl Name for` is found.
# - **Two traits with the same name in different modules** are treated as
#   one contract. Rare enough in a workspace this size to accept rather than
#   solve with a fully-qualified-path resolver this script does not have.
# - **An implementor file merely listed in the diff without actually having
#   been updated for the new signature** passes — see "checks only two
#   mechanical obligations" above. That gap is deliberate, not an oversight.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

if ! has_rust; then
  skip "core contract (no Cargo.toml yet)"
  finish
fi

require_python || finish

if ! git rev-parse --verify HEAD >/dev/null 2>&1; then
  skip "core contract (no HEAD yet)"
  finish
fi

mapfile -t changed_rs < <(git diff --cached --name-only --diff-filter=ACMR -- '*.rs' 2>/dev/null)
mapfile -t changed_all < <(git diff --cached --name-only --diff-filter=ACMR 2>/dev/null)

if (( ${#changed_rs[@]} == 0 )); then
  skip "core contract (no .rs files staged)"
  finish
fi

rc=0
# `|| rc=$?`: see check-layering.sh for why a bare call here would lose the
# PROBLEM lines under set -e.
out="$(CHANGED_RS="$(printf '%s\n' "${changed_rs[@]}")" \
       CHANGED_ALL="$(printf '%s\n' "${changed_all[@]}")" \
       python3 - <<'PYEOF'
import os, re, subprocess, sys

changed_rs = [l for l in os.environ["CHANGED_RS"].splitlines() if l]
changed_all = set(l for l in os.environ["CHANGED_ALL"].splitlines() if l)

def git_show(ref, path):
    r = subprocess.run(["git", "show", f"{ref}:{path}"],
                        capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None

def strip_comments(text):
    """Best-effort: blank out /* */ (nesting-aware) and // to end of line.

    Why this exists: `pub trait Name` and `impl Name for` are matched by
    plain substring search, and without this, a trait name mentioned in a
    later comment -- a stale doc example, commented-out old code, a
    migration note -- overwrites the real trait's captured signature set in
    the dict this builds, because re.finditer visits matches in source order
    and the last one wins. Found by review: a real breaking change to a
    `pub trait` passed silently, with the diff hidden by a *later* comment
    quoting the trait's old, unbroken shape.

    ⚠️ **Comment and string state are tracked together, in one pass, in this
    exact precedence order: string first, then block comment, then line
    comment.** Two earlier, separate-pass versions each failed differently:

    - A block-comment pass that ran *before* any string tracking existed
      blanked an unmatched `/*` inside an ordinary string literal (e.g.
      `"look: /* not closed"`, plausible in a MIME type, a glob pattern, or
      a log message) all the way to end of file, since there was no `*/`
      left to find it. Every trait declaration *after* that string --
      anywhere later in the file, not only near it -- silently vanished
      from extraction. This is the dangerous direction: a real breaking
      change reported as no change at all, defeating the one thing this
      gate exists to catch.
    - A per-line string tracker that ran *after* block-comment stripping
      mishandled a char literal holding a double quote (`'"'`): the
      unmatched `"` inside it left nothing to close the string state for
      the rest of the line, swallowing a genuine trailing `//` comment
      instead of cutting it -- the opposite direction (a non-change
      reported as changed), still wrong but the safer one to be wrong in.

    Checking `in_str` first, before `/*` or `//` are ever recognized, is
    what closes the first, more dangerous bug: a block comment cannot begin
    while scanning is inside a string, so an unmatched `/*` inside one can
    no longer consume the rest of the file. The char-literal case is
    special-cased narrowly (`'"'` specifically) rather than solved with a
    general char-literal lexer -- `'\\''` (an escaped single quote) and
    `'\\\\'` (an escaped backslash) never involve a bare `"` and so never
    reach that branch at all.

    Still not aware of a raw string (`r"..."`, `r#"..."#`), which uses
    different escaping rules this scanner does not model -- rare enough
    inside a trait's own signature or a file's `impl ... for` lines to
    accept rather than solve here.
    """
    out = []
    i, n = 0, len(text)
    depth = 0                 # block-comment nesting
    in_str = False
    esc = False
    in_line_comment = False
    while i < n:
        c = text[i]
        if in_line_comment:
            if c == '\n':
                in_line_comment = False
                out.append(c)
            else:
                out.append(' ')
            i += 1
            continue
        if depth > 0:
            out.append('\n' if c == '\n' else ' ')
            if text[i:i + 2] == '*/':
                depth -= 1
                i += 2
                continue
            i += 1
            continue
        if in_str:
            out.append(c)
            if esc:
                esc = False
            elif c == '\\':
                esc = True
            elif c == '"':
                in_str = False
            i += 1
            continue
        # Not inside a string, a block comment, or a line comment: any of
        # the three may open here, checked in that order.
        if text[i:i + 2] == '/*':
            depth = 1
            out.append('  ')
            i += 2
            continue
        if text[i:i + 2] == '//':
            in_line_comment = True
            out.append('  ')
            i += 2
            continue
        if c == '"':
            prev = text[i - 1] if i > 0 else ''
            nxt = text[i + 1] if i + 1 < n else ''
            if not (prev == "'" and nxt == "'"):   # not the '"' char literal
                in_str = True
            out.append(c)
            i += 1
            continue
        out.append(c)
        i += 1
    return ''.join(out)

def index_content(path):
    # git_show builds "{ref}:{path}"; the staged-index form is ":path" with
    # a single colon, so ref is empty here -- passing ":" built "::path" and
    # failed every single call, silently, because git_show swallows a
    # non-zero exit into None and every caller already treats None as "file
    # doesn't exist" rather than "the git invocation was malformed". Found by
    # running this against a real commit, not by reading it: every trait
    # looked like it had disappeared instead of changed.
    return git_show("", path)

def normalize(sig):
    return re.sub(r'\s+', ' ', sig).strip()

def extract_pub_traits(text):
    """name -> frozenset of normalized method signatures."""
    traits = {}
    for m in re.finditer(r'pub\s+trait\s+([A-Za-z_][A-Za-z0-9_]*)', text):
        name = m.group(1)
        brace_start = text.find('{', m.end())
        if brace_start < 0:
            continue
        depth = 0
        i = brace_start
        block_end = None
        while i < len(text):
            if text[i] == '{':
                depth += 1
            elif text[i] == '}':
                depth -= 1
                if depth == 0:
                    block_end = i
                    break
            i += 1
        if block_end is None:
            continue
        body = text[brace_start + 1:block_end]

        sigs = set()
        depth2 = 0
        j = 0
        while j < len(body):
            c = body[j]
            if depth2 == 0 and body[j:j + 3] == 'fn ':
                k = j
                while k < len(body) and body[k] not in ';{':
                    k += 1
                sigs.add(normalize(body[j:k]))
                if k < len(body) and body[k] == '{':
                    d = 1
                    k += 1
                    while k < len(body) and d > 0:
                        if body[k] == '{':
                            d += 1
                        elif body[k] == '}':
                            d -= 1
                        k += 1
                j = k
                continue
            if c == '{':
                depth2 += 1
            elif c == '}':
                depth2 -= 1
            j += 1
        traits[name] = frozenset(sigs)
    return traits

def extract_impl_files_by_trait(paths):
    """trait name -> set of paths whose *index* content implements it."""
    result = {}
    for p in paths:
        text = index_content(p)
        if text is None:
            continue
        text = strip_comments(text)
        for m in re.finditer(r'impl(?:<[^>]*>)?\s+([A-Za-z_][A-Za-z0-9_]*)(?:<[^>]*>)?\s+for\b', text):
            result.setdefault(m.group(1), set()).add(p)
    return result

problems = []
changed_traits = set()

for path in changed_rs:
    old_text = strip_comments(git_show("HEAD", path) or "")
    new_text = strip_comments(index_content(path) or "")
    old_traits = extract_pub_traits(old_text)
    new_traits = extract_pub_traits(new_text)
    for name in set(old_traits) | set(new_traits):
        if old_traits.get(name) != new_traits.get(name):
            changed_traits.add(name)

if not changed_traits:
    print("OK no pub trait's method set changed")
    sys.exit(0)

all_rs = subprocess.run(["git", "ls-files", "--", "*.rs"],
                         capture_output=True, text=True).stdout.splitlines()
impls_by_trait = extract_impl_files_by_trait(all_rs)

for name in sorted(changed_traits):
    print(f"CHANGED {name}")
    implementors = impls_by_trait.get(name, set())
    missing = sorted(p for p in implementors if p not in changed_all)
    for p in missing:
        problems.append(
            f"trait {name} changed, but implementor {p} is not part of this commit"
        )

decisions_touched = any(p.startswith("docs/internal/product/decisions/") for p in changed_all)
if not decisions_touched:
    problems.append(
        "trait method set changed but no file under docs/internal/product/decisions/ is part of this commit"
    )

for p in problems:
    print(f"PROBLEM {p}")
sys.exit(1 if problems else 0)
PYEOF
)" || rc=$?

changed_names="$(printf '%s\n' "$out" | grep '^CHANGED ' | sed 's/^CHANGED //' || true)"
problems="$(printf '%s\n' "$out" | grep '^PROBLEM ' | sed 's/^PROBLEM //' || true)"
ok_line="$(printf '%s\n' "$out" | grep '^OK ' | sed 's/^OK //' || true)"

if [[ -n "$ok_line" ]]; then
  ok "$ok_line"
elif [[ -z "$out" && "$rc" != 0 ]]; then
  fail "core contract check died (exit $rc); the tree was not checked"
elif [[ -n "$problems" ]]; then
  while IFS= read -r name; do
    [[ -n "$name" ]] || continue
    note "trait method set changed: $name"
  done <<< "$changed_names"
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "$p"
  done <<< "$problems"
  note "a contract change is one commit: the trait, every implementor, and an ADR"
elif [[ -n "$changed_names" ]]; then
  while IFS= read -r name; do
    [[ -n "$name" ]] || continue
    ok "trait $name changed with every implementor and an ADR in this commit"
  done <<< "$changed_names"
else
  fail "core contract check produced no result (exit $rc); the tree was not checked"
fi

finish
