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
M13.15 — one additional review round is authorized because the Windows staged-scratch packet reported failures that were green in the prescribed Linux container; the change remains one aggregate-evidence retention task, and this grant expires with its commit — approved-by: user explicit unblock authorization on 2026-09-23; scope: M13.15 review round four only; expires: 2026-09-23
M13.17 — one additional review round is authorized to adjudicate the repeated explicit-docker.io finding, which the prior reviewed diff already addressed and the negative fixture exercises; the M13.17 portability fixes remain one task — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round four only; expires: 2026-09-23
M13.17 — one additional review round is authorized to verify the expanded, source-level evidence for the same Docker Hub matcher argument; the M13.17 gate-portability change remains one task — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round five only; expires: 2026-09-23
M13.17 — one additional review round is authorized because the full commit hook reproduced lld thread exhaustion at four coverage jobs, requiring a verified serial coverage build; this remains the same final-gate portability task — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round six only; expires: 2026-09-23
M13.17 — one additional review round is authorized to verify the acceptance split: portable gate fixes in M13.17 and post-commit release evidence plus milestone closure in M13.18 — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round seven only; expires: 2026-09-23
M13.17 — one additional review round is authorized to verify source and regression evidence in the changed-delta packet, and the corrected acceptance boundary — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round eight only; expires: 2026-09-23
M13.17 — one additional review round is authorized to verify the post-commit task-state correction in the same unpushed M13.17 commit; no source scope changes — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round nine only; expires: 2026-09-23
M13.17 — one additional review round is authorized to verify durable evidence for the already-passed commit hook before finalizing the local task commit — approved-by: user explicit full-authority unblock instruction on 2026-09-23; scope: M13.17 review round ten only; expires: 2026-09-23
