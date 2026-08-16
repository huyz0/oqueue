#!/usr/bin/env bash
# The agent system is tool-portable. `M-1.30`, `.agents/skills/README.md`'s
# "The rules" 1-3, and the forward note under `M-1.36`'s retrospective.
#
#   scripts/check-portability.sh
#
# Four checks. The first two and the last are sourced from
# `.agents/skills/README.md` rather than invented here — that file is the
# standard this gate enforces, the same relationship every other `check-*.sh`
# has to a numbered rule elsewhere. ⚠️ **Check 3 is different**: it traces to
# no rule in that file, because it holds a claim that file *makes*. `M0.1` was
# a task written to correct exactly that claim and `M0` falsified it again, so
# the sentence needed an owner rather than a rule.
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
#   3. ⚠️ **Neither index file claims a script is missing that is present.**
#      `M0.1` was a whole task written to correct exactly that in
#      `AGENTS.md` and `.agents/skills/README.md`, and by the end of `M0`
#      both were false again — `M0.2` wrote `check-crate.sh` and `M0.17`
#      wrote `mutants.sh` while the README went on saying "a few scripts
#      these skills invoke ... are still unwritten". A prose claim about
#      what exists on disk is exactly the claim a script should hold, and
#      nobody re-reads an index file they have already read once. So: if
#      every `scripts/*.sh` that any `SKILL.md` names does exist, neither
#      file may carry a sentence saying some are missing. `M0.24`.
#   4. **Every `.claude/commands/*.md` is a pointer, not a procedure.**
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

    def check_missing_script_claims(skill_files):
        """Check 3 -- see the header.

        ⚠️ **The assertion is a conditional, not a word ban.** "Some scripts
        are missing" is a fine sentence to write on a day it is true; what
        cannot stand is writing it on a day every script it refers to is
        present. So the trigger is the *disagreement* between the sentence
        and the disk, which is the only form of this a script can judge and
        the exact form that went stale twice.

        ⚠️ **And the mechanism is a list of phrasings, which is a real bound.**
        `claims` below holds four; "Some scripts these skills invoke do not
        exist yet" is not among them and passes with everything present. A
        general "is this sentence about missing scripts" test is not something
        a regex can be, so the honest description is: this catches the wordings
        this repository has actually used, and a new wording is a new entry.
        Saying only "a conditional, not a word ban" implied the sole limit was
        timing. Found by review.

        ⚠️ **And each claim is judged against the population it is about**,
        which the first version got wrong and the gate caught on its first
        run. `.agents/skills/README.md` says "scripts these *skills* invoke";
        `AGENTS.md` says "the *standards* name" — different sets, and the
        second sentence is **true** today, because `fuzz.sh` and
        `check-secrets.sh` are named by `security.md` and written by nobody.
        One population for both would have forced a true sentence to be
        deleted, which is the opposite of this check's purpose. ⚠️ Getting the
        population *members* wrong has the same effect — see
        `scripts_named_in`."""

        def scripts_named_in(paths):
            """⚠️ **A bare `` `check-secrets.sh` `` counts too**, not only a
            `scripts/`-prefixed path. `security.md` rule 6 writes it bare, so a
            path-only pattern left it out of `AGENTS.md`'s population — and the
            moment `M2` writes `fuzz.sh`, that population would have been
            entirely present and this check would have demanded the deletion of
            a sentence still true, because `check-secrets.sh` is `M8`'s. Review
            demonstrated it by creating `scripts/fuzz.sh` and watching the gate
            block every commit. A check whose failure mode is "delete the true
            sentence" is worse than no check."""
            found = set()
            for f in paths:
                if not f.exists():
                    continue
                text = f.read_text(encoding="utf-8")
                for m in re.finditer(r"(?:scripts/|`)([A-Za-z0-9_.\-]+\.sh)", text):
                    found.add(m.group(1))
            return found

        standards = sorted((root / "docs" / "internal" / "standards").glob("*.md"))
        skill_named = scripts_named_in(skill_files)
        populations = {
            ".agents/skills/README.md": skill_named,
            "AGENTS.md": scripts_named_in(standards),
        }

        # ⚠️ **The positive, not only the conditional.** The README's claim is
        # "every script these skills invoke now exists", and until this was
        # here that sentence was unchecked in the direction that matters:
        # moving a skill-named script aside left the claim false and the gate
        # green. A *standard* may name a script nobody has written — that is
        # what `roadmap.md`'s deferral table is for — but a **skill** naming
        # one cannot run, so its absence is a defect rather than a schedule.
        # Found by review, which observed the gate passing on exactly that.
        for n in sorted(skill_named):
            if not (root / "scripts" / n).exists():
                problems.append(
                    f"a SKILL.md invokes scripts/{n}, which does not exist -- "
                    f"a skill naming a script nobody wrote cannot run"
                )
        claims = (
            r"still unwritten",
            r"are still missing",
            r"were never written",
            r"a few scripts .{0,30}invoke",
        )
        total = set()
        for rel, named in populations.items():
            total |= named
            if any(not (root / "scripts" / n).exists() for n in named):
                continue          # the sentence has something real to refer to
            path = root / rel
            if not path.exists():
                continue
            text = path.read_text(encoding="utf-8")
            for pat in claims:
                m = re.search(pat, text, re.I)
                if m:
                    line = text[: m.start()].count("\n") + 1
                    problems.append(
                        f"{rel}:{line}: says a script is missing, and all "
                        f"{len(named)} it refers to exist -- M0.1's defect, again"
                    )
                    break     # one report per file; several patterns hit one sentence
        return len(total)

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

    # --- check 3: no index file calls a present script missing -------------
    #
    # ⚠️ Reads `AGENTS.md` as well as `.agents/skills/README.md`, and judges
    # each against its own population — see the docstring.
    n_named = check_missing_script_claims(skill_files)
    if n_named == 0:
        problems.append(
            "no script is named by any SKILL.md or standard -- check 3 inspected nothing"
        )
    checked += 1

    known_skills = {f.parent.name for f in skill_files}

    # --- check 4: every .claude/commands/*.md is a pointer -----------------
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
  note "neither AGENTS.md nor .agents/skills/README.md claims a script is missing that is present"
elif (( rc != 0 )); then
  # ⚠️ Fails closed on anything the checker itself did not choose to report --
  # the same reason check-readmes.sh and check-unsafe.sh treat an unexpected
  # exit as a failure rather than a silent pass.
  fail "portability checker died (exit $rc); the agent system was not checked"
else
  ok "agent system is tool-portable (${checked:-0} file(s) checked)"
fi

finish
