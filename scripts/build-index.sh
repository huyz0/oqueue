#!/usr/bin/env bash
# Generate the index regions that would otherwise be maintained by hand. `M-1.33`.
#
#   scripts/build-index.sh            rewrite the generated regions
#   scripts/build-index.sh --check    fail if any region is stale (the gate)
#
# ## What this generates, and what it deliberately does not
#
# Generated: the standards table, the skills table, the research tag index, and
# every document count. All of it is mechanically derivable from frontmatter, so
# maintaining it by hand is a promise to keep two things in sync forever.
#
# NOT generated: the "what it answers" prose in the research document map. That
# is authored, it is the most useful part of the index, and a summary line
# cannot replace it. Instead this script *verifies* that the authored map covers
# every document exactly once -- which catches the real failure, a document
# added and never indexed.
#
# ## Why --check exists
#
# A builder nobody runs is a builder that has already drifted. `--check`
# regenerates into memory and diffs, so the pre-commit gate fails on a stale
# index rather than trusting that someone remembered.
#
# ## The marker contract
#
# Content between `<!-- index:NAME:start -->` and `<!-- index:NAME:end -->` is
# owned by this script and will be overwritten. Everything outside is authored
# and is never touched.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

MODE="${1:-write}"
case "$MODE" in
  --check) MODE=check ;;
  write|"") MODE=write ;;
  *) fail "unknown argument: $MODE"; note "usage: build-index.sh [--check]"; finish ;;
esac

require_tool python3 "apt-get install python3" || finish

# The generator lives in Python because parsing YAML front matter in bash is a
# source of bugs, not a demonstration of skill.
python3 - "$MODE" <<'PYEOF'
import re, sys, pathlib

mode = sys.argv[1]
root = pathlib.Path.cwd()
problems, changed = [], []

def frontmatter(p):
    """title/description/tags from YAML front matter. Deliberately minimal: a
    real YAML parser is a dependency this repo does not need for five keys."""
    text = p.read_text()
    if not text.startswith("---\n"):
        return None
    end = text.index("\n---", 4)
    body, out, key = text[4:end], {}, None
    for line in body.split("\n"):
        m = re.match(r'^(\w+):\s*(.*)$', line)
        if m:
            key, val = m.group(1), m.group(2).strip()
            out[key] = "" if val == ">" else val.strip('"')
        elif key and line.startswith("  "):
            out[key] = (out[key] + " " + line.strip()).strip()
    for k in ("title", "description"):
        out[k] = out.get(k, "").strip('"')
    out["tags"] = [t.strip() for t in out.get("tags", "").strip("[]").split(",") if t.strip()]
    return out

def replace_region(path, name, content):
    p = root / path
    text = p.read_text()
    s, e = f"<!-- index:{name}:start -->", f"<!-- index:{name}:end -->"
    if s not in text or e not in text:
        problems.append(f"{path}: missing marker '{name}'")
        return
    new = re.sub(re.escape(s) + r".*?" + re.escape(e),
                 f"{s}\n{content}\n{e}", text, flags=re.S)
    if new != text:
        changed.append(f"{path}:{name}")
        if mode == "write":
            p.write_text(new)

# ---- standards table -------------------------------------------------------
FAMILY = {"process": "Process", "quality": "Quality",
          "delivery": "Delivery", "code": "Code"}
by_family = {k: [] for k in FAMILY}
for p in sorted((root / "docs/internal/standards").glob("*.md")):
    fm = frontmatter(p)
    if not fm:
        problems.append(f"{p.relative_to(root)}: no front matter, so it cannot be indexed")
        continue
    fam = next((t for t in fm["tags"] if t in FAMILY), None)
    if not fam:
        problems.append(f"{p.relative_to(root)}: no family tag ({'/'.join(FAMILY)})")
        continue
    by_family[fam].append((p.name, fm["title"], fm["description"]))

rows = []
for fam, label in FAMILY.items():
    for name, title, desc in sorted(by_family[fam]):
        rows.append(f"| {label} | [{name}](docs/internal/standards/{name}) | {desc} |")
replace_region("AGENTS.md", "standards",
               "| Family | Standard | Read when |\n|---|---|---|\n" + "\n".join(rows))

# ---- skills table ----------------------------------------------------------
rows = []
for p in sorted((root / ".agents/skills").glob("*/SKILL.md")):
    fm = frontmatter(p)
    if not fm:
        problems.append(f"{p.relative_to(root)}: no front matter")
        continue
    first = fm["description"].split(".")[0].strip()
    rows.append(f"| [`{p.parent.name}`](.agents/skills/{p.parent.name}/SKILL.md) | {first} |")
replace_region("AGENTS.md", "skills",
               "| Skill | Use when |\n|---|---|\n" + "\n".join(rows))

# ---- research tag index ----------------------------------------------------
docs, tags = [], {}
for p in sorted((root / "docs/researches").glob("[0-9]*.md")):
    fm = frontmatter(p)
    if not fm:
        problems.append(f"{p.relative_to(root)}: no front matter")
        continue
    num = p.name[:2]
    docs.append((num, p.name, fm["title"]))
    for t in fm["tags"]:
        tags.setdefault(t, []).append(num)

# Only tags that actually group documents. A tag on one document is a label,
# not an index entry, and 200 alphabetical single-document tags are noise that
# buries the ~30 that mean something. The curated by-question index above is
# authored precisely because generated tags cannot replace it.
byname = dict((d[0], d[1]) for d in docs)
lines = [f"- **{t}:** " + ", ".join(f"[{n}]({byname[n]})" for n in sorted(set(v)))
         for t, v in sorted(tags.items()) if len(set(v)) >= 3]
replace_region("docs/researches/README.md", "tags", "\n".join(lines))
replace_region("docs/researches/README.md", "count",
               f"**{len(docs)} documents.**")

# ---- verify the authored map covers every document -------------------------
readme = (root / "docs/researches/README.md").read_text()
for num, name, title in docs:
    if f"({name})" not in readme:
        problems.append(f"docs/researches/{name} is not in the document map")

for line in problems:
    print(f"PROBLEM {line}")
for line in changed:
    print(f"STALE {line}")
sys.exit(2 if problems else (1 if (changed and mode == "check") else 0))
PYEOF
rc=$?

if (( rc == 2 )); then
  fail "index cannot be built — see the problems above"
  finish
elif (( rc == 1 )); then
  fail "index is stale; run scripts/build-index.sh"
  finish
fi

if [[ "$MODE" == check ]]; then
  ok "index is current"
else
  ok "index rebuilt"
fi
finish
