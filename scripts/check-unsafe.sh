#!/usr/bin/env bash
# unsafe lives in three crates only, and every SAFETY: block is baselined.
# `M-1.11`.
#
#   scripts/check-unsafe.sh
#
# Non-negotiable 7 and `security.md` rules 18-19: `unsafe` is confined to
# `oqueue-buf`, `oqueue-codec`, `oqueue-checksum` -- the three leaf crates
# doc 18 §5.7's budget names, chosen because they line up with the crate
# split for free (`docs/researches/19` §1.3). Everywhere else is
# `#![forbid(unsafe_code)]` in intent; this is the gate that makes the
# intent checkable.
#
# ## The two rules
#
# 1. **`unsafe` outside the three named crates fails.** Matched only against
#    genuine unsafe-introducing Rust syntax -- `unsafe fn`, `unsafe impl`,
#    `unsafe trait`, `unsafe extern`, `unsafe {`, and the edition-2024
#    `#[unsafe(attr)]` wrapper -- never the bare word, and never text that
#    only *looks* like that syntax because it sits inside a comment or a
#    string. A doc comment merely talking about unsafety in prose, or
#    naming `unsafe(no_mangle)` by name while explaining why a crate does
#    not need it, does not trip the gate. Matched over the whole file's
#    text, not line by line, so a site split across a line break (`unsafe`
#    on one line, `{` on the next -- ordinary, `rustfmt`-legal Rust) cannot
#    evade it either.
# 2. **Every `unsafe fn` or `unsafe { ... }` site inside an allowed crate
#    needs a `// SAFETY:` comment directly above the `unsafe` keyword,
#    skipping over any attributes in between** (a `#[cold]`/`#[inline]`/
#    `#[target_feature(...)]` line does not break the association -- doc 18
#    §5.7 and the performance standard both expect hot-path unsafe
#    functions to carry exactly such attributes). A blank line, like any
#    other non-comment, non-attribute line, ends the search -- the comment
#    has to be immediately above, not merely somewhere above. That
#    comment's text needs a baseline entry in `baselines/unsafe.txt` -- the
#    same escape-with-a-reason shape `baselines/review.txt` already uses,
#    id = sha256(file + "\0" + the SAFETY comment's own text), truncated to
#    12 hex characters, so editing unrelated lines elsewhere in the file
#    never invalidates an entry, matching `testing.md` rule 17's mutants
#    baseline for the identical reason.
#
# ⚠️ This gate does **not** check doc 18 §5.7's other five conditions: a
# benchmark, the safe alternatives tried first, a `debug_assert!` of the
# precondition, the obligation encoded in a type where possible, a
# differential property test kept forever. None of those are mechanically
# checkable from the SAFETY comment's text alone -- the baseline entry's
# *reason* is where a human states that they were satisfied, and review is
# what checks the reason is honest. This gate checks only that the entry
# exists, the same "checks only what's mechanical" posture every other gate
# in this file states for itself.
#
# ## What this does not catch
#
# - **`unsafe trait` / `unsafe impl Marker for T`** (e.g. `unsafe impl Send`),
#   `unsafe extern` blocks, and `#[unsafe(attr)]` wrappers are not required
#   to carry a SAFETY comment by this gate -- they are type-level markers, a
#   list of foreign declarations, or an attribute annotation, not a block
#   asserting one inline runtime invariant, and doc 18 §5.7's six conditions
#   are framed around a block/function with a single precondition to state.
#   They still trip rule 1 if found outside the three named crates.
# - **An unterminated string, raw string, or char literal** (none of which
#   compile). Comment/string stripping below recognizes and correctly
#   closes ordinary double-quoted strings, raw strings (`r"..."`,
#   `r#"..."#`, `br##"..."##`), and single-character literals on their real
#   closing delimiter; an unterminated one has no real closing delimiter to
#   find, the same accepted gap `check-core-contract.sh`'s own comment
#   stripper discloses for itself for the identical reason.
# - **A `\u{...}` unicode escape inside a single-quote char literal.** The
#   bounded lookahead that recognizes a char literal (`'x'`, `'\n'`) checks
#   for a closing quote at a fixed offset and does not special-case the
#   variable-length unicode-escape form, so a literal like `'\u{5B}'`
#   (which happens to encode `[`) is left unrecognized and scanned as
#   ordinary text instead of being blanked. Harmless in practice: it can
#   only cause a stray bracket character to be *visible* to the attribute
#   depth counter, never invisible, which is the safe direction to be
#   wrong in.
# - **A baseline entry whose reason is dishonest.** The gate checks
#   presence, never truth -- see `security.md`'s own "what has no gate"
#   section for the same admission about error messages.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

