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
