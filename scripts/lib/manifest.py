"""One reader for `Cargo.toml`, shared by every gate that parses one. `M1.32`.

## Why this exists

Three hand-rolled copies: `check-layering.sh`'s `package_name` and
`runtime_deps`, and `check-readmes.sh`'s own `runtime_deps` — whose docstring
said "Mirrors check-layering.sh's parser", which is a comment asking to drift.
⚠️ They never drifted from each other on any input this repository has; the
one difference, `read_text(encoding=...)`, shows only under `python3 -X utf8=0`
with `LC_ALL=C`. What
they had drifted from is another reader, `check-layering.sh`'s own
`sections()`, already fixed for the defects below — which is why the first
attempt at this consolidation adopted the weaker of the two.
All three shared one section pattern, `^\\[([A-Za-z0-9_.\\-]+)\\]$`, and it
matched none of these real spellings:

    [[bench]]                              array-of-tables: the brackets double
    [target."cfg(unix)".dependencies]      quotes and parens in the header
    name = 'oqueue-broker'                 single-quoted value
    "oqueue-core" = { path = "..." }       quoted dependency key

⚠️ **An unmatched header does not reset the section**, which is what made this
dangerous rather than merely incomplete. Measured on a manifest with
`[[bench]]` after `[dependencies]`: `runtime_deps` returned
`{harness, libc, name, tokio}` — reading a benchmark's `name` and `harness`
keys, and a target-scoped `libc`, as runtime dependencies of the crate — while
`package_name` returned `None` for a single-quoted name, which
`check-layering.sh` reports as a manifest with no `[package]` name.

## Why not a TOML library

`tomllib` is stdlib from Python 3.11 and would delete this file. The floor
this repository actually holds to is `python3` with no third-party imports
(`check-portability.sh`'s reasoning, and `m0-complete.sh`'s no-PyYAML
fallback exists for the same reason) — but nothing pins a *minimum* Python
version, so a 3.9 or 3.10 runner would `ImportError`. Left as a line parser
with the divergences named above closed, and this paragraph as the record of
what would replace it.
"""

import re

# ⚠️ **No header regex, and specifically not an anchored one.**
# `check-layering.sh`'s own `sections()` records both defects an anchored
# pattern has, as measured bugs it was already fixed for — and `M1.32`'s first
# draft of this module reintroduced both by consolidating onto the weaker
# reader:
#
#   - **Any line starting with `[` ends the previous table**, even one this
#     parser cannot name. Getting that wrong is a *silent pass*: the previous
#     section stays sticky and the unnameable table's keys are read as its.
#   - **A header may carry a trailing comment.** `[dev-dependencies]  # test
#     only` is legal TOML and this repository comments densely; anchoring on
#     `$` rejects it, and then every key under it counts as a runtime
#     dependency.
#
# Measured on the first draft: `[[bench]] # micro` after `[dependencies]`
# returned `name` and `harness` as runtime dependencies — the very answer this
# module exists to stop.

# One dotted segment of a header path: bare, or quoted with either quote.
_SEGMENT = re.compile(r'"([^"]*)"|\'([^\']*)\'|([A-Za-z0-9_\-]+)')

# A key on the left of `=`, bare or quoted, optionally dotted.
_KEY = re.compile(r'^(?:"([^"]+)"|\'([^\']+)\'|([A-Za-z0-9_\-]+))'
                  r'(?:\.[A-Za-z0-9_.\-"\']+)?\s*=')

# A string value on the right of `=`, either quote.
_STRING_VALUE = re.compile(r'^\s*(?:"([^"]*)"|\'([^\']*)\')')


def _header_path(line):
    """`(segments, is_array)` for a table header, or `None` if the line is not
    a header at all.

    ⚠️ A line that opens with `[` but cannot be parsed returns `([], False)` —
    an *unnameable* header, which still ends the previous table. Returning
    `None` for it is the silent-pass bug described above.
    """
    if not line.startswith("["):
        return None
    is_array = line.startswith("[[")
    close = "]]" if is_array else "]"
    end = line.find(close, len(close))
    if end == -1:
        return [], False
    body = line[len(close) if is_array else 1:end].strip()
    if not body:
        return [], False
    segments = []
    pos = 0
    while pos < len(body):
        sm = _SEGMENT.match(body, pos)
        if not sm:
            pos += 1
            continue
        segments.append(next(g for g in sm.groups() if g is not None))
        pos = sm.end()
        while pos < len(body) and body[pos] in ' .':
            pos += 1
    return segments, is_array


def _key_of(line):
    """The key name on the left of `=`, or `None`. Quotes stripped."""
    m = _KEY.match(line)
    if not m:
        return None
    return next(g for g in m.groups() if g is not None)


def _lines(toml_path):
    for raw in toml_path.read_text(encoding="utf-8").splitlines():
        s = raw.strip()
        if s and not s.startswith("#"):
            yield s


