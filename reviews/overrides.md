# Review round overrides

A task appears here only when its review went past the cap in
[`review.md`](../docs/internal/standards/review.md) rule 15a — three rounds —
and someone decided that was right. One line each, newest last:

```
<task id> — <why this is genuinely one change rather than two> — approved-by: <name>
```

⚠️ **This file is the whole mechanism, and its value is that it is a diff.**
The cap it guards has been advisory since `ADR-0016` set it: `M5.50` spent five
rounds, `M5.4` four, and nothing anywhere records that either happened. An
escape nobody but the agent taking it can see is not an escape, it is the
absence of a rule — which is why the cap never once caused a change to be split.
A line here is read by whoever reads the commit.

⚠️ **A blocking finding does not need a line.** Rule 15a lifts the cap for
correctness, and it always has. What needs a line is a fourth round spent on
anything else: a `major` on prose, a fix that turned out to need its own fix, a
reviewer and an author who disagree about severity.

⚠️ **A `minor` may never buy a round at all**, with or without a line. Rule 15
forbids it, and this file is not a way around that — a signature attesting that
a round was spent on a minor is a signature that the rule was broken.

## What signing means

The name on a line is the person who decided, not the agent that asked. An
agent running the milestone loop autonomously has no standing authority to sign
one on anyone's behalf; absent an explicit grant, a task that reaches a fourth
round **stops and reports** (the `milestone` skill's "stop and ask when"). A
grant, if one is given, is recorded here with its date, its scope and its
expiry, so that a reader can tell a signature that was earned from one that was
assumed.

⚠️ **No standing authority is in force.** A scoped grant recorded below is an
explicit exception for that task and does not create standing authority; it
must name its scope and expiry. A signed line with no such grant is still a
contradiction.

## Overrides

M12.3 — additional review rounds are required to verify the durable ownership and duplicate-create fixes raised by independent review; the final change is one coherent catalog-contract task and user goal continuation authorizes completing it — approved-by: user goal continuation; scope: M12.3 review rounds; expires: 2026-09-21
