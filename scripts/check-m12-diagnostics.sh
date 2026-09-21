#!/usr/bin/env bash
# M12.16: the five FR-52 diagnostic scenarios have actionable runbooks.
# The checker keeps the acceptance contract structural; the scenario review
# itself supplies the operational judgement about whether the signals truly
# distinguish the injected causes.
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

cd "$REPO_ROOT"

require_python || finish

DOC="docs/internal/operations/m12-diagnostics.md"
if [[ ! -f "$DOC" ]]; then
  fail "$DOC is missing"
  finish
fi

python3 - "$DOC" <<'PYEOF' || fail "M12 diagnostic scenario review failed"
import pathlib
import re
import sys

path = pathlib.Path(sys.argv[1])
text = path.read_text(encoding="utf-8")
required = {
    "stalled partition": ["lag", "storage_failures", "produce", "fetch"],
    "lagging consumer group": ["DescribeGroups", "partitions", "lag", "assignment"],
    "coordinator failover in progress": [
        "coordinator_ready", "coordinator_failures", "coordinator_task_alive", "group_log"
    ],
    "compaction backlog": ["compaction_backlog", "index_entries", "storage_failures", "capacity"],
    "KMS outage or key revocation": [
        "key_domain_failures", "encryption_cache_hits", "encryption_cache_misses", "kms", "unrelated"
    ],
}
labels = ("**Injection.**", "**Signals.**", "**Query steps.**", "**Distinguishing observation.**")
problems = []

if "without consulting implementation source" not in re.sub(r"\s+", " ", text):
    problems.append("the document does not state that review is possible without implementation source")
if "dropped_scoped_samples" not in text:
    problems.append("the bounded scoped-sample caveat is missing")

sections = {}
for match in re.finditer(r"^## Scenario: (.+)$", text, re.MULTILINE):
    start = match.end()
    next_heading = re.search(r"^## ", text[start:], re.MULTILINE)
    end = start + next_heading.start() if next_heading else len(text)
    sections[match.group(1).strip()] = text[start:end]

for name, tokens in required.items():
    body = sections.get(name)
    if body is None:
        problems.append(f"missing scenario section: {name}")
        continue
    for label in labels:
        if label not in body:
            problems.append(f"{name}: missing {label}")
    for token in tokens:
        if token.lower() not in body.lower():
            problems.append(f"{name}: missing required signal/query token {token}")

if problems:
    for problem in problems:
        print(f"FAIL {problem}", file=sys.stderr)
    raise SystemExit(1)

print(f"ok five M12 diagnostic scenarios have injection, signals, queries, and distinguishing observations")
PYEOF

finish
