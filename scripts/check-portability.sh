#!/usr/bin/env bash
# The agent system is tool-portable. `M-1.30`, `.agents/skills/README.md`'s
# "The rules" 1-3, and the forward note under `M-1.36`'s retrospective.
#
#   scripts/check-portability.sh
#
# Three checks, all sourced from `.agents/skills/README.md` rather than
# invented here — that file is the standard this gate enforces, the same
# relationship every other `check-*.sh` has to a numbered rule elsewhere:
#
#   1. **No vendor-specific syntax in `AGENTS.md` or any `SKILL.md`.**
#      Concretely: no line whose first non-whitespace character is `@`, the
#      one vendor syntax this repository actually uses and names by
#      example — Claude Code's `@import`, which `CLAUDE.md` (the adapter)
#      uses and `AGENTS.md`/`SKILL.md` (the portable layer) must not. Lines
#      inside fenced code blocks are exempt: a skill illustrating what
#      *not* to write is not itself a violation.
#   2. **Every `SKILL.md` has `name` and `description` in its frontmatter,
#      both non-empty**, and `name` matches the skill's own directory —
#      a skill whose declared name is not the name a tool resolves it by
#      is exactly the silent-collision risk `M-1.36`'s retrospective
#      flagged when renaming `goal` to `milestone`.
#   3. **Every `.claude/commands/*.md` is a pointer, not a procedure.**
#      Checked three ways: a non-empty `description` in frontmatter, a line
#      naming `.agents/skills/<name>/SKILL.md` for a skill that actually
#      exists (a pointer at nothing is not a pointer), and the absence of
#      the shapes a real procedure would have — a markdown heading or a
#      numbered step. The same fenced-code-block exemption check 1 uses
#      applies here too: a pointer file that quotes an example inside a
#      ``` block is not a procedure because its example contains a `#` or a
#      numbered line.
#
# ⚠️ Deliberately not checked: whether a skill's name collides with some
# other tool's reserved name. That risk is real (see `M-1.36`) but checking
# it needs a list of names to avoid that this repository has no authoritative
# source for and would otherwise have to invent — the same reason NFR-55/56
# in `requirements.md` are marked UNDERIVED rather than guessed. If such a
# list becomes available, extending this script is a small change.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

require_python || finish

rc=0
out="$(python3 - <<'PYEOF'
import re, sys, pathlib