ALLOWED_CRATES=("crates/oqueue-buf" "crates/oqueue-codec" "crates/oqueue-checksum")
BASELINE="baselines/unsafe.txt"

mapfile -t files < <(git ls-files -- '*.rs' 2>/dev/null)

if (( ${#files[@]} == 0 )); then
  skip "unsafe budget (no crate sources tracked yet)"
  finish
fi

require_python || finish

if [[ -f "$BASELINE" ]]; then
  baseline_staged="$(git show ":$BASELINE" 2>/dev/null || true)"
else
  baseline_staged=""
fi

# `|| rc=$?`: see check-core-contract.sh for why a bare call here would lose
# the PROBLEM lines under set -e.
rc=0
out="$(FILES="$(printf '%s\n' "${files[@]}")" \
       ALLOWED_PREFIXES="$(printf '%s\n' "${ALLOWED_CRATES[@]}")" \
       BASELINE_TEXT="$baseline_staged" \
       python3 - <<'PYEOF'
import bisect, hashlib, os, re, sys

def build():
    files = [l for l in os.environ["FILES"].splitlines() if l]
    allowed_prefixes = [l for l in os.environ["ALLOWED_PREFIXES"].splitlines() if l]
    baseline_text = os.environ["BASELINE_TEXT"]

    def is_allowed(path):
        return any(path == p or path.startswith(p + "/") for p in allowed_prefixes)

    baseline_ids = set()
    for line in baseline_text.splitlines():
        s = line.strip()
        if not s or s.startswith('#'):
            continue
        m = re.match(r'^([0-9a-f]{12})\s+\S', s)
        if m:
            baseline_ids.add(m.group(1))

    def looks_like_char_literal(text, i):
        # Bounded lookahead for the two shapes that occur in practice:
        # `'x'` and `'\x'` (one escaped char). Anything else starting with
        # `'` -- `'a`, `'static`, `''` -- is left as a lifetime or invalid
        # syntax and not touched; a real lifetime can never be mistaken for
        # a char literal by this check because it has no closing quote at
        # the position this looks for. This is the same "narrow special
        # case, not a full lexer" posture `check-core-contract.sh` already
        # takes for the identical ambiguity in its own scanner.
        n = len(text)
        j = i + 1
        if j >= n or text[j] == "'":
            return False
        if text[j] == '\\':
            return j + 2 < n and text[j + 2] == "'"
        return j + 1 < n and text[j + 1] == "'"

    def raw_string_hashes(text, i):
        """If `text[i]` is the opening `"` of a raw string (`r"`, `r#"`,
        `br##"`, ...), return how many `#` its delimiter uses. Otherwise
        `None`. Looks backward from the quote -- the same direction
        rustc's own lexer resolves the `r`/`br` prefix from -- and refuses
        to match if the prefix is itself the tail of a longer identifier.

        Found by review: raw strings have no escape sequences at all, but
        the ordinary `'string'` state below applies backslash-escaping
        uniformly to every double-quoted string. A raw string whose
        content ends in a backslash right before the closing quote (`r"\\"`,
        any Windows-style path) made that state treat the real closing
        quote as escaped and kept consuming -- silently blanking every
        real character after it, including a genuine `unsafe { ... }` site,
        until some unrelated later `"` or end of file. That is a false
        *pass*, the dangerous direction non-negotiable 7 exists to prevent,
        not a false rejection like every other gap this file discloses.
        Recognizing the raw-string delimiter and closing it on the correct
        rule (`"` followed by exactly that many `#`, no escaping in
        between) is what closes it, rather than merely disclosing it.
        """
        n = len(text)
        j = i - 1
        hashes = 0
        while j >= 0 and text[j] == '#':
            hashes += 1
            j -= 1
        if j < 0 or text[j] != 'r':
            return None
        j -= 1
        if j >= 0 and text[j] == 'b':
            j -= 1
        if j >= 0 and (text[j].isalnum() or text[j] == '_'):
            return None   # part of a longer identifier, not a real prefix
        return hashes

    def strip_for_scan(text):
        """Blank comments, string contents, and char-literal contents so
        SITE and the attribute-bracket counter below never see code that
        isn't really there.

        Ported from `check-core-contract.sh`'s `strip_comments()`, adapted
        for what this gate needs: string and char-literal *interiors* are
        blanked too, not merely left in place. Rounds 4 and 5 found the
        same "unbalanced bracket not stripped" defect twice -- once via a
        double-quoted attribute-argument string, once via a `/* */` block
        comment containing commented-out attribute-like text -- because
        each fix covered one specific case rather than the general one.
        Comments and literals are indistinguishable from real code to a
        regex or a naive bracket count; the fix is to make them
        indistinguishable from *whitespace* instead, once, for every
        caller, rather than special-case each new shape a bracket can hide
        inside.

        Every branch appends exactly one output character per input
        character consumed, and every `\\n` is preserved as itself in every
        state -- callers rely on the stripped text being the same length as
        the original, at the same line boundaries, so offsets found in the
        stripped text still index correctly into the original for line
        reporting.

        Unlike `check-core-contract.sh`'s version, no special-case lookaround
        is needed for a `'"'` char literal: char-literal detection here
        happens at the *opening* quote via bounded lookahead, before the
        interior `"` is ever visited on its own, so the ambiguity that
        forced that special case there does not arise here.
        """
        out = []
        i, n = 0, len(text)
        state = 'normal'
        esc = False
        saw_star = False
        raw_hashes = 0
        while i < n:
            c = text[i]
            if state == 'line_comment':
                out.append('\n' if c == '\n' else ' ')
                if c == '\n':
                    state = 'normal'
                i += 1
                continue
            if state == 'block_comment':
                out.append('\n' if c == '\n' else ' ')
                if saw_star and c == '/':
                    state = 'normal'
                    saw_star = False
                else:
                    saw_star = (c == '*')
                i += 1
                continue
            if state == 'string':
                if esc:
                    esc = False
                    out.append('\n' if c == '\n' else ' ')
                elif c == '\\':
                    esc = True
                    out.append(' ')
                elif c == '"':
                    state = 'normal'
                    out.append('"')
                else:
                    out.append('\n' if c == '\n' else ' ')
                i += 1
                continue
            if state == 'raw_string':
                # No escaping at all -- the only thing that closes a raw
                # string is `"` followed by exactly `raw_hashes` `#`s.
                if c == '"' and text[i + 1:i + 1 + raw_hashes] == '#' * raw_hashes:
                    state = 'normal'
                    out.append('"')
                    i += 1
                    for _ in range(raw_hashes):
                        out.append('#')
                        i += 1
                    continue
                out.append('\n' if c == '\n' else ' ')
                i += 1
                continue
            if state == 'char_lit':
                if esc:
                    esc = False
                    out.append('\n' if c == '\n' else ' ')
                elif c == '\\':
                    esc = True
                    out.append(' ')
                elif c == "'":
                    state = 'normal'
                    out.append("'")
                else:
                    out.append('\n' if c == '\n' else ' ')
                i += 1
                continue
            # state == 'normal': any of the five may open here.
            nxt = text[i + 1] if i + 1 < n else ''
            if c == '/' and nxt == '*':
                state = 'block_comment'
                out.append(' ')
                i += 1
                continue
            if c == '/' and nxt == '/':
                state = 'line_comment'
                out.append(' ')
                i += 1
                continue
            if c == '"':
                hashes = raw_string_hashes(text, i)
                if hashes is not None:
                    state = 'raw_string'
                    raw_hashes = hashes
                else:
                    state = 'string'
                out.append('"')
                i += 1
                continue
            if c == "'" and looks_like_char_literal(text, i):
                state = 'char_lit'
                out.append("'")
                i += 1
                continue
            out.append(c)
            i += 1
        return ''.join(out)

    # Matched only against genuine unsafe-introducing syntax (fn/impl/
    # trait/extern/a block/the attribute-wrapper form), never the bare
    # word -- an earlier version matched literal "unsafe" with a portable
    # word boundary and nothing else, which also matched the English word
    # in prose and failed the gate on crates that never used the `unsafe`
    # keyword at all. Found by review, fixed by requiring one of the
    # tokens that actually follows the keyword in real Rust syntax.
    #
    # Matched against `strip_for_scan(text)`, not the raw text -- found by
    # review: a `//` comment merely naming `unsafe(no_mangle)`, or a doc
    # example reading `unsafe fn`/`unsafe {`, matched exactly like real
    # code once the trailing-token requirement alone was satisfied. This is
    # the same class of gap the "What this does not catch" section used to
    # disclose as accepted; stripping comments and strings first closes it
    # instead.
    #
    # Group 2 is the "unsafe" keyword itself, not the token after it -- the
    # SAFETY-comment lookback below anchors to this group's line, because
    # "directly above it" means above `unsafe`, not above a `{`/`fn` that
    # may sit on a different line. `\s*` already matches a newline in
    # Python by default, so this spans `unsafe` and its trailing token
    # across a line break for free, as long as the match runs against the
    # whole file's text at once rather than once per line.
    SITE = re.compile(r'(^|[^A-Za-z0-9_])(unsafe)\s*(fn\b|impl\b|trait\b|extern\b|\{|\()')
    SAFETY_REQUIRED = {'fn', '{'}

    # `unsafe extern` is ambiguous by itself: `unsafe extern "C" fn foo() {
    # ... }` is a real function definition with a body and exactly one
    # inline invariant to state, indistinguishable in shape from `unsafe
    # fn`; `unsafe extern "C" { fn foo(); }` is a foreign block, a list of
    # declarations with nothing to assert. `SITE` alone cannot tell them
    # apart -- it only captures the token immediately after `unsafe`, which
    # is `extern` either way. Found by review: every `unsafe extern "ABI"
    # fn` site was silently treated as the marker-exempt block form and
    # never checked for a SAFETY comment or baseline entry at all, the same
    # dangerous-direction false pass as the last two rounds, just in a
    # construct the header's own "what this does not catch" section
    # mischaracterized as the *block* form's disclosed exemption when it
    # was actually excusing the function-definition form the exemption was
    # never meant to cover.
    #
    # Resolved by peeking past the optional ABI string literal for the
    # real continuation. An ambiguous or unrecognized continuation (a
    # macro-generated ABI string, an unusual layout this bounded lookahead
    # doesn't anticipate) resolves to `fn` -- requiring a SAFETY comment --
    # rather than to the exempt block form, because a false rejection here
    # costs a human a second look, while a false exemption costs nothing
    # and is invisible.
    EXTERN_CONTINUATION = re.compile(r'\s*(?:"[^"]*"\s*)?(fn\b|\{)')

    def resolve_extern_kind(scan_text, end):
        cont = EXTERN_CONTINUATION.match(scan_text, end)
        if cont and cont.group(1) == '{':
            return 'extern-block'   # foreign declarations: marker, no SAFETY needed
        return 'fn'                 # a real function definition, or unresolved: SAFETY required

    # An attribute -- `#[inline]`, `#[cold]`, `#[target_feature(...)]`, or a
    # multi-line one -- sits between a `// SAFETY:` comment and the
    # `unsafe` site it documents in exactly the ordinary style doc 18 §5.7
    # and the performance standard both recommend for hot-path unsafe
    # functions. Found by review: the lookback below originally stopped at
    # the first non-comment, non-blank line, so an attribute line broke the
    # association and a correctly annotated site failed the gate. Fixed by
    # marking every line that is *purely* an attribute (or an attribute
    # continuation) and making the lookback skip over those lines rather
    # than stopping at them. Operates on the *stripped* lines (comments and
    # string/char-literal interiors already blanked), not the raw ones --
    # otherwise a `[` inside an attribute's own string argument, or inside
    # a `/* */` block comment that merely contains attribute-shaped text,
    # desyncs the depth count for the rest of the file, silently marking
    # every later line -- including a real `// SAFETY:` comment -- as
    # "attribute". Found by review, across two separate rounds.
    #
    # Depth is tracked character by character, not by a whole-line
    # `str.count('[') - str.count(']')` -- found by review: a line where an
    # attribute closes and *real code follows on the same line*
    # (`#[derive(Debug)] struct Padding([u8; 4]);`) still nets to a
    # balanced bracket count for the whole line, since the struct's own
    # `[u8; 4]` also balances, so the whole line was marked transparent --
    # including the unrelated struct declaration after the attribute. The
    # lookback would then skip straight over it, letting a `// SAFETY:`
    # comment several lines further up attach itself to an unsafe site the
    # comment was never written to describe: a false *pass* on the
    # baseline-coverage check, the dangerous direction. Scanning character
    # by character finds the exact column the attribute's own bracket
    # closes at, so only a line whose content *after* that column is
    # blank is marked transparent -- a line mixing an attribute with real
    # code is left unmarked and acts as an ordinary boundary, same as any
    # other non-comment, non-blank line.
    def attribute_lines(stripped_lines):
        marked = set()
        in_attr = False
        depth = 0
        for k, ln in enumerate(stripped_lines):
            s = ln.strip()
            if not in_attr and (s.startswith('#[') or s.startswith('#![')):
                in_attr = True
                depth = 0
            if not in_attr:
                continue
            close_col = None
            for col, ch in enumerate(ln):
                if ch == '[':
                    depth += 1
                elif ch == ']':
                    depth -= 1
                    if depth <= 0:
                        close_col = col
                        break
            if close_col is None:
                marked.add(k)   # whole line is still inside the attribute
                continue
            in_attr = False
            if ln[close_col + 1:].strip() == '':
                marked.add(k)
            # else: real content follows the attribute's close on this
            # line -- leave it unmarked so the lookback treats it as a
            # real boundary.
        return marked

    def line_index_of(offset, line_starts):
        return bisect.bisect_right(line_starts, offset) - 1

    disallowed = 0
    no_comment = 0
    no_baseline = 0
    covered = 0
    scanned = 0

    for path in files:
        try:
            with open(path, encoding='utf-8') as fh:
                text = fh.read()
        except (OSError, UnicodeDecodeError) as exc:
            print(f"SKIPPED {path}: {type(exc).__name__}")
            continue
        scanned += 1

        lines = text.split('\n')
        line_starts = [0]
        for ln in lines[:-1]:
            line_starts.append(line_starts[-1] + len(ln) + 1)

        scan_text = strip_for_scan(text)
        attr_lines = attribute_lines(scan_text.split('\n'))

        # No line-level dedup here -- an earlier version skipped every match
        # after the first one found on a given line, on the mistaken
        # assumption that a line holds at most one relevant site. Found by
        # review: `unsafe fn f() { unsafe { ... } }`, or an edition-2024
        # `#[unsafe(no_mangle)] pub unsafe extern "C" fn foo() { unsafe {
        # ... } }`, puts two -- or more -- genuinely distinct `unsafe`
        # occurrences on one line, and the inner block (the one actually
        # performing the unsafe operation) was silently never examined for
        # a SAFETY comment or baseline entry at all: a real, uncommented,
        # unbaselined unsafe block reported as compliant. `SITE.finditer`
        # never produces two matches for the *same* keyword occurrence --
        # each match consumes its own boundary character and matching
        # resumes strictly after it -- so every match here is already a
        # genuinely distinct `unsafe` occurrence needing its own check;
        # there was nothing to dedup.
        allowed = is_allowed(path)
        for m in SITE.finditer(scan_text):
            i = line_index_of(m.start(2), line_starts)
            kind = m.group(3)  # 'fn'/'impl'/'trait'/'extern'/'{'/'(' -- \b is zero-width, not captured
            if kind == 'extern':
                kind = resolve_extern_kind(scan_text, m.end())

            if not allowed:
                print(f"DISALLOWED {path}:{i + 1}: {lines[i].strip()}")
                disallowed += 1
                continue

            if kind not in SAFETY_REQUIRED:
                continue   # marker impl/trait/extern/attr: confined, no SAFETY needed

            # The comment lookback reads the *raw* lines, not the stripped
            # ones -- it needs the real "SAFETY:" text to hash and baseline,
            # which strip_for_scan has deliberately blanked.
            j = i - 1
            comment_lines = []
            while j >= 0:
                if j in attr_lines:
                    j -= 1
                    continue
                s = lines[j].strip()
                if s.startswith('//'):
                    comment_lines.insert(0, s)
                    j -= 1
                elif s == '':
                    break
                else:
                    break
            safety_text = '\n'.join(comment_lines)
            if 'SAFETY:' not in safety_text:
                print(f"NOCOMMENT {path}:{i + 1}: {lines[i].strip()}")
                no_comment += 1
                continue
            key = f"{path}\0{safety_text}".encode('utf-8')
            h = hashlib.sha256(key).hexdigest()[:12]
            if h in baseline_ids:
                covered += 1
            else:
                print(f"NOBASELINE {path}:{i + 1}: id={h}")
                no_baseline += 1

    print(f"SUMMARY {scanned} {disallowed} {no_comment} {no_baseline} {covered}")
    sys.exit(1 if (disallowed or no_comment or no_baseline) else 0)

# Wrapped so that an unexpected exception becomes a distinct exit code (3)
# rather than landing on 1 -- the same code `sys.exit(1)` above uses
# intentionally for "real violations found". Without this,
# `check-core-contract.sh`'s own sibling defect class recurs here in a new
# shape: an uncaught `UnicodeDecodeError` from a non-UTF-8 byte in a
# scanned file killed the process with Python's own exit code 1,
# indistinguishable on the bash side from "no violations found" -- and
# every file after the crashing one in scan order was silently never
# checked. `except (OSError, UnicodeDecodeError)` around the one `open()`
# call closes the specific case found by review; this closes the general
# one, the same fix `build-index.sh` already applies to itself for the
# identical reason.
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

disallowed_lines="$(printf '%s\n' "$out" | grep '^DISALLOWED ' | sed 's/^DISALLOWED //' || true)"
nocomment_lines="$(printf '%s\n' "$out" | grep '^NOCOMMENT ' | sed 's/^NOCOMMENT //' || true)"
nobaseline_lines="$(printf '%s\n' "$out" | grep '^NOBASELINE ' | sed 's/^NOBASELINE //' || true)"
skipped_lines="$(printf '%s\n' "$out" | grep '^SKIPPED ' | sed 's/^SKIPPED //' || true)"
summary="$(printf '%s\n' "$out" | grep '^SUMMARY ' | sed 's/^SUMMARY //' || true)"
crashed="$(printf '%s\n' "$out" | grep -c '^CRASH ' || true)"

# A file the scanner could not read is not scanned for unsafe -- lib.sh's own
# contract says a gate states what it checked even when it passes, so this is
# a warn (visible, not a failure) rather than silence. A skipped file inside
# one of the three allowed crates is exactly where an undetected `unsafe`
# site would be most consequential, which is why this stays loud rather than
# folded into the "ok" line's file count.
if [[ -n "$skipped_lines" ]]; then
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    warn "not scanned for unsafe (could not read as UTF-8): $l"
  done <<< "$skipped_lines"
fi

if (( crashed > 0 )); then
  fail "the SAFETY/baseline scanner crashed; the tree was not fully checked"
  note "$(printf '%s\n' "$out" | grep '^CRASH ')"
elif [[ -n "$disallowed_lines" || -n "$nocomment_lines" || -n "$nobaseline_lines" ]]; then
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    fail "unsafe outside the three named crates: $l"
  done <<< "$disallowed_lines"
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    fail "unsafe site has no SAFETY: comment: $l"
  done <<< "$nocomment_lines"
  while IFS= read -r l; do
    [[ -n "$l" ]] || continue
    fail "SAFETY block has no baseline entry: $l"
  done <<< "$nobaseline_lines"
  [[ -n "$nobaseline_lines" ]] && note "add a line to $BASELINE: <id>  <why the six conditions in doc 18 §5.7 are met>"
elif (( rc != 0 && rc != 1 )); then
  fail "unsafe scan died (exit $rc); the tree was not fully checked"
else
  if [[ -n "$summary" ]]; then
    read -r total dis nc nb cov <<< "$summary"
    ok "unsafe budget holds ($total .rs file(s) scanned, $cov baselined SAFETY block(s))"
  else
    ok "unsafe budget holds"
  fi
fi

finish
