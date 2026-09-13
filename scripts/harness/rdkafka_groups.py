"""librdkafka consumer-group conformance against a broker (`M4.17`, FR-20).

Usage: rdkafka_groups.py <bootstrap host:port>

Drives a real `librdkafka` group through the three membership changes FR-20
names — an initial join, a consumer *added*, and a consumer *removed* — and
asserts after each that the group's partitions are distributed across the
live members with **no partition owned by two of them at once**, which is
FR-20's revoke-before-reassign invariant seen from the client side.

⚠️ **Several single-partition topics, not one multi-partition topic.**
`Cluster::ensure_topic` creates every topic with exactly `partitions: 1` and
this broker dispatches no `CreateTopics`, so a partition count is not
something a client can ask for. Three topics give three assignable units,
which is what makes "distributed without duplicates" a claim with content;
one topic would only ever prove that one consumer got it.

⚠️ **`cooperative-sticky`, because that is the protocol `M4.16` built the
round bookkeeping for**, and the one whose invariant is revoke-before-
reassign rather than stop-the-world.

Prints `GROUP CONFORMANCE OK` on success — the string
`kafka-client-harness.sh` greps for — and exits non-zero on any failure.
"""

import sys
import time

from confluent_kafka import Consumer, Producer

BOOTSTRAP = sys.argv[1]
TOPICS = ["groups-a", "groups-b", "groups-c"]
GROUP = "m4-17-conformance"

# ⚠️ **Both eviction paths are deliberately longer than `SETTLE_SECONDS`,
# and `M4.38` is why.** These were 6 s and 20 s against a 60 s settle
# window, so the "a consumer is removed" stage was satisfied by *any* path
# that eventually removed the member. Measured: replacing `c2.close()` with
# abandoning `c2` — never closed, never polled — still printed
# `GROUP CONFORMANCE OK` after 21.2 s, which is librdkafka's own
# `MAXPOLL ... leaving group` at the 20 s poll interval. ⚠️ Not the broker's
# session sweep, which an earlier version of this note blamed: librdkafka
# heartbeats from its internal thread whether the application polls or not,
# so the 6 s session never expired. The stage as written
# constrained "the group converges after a member disappears", not
# "`LeaveGroup` was handled" — and `M4.10` is specifically `LeaveGroup`
# firing exactly one rebalance. With both timeouts past the window, the only
# thing that can remove a member in time is the `LeaveGroup` `close()`
# sends.
#
# ⚠️ The broker's own floor is 6 s (`MIN_SESSION_TIMEOUT_MS`); this is far
# above it, which costs nothing because nothing here waits out a session.
SESSION_TIMEOUT_MS = 45000
# ⚠️ `max.poll.interval.ms` is what a real client seeds `rebalance_timeout_ms`
# from, and the broker uses that as a round's deadline once a group has a
# roster to wait for. ⚠️ **It was kept modest so a stuck round would fail
# this script rather than hang it, and it never did that** — review of
# `M4.38` stalled a member mid-group and measured the same hang under the
# old 20 s value: `settle` does raise on its own deadline, but the process
# then hangs in librdkafka's teardown at interpreter exit, with or without a
# `finally` that closes. Neither value buys the property that sentence
# claimed, and what a short one did buy was the second eviction path this
# stage must not have. The real repair is a `timeout` around the harness's
# own invocation, which `kafka-client-harness.sh` has for neither script —
# filed rather than done here.
MAX_POLL_INTERVAL_MS = 300000
SETTLE_SECONDS = 30


def consumer(name: str) -> Consumer:
    return Consumer(
        {
            "bootstrap.servers": BOOTSTRAP,
            "group.id": GROUP,
            "client.id": name,
            "auto.offset.reset": "earliest",
            "session.timeout.ms": SESSION_TIMEOUT_MS,
            "max.poll.interval.ms": MAX_POLL_INTERVAL_MS,
            "partition.assignment.strategy": "cooperative-sticky",
            "enable.auto.commit": False,
        }
    )


def owned(c: Consumer) -> set:
    return {(tp.topic, tp.partition) for tp in c.assignment()}


