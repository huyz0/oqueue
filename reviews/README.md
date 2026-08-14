# Milestone review verdicts

One JSON file per cross-cutting review, named
`milestone-<milestone>-<sha256 of the commit set>.json`. Written by
[`scripts/milestone-review.sh record`](../scripts/milestone-review.sh) and read
by [`scripts/check-milestone-review.sh`](../scripts/check-milestone-review.sh).

⚠️ **Tracked, unlike the per-commit reviewer's artifacts** in gitignored
`target/review/`. That asymmetry is deliberate. A per-commit verdict is keyed to
a staged diff that is about to become a commit, and it is consumed on the
machine that produced it. A milestone's coverage is the opposite: a durable
claim about history, which a second agent, a fresh clone, and CI all have to be
able to check. On disk under `target/` it did not survive `cargo clean`.

⚠️ **The gate reads these from the index, not from disk**, for the same reason
[`baselines/review.txt`](../baselines/review.txt) is read that way: a file that
counts while unstaged rewards the path that leaves no trace in history.

Do not hand-edit. `record` validates the commit list against real history; the
gate trusts what it finds here.

See [`.agents/skills/milestone-review`](../.agents/skills/milestone-review/SKILL.md)
and [docs/researches/21](../docs/researches/21-ai-development-loop.md) §8.
