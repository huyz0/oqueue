---
name: tdd
description: Implement a task test-first. Use when writing any code. Covers the red-green cycle, what to assert, and the rules that keep the resulting test worth having.
---

# Test-driven implementation

## The cycle

1. **Write the failing test first.**
2. **Run it and watch it fail.** ⚠️ A test never observed to fail has not been
   shown to test anything — it may pass for reasons unrelated to the code.
3. **Implement the smallest thing that passes.**
4. **Run it and watch it pass.**
5. Refactor with the test green.

## What to assert

- **Behaviour, never implementation.** A test that breaks on refactor and passes
  when behaviour breaks is worse than none.
- **The acceptance criterion from the task**, in terms a reader recognises.
- Name the test for the behaviour: `fetch_at_high_watermark_issues_no_gets`, not
  `test_fetch_2`.

## Tier

Default to **T0** — pure logic, in-memory fakes, no I/O. If the test seems to
need a socket, a clock, or an object store, ⚠️ **that is a signal the logic is
in the wrong layer**, not a reason to move up a tier. Inject the seam instead.

See [testing.md](../../../docs/internal/standards/testing.md) for the tiers and
the no-flake rules. The ones broken most often:

- **No `sleep`.** Use `tokio::time::pause()` / `advance()`.
- **No fixed ports.** Bind `:0`.
- **Scratch under `target/tmp`**, never the system temp directory.
- **Prefer a fake over a mock.**

## Before you call it done

- Every acceptance criterion **observed** to be met, not expected to be.
- `scripts/check-crate.sh <crate>` green.
- ⚠️ Consider running scoped mutation testing on what you wrote:
  `scripts/mutants.sh <crate>`. A surviving mutant means the test executes the
  line without constraining it — the characteristic failure of generated tests.
