"""Kill the brokers a harness leg started, when the leg is stopped.

⚠️ **`run_bounded` signals the direct child only**, which its own header in
`scripts/lib.sh` records as the first of three limits `timeout(1)` does not
have. `M4.53` made that limit reachable: three legs — this module's three
importers — start an `oqueue serve` of their own, and a ceiling that fires
sends `SIGTERM` to *python*, not to the broker. Python's default `SIGTERM`
disposition terminates the interpreter without unwinding, so the `finally`
that each leg already has around `proc.kill()` never runs and the broker is
reparented and left behind. `M4.61`.

⚠️ **The leak is on the failure path, which is where it is least likely to
be noticed**: the gate has already gone red, and the orphan accumulates
against the next run rather than announcing itself. It holds no fixed port
— every one of these brokers binds `127.0.0.1:0` — so what it costs is
processes and memory, not a bind conflict.

## What this does not cover, and why it is written down

⚠️ **A python stopped inside librdkafka's C teardown cannot run this.**
`M4.38` measured exactly that hang: an assertion escapes with a client
alive and the interpreter never returns from the C call at exit. Python
runs signal handlers only between bytecodes, so `SIGTERM` is noted and not
delivered, `run_bounded` escalates to `SIGKILL` two seconds later, and
nothing here runs at all. That is the case `M4.61`'s row calls the residue;
it is narrower than the one this closes, because it needs the leg to be
wedged in C *and* the ceiling to fire, where the common case is a leg that
is merely slow or a broker that never becomes ready.
"""

from __future__ import annotations

import atexit
import signal
import subprocess
import sys

_tracked: list[subprocess.Popen] = []


def track(proc: subprocess.Popen) -> subprocess.Popen:
    """Registers `proc` to be killed if this process is stopped or exits."""
    _tracked.append(proc)
    return proc


def _kill_tracked() -> None:
    for proc in _tracked:
        if proc.poll() is None:
            try:
                proc.kill()
            except OSError:
                # Already gone, or never really started. Either way there is
                # nothing left to reap and nothing to report.
                pass


def _on_terminate(_signum: int, _frame: object) -> None:
    # ⚠️ **Kill first, then unwind, and the order is the whole of `M4.61`'s
    # blocking finding.** Leaving the kill to the legs' own `finally` blocks
    # and to `atexit` defers it behind the *entire* unwind — and the
    # innermost `finally` in these legs is client teardown, not broker
    # teardown: librdkafka's `close()` blocks for seconds against a broker
    # that has stopped answering, which is exactly when a ceiling fires.
    # `_run_bounded_stop` allows two seconds between `SIGTERM` and
    # `SIGKILL`, so python was killed mid-unwind and the broker orphaned
    # anyway. Measured: an inner `finally` sleeping five seconds orphaned
    # the child; at zero seconds it was reaped. Killing here costs nothing
    # the orderly close was buying, because the broker is going away either
    # way.
    _kill_tracked()
    # ⚠️ **`SystemExit`, not `os._exit`.** Raising still unwinds, so each
    # leg's own `finally` runs and closes its clients — and `atexit` is the
    # backstop for a path that never reaches here. `128 + SIGTERM`, the
    # shell's own convention, so `run_bounded`'s caller sees a terminated
    # run rather than a failure.
    raise SystemExit(143)


signal.signal(signal.SIGTERM, _on_terminate)
atexit.register(_kill_tracked)

if __name__ == "__main__":
    # ⚠️ **Self-test, because a reaper nobody exercises is the shape this
    # repository keeps finding.** Starts a long `sleep` under `track`, sends
    # this process `SIGTERM`, and reports whether the child outlived it.
    # `tests/gates/negative.sh` drives this rather than a real broker: the
    # property is about the signal path, and `oqueue serve` would need a
    # built workspace the negative suite does not have.
    import os
    import time

    # ⚠️ **`REAP_SELFTEST_TRACK=0` runs the same body *without* tracking**,
    # and the suite drives both arms. "The child did not outlive us" proves
    # nothing on its own — a child that was never started does not outlive
    # anything either — so the untracked arm is what shows the leak is real
    # and that `track` is what closes it. ⚠️ Both arms only work because the
    # child's stdout is `DEVNULL` above; with it inherited the untracked arm
    # hangs rather than reporting, which is how it came to be dropped once.
    # ⚠️ **`DEVNULL`, or the child holds the caller's stdout open.** A
    # `sleep` inheriting this process's stdout keeps a command substitution
    # around it from ever returning, so a suite case reading `$( ... )`
    # hangs for the child's whole lifetime instead of sampling liveness and
    # failing. That is what made an earlier untracked arm look as though the
    # child died with its parent — it did not; the case simply never got to
    # look. Found by review of `M4.61`.
    child = subprocess.Popen(["sleep", "607"], stdout=subprocess.DEVNULL)
    if os.environ.get("REAP_SELFTEST_TRACK", "1") == "1":
        track(child)
    print(child.pid, flush=True)
    # ⚠️ **`REAP_SELFTEST_SLOW=1` waits for its caller's signal instead of
    # raising its own, and puts a slow `finally` behind it.** That is the
    # only arm that can see whether the handler kills *before* it raises:
    # the other two give the child a `sleep` with nothing to unwind, so a
    # reaper deferred to `atexit` looks identical to one that kills first —
    # and the ordering is `M4.61`'s own blocking finding. Review caught that
    # the fix for it was guarded by nothing.
    #
    # ⚠️ **The `finally` stands in for librdkafka's `close()`**, which blocks
    # for seconds against a broker that has stopped answering, which is
    # exactly when a ceiling fires. The caller sends `SIGTERM`, waits, then
    # `SIGKILL` — `_run_bounded_stop`'s own escalation — so an `atexit`-only
    # reaper is overtaken and never runs at all.
    if os.environ.get("REAP_SELFTEST_SLOW") == "1":
        try:
            time.sleep(300)
        finally:
            time.sleep(10)
    os.kill(os.getpid(), signal.SIGTERM)
    # Reached only when the handler did not raise: `SIGTERM` was noted and
    # never delivered, which is the C-teardown case this module's doc names
    # as the residue it cannot cover.
    time.sleep(5)
    print("HANDLER DID NOT FIRE", flush=True)
    sys.exit(1)