def package_name(toml_path):
    """`package.name`, or `None`. Accepts either quote style."""
    section = None
    for s in _lines(toml_path):
        header = _header_path(s)
        if header is not None:
            section = header[0]
            continue
        if section == ["package"] and _key_of(s) == "name":
            vm = _STRING_VALUE.search(s[s.index("=") + 1:])
            if vm:
                return next(g for g in vm.groups() if g is not None)
    return None


def _is_dependency_section(segments):
    """Does this header introduce runtime dependencies?

    `[dependencies]`, `[dependencies.x]`, and the target-scoped forms. ⚠️ Never
    `dev-dependencies` or `build-dependencies` — the layering rule is about
    what ships. ⚠️ Target-scoped dependencies *do* count, and now do so
    deliberately: they were previously included by accident, because the
    header failed to match and the section before it stayed in force.
    """
    if not segments:
        return None
    if segments[0] == "dependencies":
        return segments[1] if len(segments) > 1 else None, True
    if segments[0] == "target" and "dependencies" in segments[1:]:
        i = segments.index("dependencies")
        return (segments[i + 1] if len(segments) > i + 1 else None), True
    return None


def runtime_deps(toml_path):
    """Dependency names under `[dependencies]`, flow or table form, including
    target-scoped. Never dev- or build-dependencies."""
    deps = set()
    in_deps = False
    for s in _lines(toml_path):
        header = _header_path(s)
        if header is not None:
            segments, is_array = header
            # ⚠️ An array-of-tables is never a dependency table, and — the
            # part that matters — it *ends* the previous one.
            hit = None if is_array else _is_dependency_section(segments)
            if hit is None:
                in_deps = False
            else:
                named, in_deps = hit
                if named is not None:
                    deps.add(named)
                    in_deps = False
            continue
        if in_deps:
            key = _key_of(s)
            if key is not None:
                deps.add(key)
    return deps


def _array_entries(text, state):
    """Feed one line of a flow array into `state`, yielding the strings it
    closes over.

    ⚠️ **A character scan, not a `split(",")`.** The split form dropped
    `members = ["crates/*"]  # every crate` — `_lines` strips whole-line
    comments only, so the chunk ended in a comment rather than in `]`, the
    quote test failed and the entry vanished. A dropped entry here is a *silent
    skip* in every gate that asks whether the workspace claims a crate, which
    is the exact failure `M5.58` exists to close; it came back inside the fix
    and review measured it.

    `state` is a one-element list holding whether the array is still open.
    """
    out = []
    i = 0
    while i < len(text):
        ch = text[i]
        if ch in "\"'":
            quote = ch
            i += 1
            value = []
            while i < len(text) and text[i] != quote:
                value.append(text[i])
                i += 1
            # An unterminated string is a manifest Cargo would refuse; taking
            # what there is and moving on keeps this a parser rather than a
            # validator.
            out.append("".join(value))
            i += 1
            continue
        if ch == "]":
            state[0] = False
            return out
        # ⚠️ A `#` outside a string ends the line, and inside one it does not —
        # which is why the quote arm above consumes its own characters.
        if ch == "#":
            return out
        i += 1
    return out


def workspace_members(toml_path):
    """The `[workspace] members` patterns, verbatim.

    ⚠️ **Patterns, not paths.** `members = ["crates/*"]` is ordinary and is
    what `M5.58` was about: a gate that greps the manifest's *text* for a crate
    name is switched off by a tidy-up nobody associates with it. Resolving them
    is `workspace_claims` below; this is the parse.

    ⚠️ Flow form only, because that is the form Cargo writes and this
    repository uses. A `[workspace.members]` table is not valid TOML for this
    key, so there is no second spelling to miss.
    """
    members = []
    in_workspace = False
    state = [False]
    for s in _lines(toml_path):
        header = _header_path(s)
        if header is not None:
            segments, is_array = header
            in_workspace = not is_array and segments == ["workspace"]
            state[0] = False
            continue
        if not in_workspace:
            continue
        if not state[0]:
            if _key_of(s) != "members":
                continue
            state[0] = True
            s = s.split("=", 1)[1]
        members.extend(_array_entries(s, state))
    return members


def workspace_claims(root, name):
    """Whether the workspace rooted at `root` has a member package called
    `name`, with every member pattern resolved.

    ⚠️ **The resolved list, not the manifest's text** (`M5.58`). A member
    entry is a directory, so the package's real name is in *its* manifest —
    which is what makes `members = ["crates/*"]` and
    `members = ["crates/oqueue-compact"]` answer the same question, as they
    must.
    """
    from pathlib import Path

    root = Path(root)
    for pattern in workspace_members(root / "Cargo.toml"):
        for path in sorted(root.glob(pattern)):
            manifest = path / "Cargo.toml"
            if manifest.is_file() and package_name(manifest) == name:
                return True
    return False
