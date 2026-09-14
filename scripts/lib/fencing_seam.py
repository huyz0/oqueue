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
# A variant declaration: `Name,`, `Name(T),`, `Name { .. }`, or
# `Name = 9,` — plus the last one, which may carry no trailing comma at all.
#
# ⚠️ **The discriminant form was missed, and it was the one gap that was a
# false *pass*.** `M4.36`'s own commit body recorded it and nobody acted;
# `M4.56` is that row. Measured before the fix: a copy of the seam with
# `FencedInstance = 9,` and its `Self::FencedInstance =>
# error_codes::FENCED_INSTANCE_ID,` arm printed the same six variant/code
# lines and exited 0, so `FENCED_INSTANCE_ID` never entered `CODES`,
# `MIN_VARIANTS` stayed satisfied, and a handler could construct that code
# anywhere while `check-fencing-seam.sh` said nothing. This module's own
# header claims it "fails loudly rather than guessing"; for that input it
# guessed.
VARIANT_RE = re.compile(
    r"^\s+([A-Z][A-Za-z0-9]*)\s*(?:=\s*[^,{(]+?)?\s*[({,]?\s*$"
    r"|^\s+([A-Z][A-Za-z0-9]*)\s*[({]"
)
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
    # ⚠️ **Attributes are consumed by bracket depth, not by their first
    # line.** Skipping a line that merely *starts* `#[` leaves the
    # continuations of one rustfmt wrapped past 100 columns to be judged as
    # declarations, which the refusal below then rejects: a seam that parsed
    # cleanly, failing on its formatting, with the message pointing at a line
    # that is not a declaration. Found by review of `M4.56` — the same commit
    # that made an unmatched line fatal and so created the hazard.
    lines = strip_comments(body).splitlines()
    index = 0
    while index < len(lines):
        line = lines[index]
        index += 1
        if line.lstrip().startswith("#["):
            depth = line.count("[") - line.count("]")
            while depth > 0 and index < len(lines):
                depth += lines[index].count("[") - lines[index].count("]")
                index += 1
            # ⚠️ **Brackets that never balance are a refusal, not a shorter
            # list.** Running off the end silently consumed every declaration
            # after the attribute — a partial answer with no diagnostic,
            # which is the exact class the branch below was added to close
            # and which this walk reintroduced one round later. Found by
            # review of `M4.56`.
            if depth > 0:
                raise ValueError(
                    f"unbalanced attribute in the Refusal enum body: {line.strip()!r}"
                )
            continue
        if not line.strip():
            continue
        got = VARIANT_RE.match(line)
        if got:
            found.append(got.group(1) or got.group(2))
            continue
        # ⚠️ **An unmatched line is a refusal, not a skip** — `M4.56`'s own
        # review, and the difference between fixing the instance and closing
        # the class. Widening `VARIANT_RE` for `Name = 9,` left the *silent*
        # half intact: `Name = (9),` and `Name = compute(),` are valid
        # discriminants this regex still rejects, and without this branch each
        # dropped its variant with no diagnostic — so its code never entered
        # the derived list, `MIN_VARIANTS` stayed satisfied, and a handler
        # constructing it passed. Bit for bit the defect this task closes, one
        # syntax over.
        #
        # ⚠️ **A struct variant's field lines are why this is not simply
        # "every unmatched line"**: its body spans lines this regex is not
        # meant to match. They are skipped by shape — `name: Type,` or a
        # closing brace — so a genuinely unrecognised *declaration* still
        # fails loudly.
        if re.match(r"^\s*(\}|[a-z_][A-Za-z0-9_]*\s*:)", line):
            continue
        raise ValueError(f"unrecognised line in the Refusal enum body: {line.strip()!r}")
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
        out.append((_pattern_of(pattern), body.strip()))
    return out


def _pattern_of(fragment: str) -> str:
    """This arm's pattern, with the previous arm's body trimmed off the front.

    ⚠️ **The last line alone is not the pattern when rustfmt wraps an
    or-pattern**, which was the second of `M4.36`'s three recorded gaps.
    `Self::A\n| Self::B =>` kept only `| Self::B`, so `Self::A` matched no
    arm and the gate reported `Refusal::A answers no error_codes:: path`
    against a correct seam — a false *failure*, which costs a commit rather
    than a violation, and is why it ranks below the discriminant gap.

    Walking back while the lines are joined by `|` keeps both spellings
    rustfmt produces (the bar trailing the previous line, or leading the
    next) without re-parsing Rust: a line that neither ends nor is followed
    by one belongs to the previous arm's body.
    """
    lines = fragment.strip().splitlines()
    if not lines:
        return ""
    taken = [lines[-1]]
    index = len(lines) - 2
    while index >= 0:
        previous = lines[index].strip()
        if previous.endswith("|") or taken[0].strip().startswith("|"):
            taken.insert(0, lines[index])
            index -= 1
        else:
            break
    return " ".join(line.strip() for line in taken).strip()


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
        # ⚠️ **Every `error_codes::` path in the body, not the first one** —
        # the third of `M4.36`'s recorded gaps. `search` exported the first
        # and dropped the rest silently, so an arm answering two codes put
        # one of them outside the derived list and a handler could then
        # construct *that* one anywhere. There is no correct single answer
        # for such an arm, so this refuses rather than choosing: a seam whose
        # arm is a conditional is a seam this gate cannot describe, and
        # saying so is the whole of what `fails loudly rather than guessing`
        # is supposed to mean.
        #
        # ⚠️ **Exporting *both* was the alternative, and it is rejected
        # rather than unconsidered** — review asked, and the cost of refusing
        # is real: a later milestone writing a legitimate conditional arm
        # blocks every commit until the enum is restructured, and
        # `check-fencing-seam.sh` calls `finish` on a `PROBLEM`, so its
        # handler-scan leg does not run either. Exporting both would
        # grep-protect both codes — strictly more coverage, and no false
        # pass. ⚠️ **What breaks is the caller's parsing of this output, and
        # the direction matters** — *one-to-many*, not many-to-one. Two
        # variants answering one code is already supported and correct: the
        # or-pattern fixture is that shape, and `check-fencing-seam.sh:82`
        # floors variants rather than deduped codes in bold for exactly that
        # reason. One variant answering two codes has no good spelling.
        # Printing two lines makes `cut -f1` count that variant twice, so the
        # floor reads seven for a six-variant enum and `the seam answers N
        # code(s) across M variant(s)` lies. Printing `Variant\tA,B` is
        # worse: `CODES` then holds the literal `A,B` and the grep for
        # `error_codes::A,B\b` matches nothing, silently losing protection
        # for *both* codes. ⚠️ A first version of this paragraph argued from
        # `MIN_VARIANTS` counting variants against codes, which it does not;
        # review caught that, and the conclusion survives on the mechanism
        # above rather than the one first written down.
        # Refusing keeps the gate's own model honest and makes the day such
        # an arm is written a deliberate decision rather than a silent
        # widening. Revisit here, not in the caller.
        codes = dict.fromkeys(CODE_RE.findall(body))
        if not codes:
            continue
        if len(codes) > 1:
            named = ", ".join(f"Refusal::{n}" for n in SELF_RE.findall(pattern)) or "an arm"
            print(
                f"PROBLEM {named} answers more than one error_codes:: path: "
                f"{', '.join(codes)}",
                file=sys.stderr,
            )
            return 2
        code = next(iter(codes))
        for named in SELF_RE.findall(pattern):
            answered[named] = code

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
