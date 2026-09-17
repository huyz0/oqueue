# How many rounds a task has had, and what the last one left open.
#
# ⚠️ **Why this exists.** `M5.50` took five review rounds on a commit that
# changed one Markdown file, and every round after the first was opened for a
# finding that never blocked. `review.md` rule 15 already said a `minor` on a
# `pass` is recorded rather than re-reviewed; nothing said it at the moment the
# decision is made. A rule an agent has to recall many turns after reading it is
# the weakest place to put one, so the packet prints it — with the round number
# beside it, which is a fact only the verdict files hold.
#
# A round is a **distinct staged hash reviewed for the same task**. Counting
# verdict files instead would count a second opinion on one diff as a round.
#
# ⚠️ **Nothing here fails closed.** An unreadable verdict, an absent directory
# or a verdict missing `task_id` is skipped, because this feeds a packet rather
# than a gate: the caller's fallback is to show the reviewer *more*, and less
# review is never the failure-safe default.
"""Rounds a task has had, and the findings its earlier rounds left open."""

import glob
import json
import os
import sys

SEVERITIES_THAT_BLOCK = ("blocking", "major")


def _verdicts(review_dir):
    """Every readable verdict, oldest first.

    ⚠️ **By modification time, not by name.** A verdict is named for the sha256
    of the diff it judged, so sorting by filename orders rounds by a hash —
    which is to say arbitrarily. `PREVIOUS` would then name whichever round
    hashed largest, and round three would be handed round one's tree and shown
    edits round two had already cleared. Always a superset, never less, which
    is why it was a minor rather than the failure this must never have.
    """
    paths = glob.glob(os.path.join(review_dir, "*.json"))
    # A file that vanishes between the glob and the stat is skipped, not fatal:
    # `target/` is disposable and this feeds a packet.
    def when(path):
        try:
            return os.path.getmtime(path)
        except OSError:
            return 0.0

    for path in sorted(paths, key=lambda p: (when(p), p)):
        try:
            with open(path, encoding="utf-8") as handle:
                yield path, json.load(handle)
        except (OSError, ValueError):
            continue


def prior_rounds(task, current_sha, review_dir):
    """Verdicts for `task` on hashes other than `current_sha`, oldest first."""
    rounds = {}
    for path, verdict in _verdicts(review_dir):
        if verdict.get("task_id") != task:
            continue
        sha = verdict.get("diff_sha256")
        if not sha or sha == current_sha:
            continue
        rounds.setdefault(sha, (path, verdict))
    # `dict` preserves insertion order, and `_verdicts` inserts oldest first.
    return list(rounds.values())


def open_findings(prior):
    """Blocking and major findings from earlier rounds, newest round last.

    ⚠️ A finding is listed whether or not a later round called it fixed: this
    is what the verify round exists to check, and a reviewer told only about
    what the author says is resolved is reading the author's summary again.
    """
    out = []
    for _, verdict in prior:
        for finding in verdict.get("findings") or []:
            if finding.get("severity") in SEVERITIES_THAT_BLOCK:
                out.append((verdict.get("diff_sha256", "?")[:12], finding))
    return out


def main(argv):
    task, current_sha, review_dir = argv[1], argv[2], argv[3]
    prior = prior_rounds(task, current_sha, review_dir)
    print("ROUND=%d" % (len(prior) + 1))
    if not prior:
        return 0
    last_sha = prior[-1][1].get("diff_sha256", "")
    print("PREVIOUS=%s" % last_sha)
    for sha, finding in open_findings(prior):
        print(
            "FINDING\t%s\t%s\t%s\t%s"
            % (
                finding.get("severity", "?"),
                sha,
                "%s:%s" % (finding.get("file", "?"), finding.get("line", "?")),
                " ".join(str(finding.get("summary", "")).split()),
            )
        )
        scenario = " ".join(str(finding.get("failure_scenario", "")).split())
        if scenario:
            print("SCENARIO\t%s" % scenario)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