def build():
    root = pathlib.Path.cwd()
    problems = []
    checked = 0

    def strip_fenced_lines(text, rel=None):
        """(line_no, line) for every line outside a fenced code block, so an
        illustrative `@import` inside a ``` block is not mistaken for the
        file's own syntax. `line_no` is the line's position in the
        *original* text -- enumerating only the surviving lines would report
        a fenced-and-stripped file's real violations under the wrong line
        number, found by review reproducing it against a real SKILL.md that
        already has fenced blocks earlier in the file.

        ⚠️ If the file's ``` markers do not come in pairs, no exemption is
        granted at all -- every line is returned, fenced-looking or not --
        and a PROBLEM is recorded (when `rel` is given). Dropping the
        unclosed fence's "exempt" lines and returning early would silently
        exempt everything from the unclosed marker to end of file, an
        ordinary missing-closing-fence typo away from disabling every check
        past that point with no diagnostic at all -- found by review
        reproducing it directly. Failing closed on a malformed document,
        with a stated reason, is the same discipline `require_sha256` and
        `require_python` in `lib.sh` already use for a check that cannot
        skip and still mean anything."""
        lines = text.splitlines()
        fence_markers = sum(1 for line in lines if line.strip().startswith("```"))
        if fence_markers % 2 != 0:
            if rel is not None:
                problems.append(
                    f"{rel}: has an unterminated ``` fence -- no fence "
                    f"exemption applied; every line checked as-is"
                )
            return list(enumerate(lines, start=1))
        result = []
        in_fence = False
        for i, line in enumerate(lines, start=1):
            if line.strip().startswith("```"):
                in_fence = not in_fence
                continue
            if not in_fence:
                result.append((i, line))
        return result

    def fence_stripped_text(text, rel=None):
        """The same exemption as `strip_fenced_lines`, joined back into text
        for a regex search rather than a line-by-line scan -- used wherever
        a pattern, not a line number, is what matters."""
        return "\n".join(line for _, line in strip_fenced_lines(text, rel))

    def check_no_vendor_syntax(path):
        rel = path.relative_to(root)
        for i, line in strip_fenced_lines(path.read_text(encoding="utf-8"), rel):
            if line.lstrip().startswith("@"):
                problems.append(
                    f"{rel}:{i}: line starts with '@' -- "
                    f"vendor syntax (Claude Code's @import) belongs in CLAUDE.md, not here"
                )

    def frontmatter(text):
        """(dict, body) from a leading `---`-delimited YAML block, `key:
        value` pairs only -- this repository's frontmatter is always flat,
        so a real YAML parser is not needed. Returns (None, text) if there
        is no frontmatter block at all."""
        m = re.match(r'^---\n(.*?)\n---\n?(.*)$', text, re.S)
        if not m:
            return None, text
        fm = {}
        for line in m.group(1).splitlines():
            km = re.match(r'^([A-Za-z0-9_]+):\s*(.*)$', line)
            if km:
                fm[km.group(1)] = km.group(2).strip()
        return fm, m.group(2)

    # --- check 1: vendor syntax -------------------------------------------
    agents_md = root / "AGENTS.md"
    if agents_md.exists():
        checked += 1
        check_no_vendor_syntax(agents_md)
    else:
        problems.append("AGENTS.md not found")

    skill_files = sorted((root / ".agents" / "skills").glob("*/SKILL.md"))
    for skill_file in skill_files:
        checked += 1
        check_no_vendor_syntax(skill_file)

        # --- check 2: name + description, name matches the directory -----
        fm, _ = frontmatter(skill_file.read_text(encoding="utf-8"))
        rel = skill_file.relative_to(root)
        dirname = skill_file.parent.name
        if fm is None:
            problems.append(f"{rel}: no frontmatter block")
            continue
        name = fm.get("name", "")
        desc = fm.get("description", "")
        if not name:
            problems.append(f"{rel}: frontmatter has no 'name'")
        elif name != dirname:
            problems.append(
                f"{rel}: frontmatter name '{name}' does not match its "
                f"directory '{dirname}'"
            )
        if not desc:
            problems.append(f"{rel}: frontmatter has no 'description'")

    known_skills = {f.parent.name for f in skill_files}

    # --- check 3: every .claude/commands/*.md is a pointer -----------------
    command_files = sorted((root / ".claude" / "commands").glob("*.md"))
    for cmd_file in command_files:
        checked += 1
        rel = cmd_file.relative_to(root)
        text = cmd_file.read_text(encoding="utf-8")
        fm, body = frontmatter(text)
        if fm is None or not fm.get("description", ""):
            problems.append(f"{rel}: no non-empty 'description' in frontmatter")

        stripped_body = fence_stripped_text(body, rel)

        pm = re.search(r'\.agents/skills/([A-Za-z0-9_\-]+)/SKILL\.md', stripped_body)
        if not pm:
            problems.append(f"{rel}: does not point at a .agents/skills/*/SKILL.md")
        elif pm.group(1) not in known_skills:
            problems.append(
                f"{rel}: points at skill '{pm.group(1)}', which does not exist"
            )

        # Fences stripped here too, for the same reason check 1 strips them:
        # a pointer file that quotes an example (a bad commit message shape,
        # say) inside a fenced block is not a procedure just because the
        # example it quotes contains a heading or a numbered line.
        if re.search(r'^#', stripped_body, re.M):
            problems.append(f"{rel}: contains a markdown heading -- looks like a procedure, not a pointer")
        if re.search(r'^\s*\d+\.\s', stripped_body, re.M):
            problems.append(f"{rel}: contains a numbered step -- looks like a procedure, not a pointer")

    for p in problems:
        print(f"PROBLEM {p}")
    print(f"CHECKED {checked}")
    sys.exit(1 if problems else 0)

try:
    build()
except SystemExit:
    raise                      # the deliberate verdict above, not a crash
except Exception as exc:
    import traceback
    traceback.print_exc()
    print(f"PROBLEM the portability checker raised {type(exc).__name__}: {exc}")
    sys.exit(3)
PYEOF
)" || rc=$?

problems="$(printf '%s\n' "$out" | grep '^PROBLEM ' | sed 's/^PROBLEM //' || true)"
checked="$(printf '%s\n' "$out" | grep '^CHECKED ' | sed 's/^CHECKED //' || true)"

if [[ -n "$problems" ]]; then
  while IFS= read -r p; do
    [[ -n "$p" ]] || continue
    fail "$p"
  done <<< "$problems"
  note "no vendor syntax in AGENTS.md or any SKILL.md"
  note "every SKILL.md has a name and description, name matching its directory"
  note "every .claude/commands/*.md points at a real skill and reads as a pointer, not a procedure"
elif (( rc != 0 )); then
  # ⚠️ Fails closed on anything the checker itself did not choose to report --
  # the same reason check-readmes.sh and check-unsafe.sh treat an unexpected
  # exit as a failure rather than a silent pass.
  fail "portability checker died (exit $rc); the agent system was not checked"
else
  ok "agent system is tool-portable (${checked:-0} file(s) checked)"
fi

finish
