"""Reads the fencing seam and says which wire code each `Refusal` variant answers.

⚠️ **This exists because a line scanner cannot do it, and four review rounds
proved that one syntax at a time.** `check-fencing-seam.sh` derived its code
list with awk, and every version was defeated by something ordinary:

  1. any `error_codes::` between `fn error_code(` and the closing brace — a
     comment naming a code the seam does not answer injected it, and a
     correct use of that code elsewhere was then reported as a violation;
  2. only lines containing `=>`, taking the leftmost code — a comment can
     contain an arrow, so the injection came straight back;
  3. stripping to the arrow with a greedy `.*` — which strips to the *last*
     arrow, so a trailing `// was => error_codes::X` substituted its code for
     the arm's and the gate stopped looking for the real one: a false pass;
  4. deleting `//.*$` first — the same thing one comment syntax over, via
     `/* ... */`, and still blind to a block-bodied arm
     (`Self::X => { error_codes::Y }`), which is rustfmt-stable, clippy-clean
     and what anyone writes on adding a second statement. No amount of
     comment stripping reaches that one.

The shape that ends the series is to stop reading lines: strip comments from
the whole function, split the match on `=>`, and read each arm as a pattern
and a body. An arm's body may be one expression or a block, on one line or
five, and a comment is gone before either is looked at.

⚠️ **It is a parser only in the sense the job needs.** It does not parse
Rust; it assumes the shapes `rustfmt` and this repository actually produce,
and it fails loudly rather than guessing when it meets something else — a
silent wrong answer here is a gate reporting that no handler bypasses the
seam when one does.
"""

from __future__ import annotations

import re
import sys

ENUM_RE = re.compile(r"^pub\(crate\) enum Refusal \{$", re.MULTILINE)
FN_RE = re.compile(r"fn error_code\(")
# A variant declaration: `Name,`, `Name(T),`, or `Name { .. }` — plus the
# last one, which may carry no trailing comma at all.
VARIANT_RE = re.compile(r"^\s+([A-Z][A-Za-z0-9]*)\s*[({,]?\s*$|^\s+([A-Z][A-Za-z0-9]*)\s*[({]")
CODE_RE = re.compile(r"\berror_codes::([A-Z][A-Z0-9_]*)")
SELF_RE = re.compile(r"\bSelf::([A-Z][A-Za-z0-9]*)")


def strip_comments(text: str) -> str:
    """Removes `//` and `/* */` comments, keeping the text's length shape.

    ⚠️ **Not string-aware, and that is a deliberate limit rather than an
    oversight.** `Refusal::error_code` is a `const fn -> i16` whose every arm
    is an `error_codes::` path; there is no string literal in it and a future
    one would be a different function. Being wrong here costs a missing code,
    which the caller turns into a failure — never a silent pass.
    """
    out = re.sub(r"/\*.*?\*/", " ", text, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", "", out)


def _balanced_block(text: str, open_at: int) -> str:
    """The `{ ... }` beginning at `open_at`, brace-matched."""
    depth = 0
    for i in range(open_at, len(text)):
        if text[i] == "{":
            depth += 1
        elif text[i] == "}":
            depth -= 1
            if depth == 0:
                return text[open_at + 1 : i]
    raise ValueError("unbalanced braces")


def variants(source: str) -> list[str]:
    m = ENUM_RE.search(source)
    if not m:
        raise ValueError("no `pub(crate) enum Refusal {` in the seam")
    body = _balanced_block(source, m.end() - 1)
    found = []
    for line in strip_comments(body).splitlines():
        if not line.strip() or line.lstrip().startswith("#["):
            continue
        got = VARIANT_RE.match(line)
        if got:
            found.append(got.group(1) or got.group(2))
    return found


def arms(source: str) -> list[tuple[str, str]]:
    """`(pattern, body)` per `match` arm in `error_code`, comments gone."""
    m = FN_RE.search(source)
    if not m:
        raise ValueError("no `fn error_code(` in the seam")
    fn_open = source.index("{", m.end())
    fn_body = strip_comments(_balanced_block(source, fn_open))
    match_at = fn_body.index("match")
    match_body = _balanced_block(fn_body, fn_body.index("{", match_at))

    # Split on `=>`, then hand each fragment back: the text before an arrow is
    # that arm's pattern, and the text after it up to the next arrow's own
    # pattern is its body. Walking the arrows rather than the lines is what
    # makes a block-bodied arm no different from a one-liner.
    pieces = match_body.split("=>")
    out = []
    for i in range(len(pieces) - 1):
        pattern = pieces[i]
        body = pieces[i + 1]
        if i + 2 < len(pieces):
            # Trim the next arm's pattern off the end of this body: everything
            # from the last `Self::`, `_` or `|` that starts it.
            cut = max(body.rfind("Self::"), body.rfind("\n"))
            if cut > 0:
                body = body[:cut]
        out.append((pattern.strip().splitlines()[-1].strip(), body.strip()))
    return out


def main() -> int:
    try:
        source = open(sys.argv[1], encoding="utf-8").read()
        vs = variants(source)
        arm_list = arms(source)
    except (OSError, ValueError, IndexError) as exc:
        print(f"PROBLEM {exc}", file=sys.stderr)
        return 2

    if not vs:
        print("PROBLEM the Refusal enum declares no variants", file=sys.stderr)
        return 2

    for pattern, _ in arm_list:
        stripped = pattern.strip()
        # A wildcard or a bare binding is a catch-all: every variant would
        # still "appear", while the code it answers stays out of the list and
        # a handler may then construct it anywhere.
        if stripped == "_" or stripped.startswith("_ ") or re.fullmatch(r"[a-z_][a-z0-9_]*", stripped):
            print(f"PROBLEM error_code has a catch-all arm: `{stripped}`", file=sys.stderr)
            return 2

    answered: dict[str, str] = {}
    for pattern, body in arm_list:
        code = CODE_RE.search(body)
        if not code:
            continue
        for named in SELF_RE.findall(pattern):
            answered[named] = code.group(1)

    missing = [v for v in vs if v not in answered]
    if missing:
        for v in missing:
            print(f"PROBLEM Refusal::{v} answers no error_codes:: path", file=sys.stderr)
        return 2

    for v in vs:
        print(f"{v}\t{answered[v]}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
