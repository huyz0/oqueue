# Skills

Procedures, written to the [Agent Skills](https://agent-skills.org) spec so any
tool that reads `SKILL.md` can use them. Claude-specific files under `.claude/`
are **thin adapters that delegate here** — they never contain logic of their own.

## Progressive disclosure

The problem: this repository holds ~110,000 words of research, seven standards,
and a product spec. Loading that into every session is impossible and would be
useless if it were possible. The answer is four layers, each loaded only when
the one above says it is relevant.

| Layer | What | Loaded |
|---|---|---|
| **0** | [`AGENTS.md`](../../AGENTS.md) — an index, deliberately | every session |
| **1** | The `description:` line of every skill below | every session (a few hundred words total) |
| **2** | A skill's body | when that skill is invoked |
| **3** | Standards, product docs, research documents | when a skill says to read one |

⚠️ **Layer 1 is the whole mechanism.** A skill's `description` is the only thing
an agent sees before deciding to load it, so it must say *when to use this*, not
*what this is*. "Write an ADR" is a title; "use when making a choice that is
expensive to reverse" is a description that gets the skill loaded at the right
moment.

**Layer 3 is never loaded wholesale.** The research corpus has a README with a
document map and a tag index; the `research` skill exists to teach navigating it
rather than reading it.

## The rules

1. **Skills call scripts in `scripts/`, never a tool-specific built-in.** The
   script is the enforcement path and it must work for a developer on any tool.
2. **`SKILL.md` frontmatter is `name` and `description`, both required**, and
   the file must parse as YAML front matter followed by Markdown. A tool that
   cannot parse it ignores the skill silently.
3. **No vendor-specific syntax in `AGENTS.md` or in any `SKILL.md`.** Claude
   Code's `@import` belongs in `CLAUDE.md`, which is the adapter. → `check-portability.sh`
4. **A skill is a procedure, not an explanation.** Rationale lives in the
   standards and the corpus; the skill says what to do and links to why.
5. **Adapters stay thin.** A `.claude/commands/*.md` file that contains a
   procedure rather than a pointer is a fork waiting to drift.

## The skills

| Skill | Use when |
|---|---|
| [`goal`](goal/SKILL.md) | Driving a milestone to completion without a human in the loop |
| [`next-task`](next-task/SKILL.md) | Starting work and needing to know what to do next |
| [`spec`](spec/SKILL.md) | A milestone or feature needs specifying before code |
| [`tdd`](tdd/SKILL.md) | Implementing a task |
| [`review`](review/SKILL.md) | A change is staged and needs an independent reviewer |
| [`adr`](adr/SKILL.md) | Making a choice that is expensive to reverse |
| [`research`](research/SKILL.md) | A question might already be answered in the corpus |

## ⚠️ Bootstrap state

Several skills invoke scripts that **do not exist yet** — the remaining gates
are M-1.7 through M-1.12. Until those land, a skill that says "run the gate"
describes an intended step rather than an available one, and the honest response
is to say the gate did not run, not to proceed as though it passed.

`review` is no longer in that state: `scripts/review.sh` and
`scripts/check-reviewed.sh` exist and its verdict is bound to the staged diff by
hash.

See [backlog.md](../../docs/internal/product/backlog.md).