def settle(consumers: dict, stage: str, expect_members: int) -> dict:
    """Polls every consumer until the assignment is complete, checking on
    every pass that no partition is owned by two members at once.

    ⚠️ **The overlap check belongs *here*, not only at the end.** FR-20's
    invariant is that a partition is revoked by its previous owner before it
    is handed on, and that is a claim about every instant of a rebalance, not
    about the state it converges to. A check applied only to the converged
    assignment is implied by this loop's own exit condition and so can never
    fire — which is what an earlier version of this harness did, reporting
    FR-20 verified while sampling the one moment it cannot be violated.
    Found by review.

    ⚠️ **Every consumer must be polled throughout.** librdkafka runs its
    rebalance on the application's own `poll` thread, so a consumer nobody
    polls never completes a join and is eventually evicted — the group would
    then "settle" at the wrong size, against a state this harness's own
    inattention created.
    """
    deadline = time.time() + SETTLE_SECONDS
    last = {}
    samples = 0
    while time.time() < deadline:
        for c in consumers.values():
            c.poll(0.2)
        last = {name: owned(c) for name, c in consumers.items()}
        assert_disjoint(f"{stage} (mid-rebalance)", last, complete=False)
        samples += 1
        union = set().union(*last.values()) if last else set()
        total = sum(len(v) for v in last.values())
        members_with_work = sum(1 for v in last.values() if v)
        if total == len(union) == len(TOPICS) and members_with_work == min(
            expect_members, len(TOPICS)
        ):
            print(f"{stage}: settled after {samples} sample(s)")
            return last
    raise AssertionError(
        f"group never settled within {SETTLE_SECONDS}s with {expect_members} "
        f"member(s); last assignment {last}"
    )


def assert_disjoint(stage: str, assignment: dict, complete: bool = True) -> None:
    """No partition owned twice — and, once settled, none owned by nobody.

    `complete=False` is the mid-rebalance form: a partition legitimately
    belongs to nobody for the window between its owner revoking it and its
    new owner being assigned it, which is precisely what cooperative
    rebalancing buys. Owning it *twice* is never legitimate, at any instant.
    """
    seen = {}
    for name, tps in assignment.items():
        for tp in tps:
            if tp in seen:
                raise AssertionError(
                    f"{stage}: {tp} assigned to both {seen[tp]} and {name} — "
                    "FR-20's revoke-before-reassign invariant violated"
                )
            seen[tp] = name
    if not complete:
        return
    missing = {(t, 0) for t in TOPICS} - set(seen)
    if missing:
        raise AssertionError(f"{stage}: {missing} assigned to nobody")
    print(f"{stage}: {[(n, sorted(v)) for n, v in sorted(assignment.items())]}")


def main() -> None:
    producer = Producer({"bootstrap.servers": BOOTSTRAP, "message.timeout.ms": 10000})
    for topic in TOPICS:
        for i in range(3):
            producer.produce(topic, f"{topic}-{i}".encode())
    producer.flush(10)
    print(f"produced to {len(TOPICS)} topics")

    consumers = {}
    try:
        # ── join: two consumers, three partitions ───────────────────────────
        for name in ("c1", "c2"):
            consumers[name] = consumer(name)
            consumers[name].subscribe(TOPICS)
        assert_disjoint("join", settle(consumers, "join", 2))

        # ── a consumer is added ─────────────────────────────────────────────
        consumers["c3"] = consumer("c3")
        consumers["c3"].subscribe(TOPICS)
        assert_disjoint("added", settle(consumers, "added", 3))


        # ── a consumer is removed ───────────────────────────────────────────
        # ⚠️ `close()` sends `LeaveGroup`, and that is the whole of what this
        # stage tests: both eviction timeouts are past `SETTLE_SECONDS`, so a
        # member that merely stopped polling would still be in the group when
        # `settle` gives up. See the timeout constants.
        consumers.pop("c2").close()
        assert_disjoint("removed", settle(consumers, "removed", 2))
    finally:
        for c in consumers.values():
            try:
                c.close()
            except Exception:  # noqa: BLE001 - cleanup must not mask a failure
                pass

    print("GROUP CONFORMANCE OK")


main()
