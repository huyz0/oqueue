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
| [`milestone`](milestone/SKILL.md) | Driving a milestone to completion without a human in the loop |
| [`milestone-review`](milestone-review/SKILL.md) | A milestone reaches a checkpoint or its end, and the commits need reading as a whole |
| [`next-task`](next-task/SKILL.md) | Starting work and needing to know what to do next |
| [`spec`](spec/SKILL.md) | A milestone or feature needs specifying before code |
| [`tdd`](tdd/SKILL.md) | Implementing a task |
| [`review`](review/SKILL.md) | A change is staged and needs an independent reviewer |
| [`adr`](adr/SKILL.md) | Making a choice that is expensive to reverse |
| [`contract-change`](contract-change/SKILL.md) | A `pub trait`'s method set in `oqueue-core` is gaining, losing, or changing a method |
| [`research`](research/SKILL.md) | A question might already be answered in the corpus |

## ⚠️ When a skill names a script that is not there

M-1 is complete and every gate it built exists, but a few scripts these skills
invoke were never in M-1's scope and are still unwritten. A skill that says
"run the gate" then describes an intended step rather than an available one,
and **the honest response is to say the gate did not run** — not to proceed as
though it passed. That is the same discipline as `AGENTS.md`'s rule 3, and it
is the one thing no script can check.

⚠️ **This section deliberately names no scripts.** A list here would be a
second copy of a fact that moves, and the version in
[`backlog.md`](../../docs/internal/product/backlog.md) is the one every gate
reads and every task updates. Check `scripts/` for the file before assuming it
is there. ⚠️ A missing script does not guarantee a backlog row either — some
are named by a standard and scheduled nowhere — so "no row" means unscheduled,
not "already done".
