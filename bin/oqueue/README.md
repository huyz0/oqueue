# `oqueue` (the binary)

## What is it?

The broker's composition root: the process that starts, chooses which concrete
implementation sits behind each `oqueue-core` seam, and hands the result to the
I/O shell.

## Why does it exist?

Because a library crate here does not reach for S3, a socket or a clock
directly — it takes a trait and lets something else choose (NFR-51) — and
something has to make that choice. ⚠️ **Which is a rule with named exceptions,
not a property of every crate.** `scripts/check-sans-io.sh` draws the line, and
what it actually exempts is: `oqueue-store` from the object-storage pattern,
because it is the crate that implements those backends; and `oqueue-broker`
from all three, because the I/O shell is meant to live there. Every other
crate is held to all three — ⚠️ but `bin/` is not scanned at all, so nothing
here is held to any of them; this crate's own Invariants table says so too.

⚠️ This sentence claimed *every* library crate until `M1.28`. `oqueue-store`
had falsified it since `M1.15` gave it a real S3 backend — not, as that row
predicted, the connection loop.

⚠️ **This is the only place that choice is made** (FR-50) — which is what lets
the entire system be tested with no network, no credentials and no container.

## Upstream

- `oqueue-core` — the seams and the types.
- `oqueue-crypto` — `NoOpKeyProvider`, the default when no KMS is configured.
- `oqueue-broker` — the I/O shell `serve` composes: connection task, dispatcher, cluster (`M2.25`, pointed at the real write and read paths by `M3.14`).
- `oqueue-coordinator` — the offset sequencer `serve` opens and whose loop it spawns. ⚠️ The loop is spawned *here* because `async-concurrency.md` rule 13 wants an owner that can observe a task, and the process is that owner.
- `oqueue-index` — `MemoryIndex`, the materialization chosen here. ⚠️ `M3`'s answer, not the project's: doc 10 #12's disk engine is open, and choosing in a composition root is what makes swapping it one line.
- `oqueue-store` — `S3Store` and `GcsStore`, selected by `OQUEUE_STORE`. ⚠️ **Unset means an in-memory store, which is not durable** — the banner names it and `serve` warns that every acknowledged record is lost at exit. ⚠️ **A value that is neither `s3` nor `gcs` is refused**, not defaulted: `OQUEUE_STORE=S3` is a typo, and a typo must not start a broker that loses records.
- `tokio` — the runtime under `serve`'s listener; `net` arrived exactly when this binary bound one.
- `tokio-rustls` — the acceptor `serve` terminates TLS with (`M4.18`). ⚠️ **No new C toolchain**: the `ring` provider `ADR-0012` chose is already here beneath `oqueue-crypto` and `oqueue-store`, and this manifest pulls the same one.
- `thiserror` — the security configuration's own error type, spelled the way every other crate here spells one.
- `mimalloc` — the global allocator. ⚠️ C, compiled by `cc` at build time;
  within NFR-42 ("cargo and a C compiler") and recorded in ADR-0007.
- `tikv-jemallocator` — **optional**, behind the non-default `heap-profiling`
  feature. ⚠️ It bakes in the *build host's* page size, so a binary built on a
  4 KB-page host aborts at startup on a 64 KB-page aarch64 kernel. Build with
  `JEMALLOC_SYS_WITH_LG_PAGE=16` when those hosts are in scope. ADR-0007.

⚠️ **A composer.** `check-layering.sh`'s `COMPOSERS` set is
`{oqueue-broker, oqueue}`; every other crate may depend on `oqueue-core` and
nothing else here.

## Security configuration

⚠️ **Every source is named by an environment variable and never sniffed, and a
malformed one stops the process** — `behavior.md` rule 8, and `OQUEUE_STORE`'s
own argument. A typo'd credential path that fell back to "no credentials"
would start a broker that accepts every client, which from outside looks
identical to one configured correctly.

| Variable | Holds | Absent means |
|---|---|---|
| `OQUEUE_TLS_CERT`, `OQUEUE_TLS_KEY` | paths to a PEM certificate chain and private key | cleartext, warned about. ⚠️ **Both or neither** — one alone is refused, because falling back to cleartext on a typo'd path starts exactly the broker the operator was avoiding |
| `OQUEUE_CREDENTIALS` | a file of `principal:password` lines | no authentication, and authorization fails **open**. ⚠️ Requires TLS: `SASL/PLAIN` is only accepted inside a TLS session (`ADR-0032`), so credentials without TLS is refused rather than started — it would serve nobody |
| `OQUEUE_TOPIC_GRANTS` | a file of `principal:topic` lines, one grant each | no grants. With credentials configured that refuses every authenticated client every topic, so it is warned about |
| `OQUEUE_GROUP_GRANTS` | a file of `principal:group` lines, one group-ownership grant each | no group ownership. With credentials configured, group operations deny by default, so it is warned about |
| `OQUEUE_ADMIN_GRANTS` | a file of `principal:operation` lines, one administrative grant each. Supported operations are `create_topics`, `delete_topics`, `describe_configs`, `alter_configs`, `describe_groups`, `list_groups`, and `alter_quotas` | no administrative authority. With credentials configured, administrative operations deny by default, so it is warned about |
| `OQUEUE_MAX_IN_FLIGHT` | a positive integer | no per-principal quota. ⚠️ Inert without credentials, since the quota keys on the authenticated principal — and warned about |

⚠️ **File format**: `name:value`, split on the **first** colon (a password may
contain one; a principal may not). ⚠️ **Whole-line `#` comments** and blank lines are ignored — a `#` *after* a value is part of that value, so `alice:secret # prod` sets the password to `secret # prod` and the broker then refuses the operator's own client while the file reads correctly. A password may legitimately contain `#`, which is why it cannot be stripped.
⚠️ **Surrounding whitespace *is* stripped**, on both the name and the value:
`alice:s3cret ` sets the password to `s3cret`. That is almost always what an
operator meant, but it is equally invisible in the file — and a password that
was meant to end in a space is then refused with the deliberately
uninformative `SASL/PLAIN authentication failed`.
⚠️ **One grant per line**: `alice:orders,payments` names a single topic called
`orders,payments`, not two — and the client is then refused with nothing
saying why. ⚠️ **A named source that parses to nothing is refused** rather
than treated as absent: an empty credential file would otherwise mean "no
authentication at all". Administrative grants use the same one-entry-per-line
rule, for example `alice:create_topics`; topic visibility never grants
administrative authority.


## Downstream

Nothing. It is the top of the graph.

## Invariants

| Must stay true | Held by |
|---|---|
| The only place the *composition* is chosen — which concrete implementation the running broker gets | ⚠️ **No gate.** `check-sans-io.sh` does not scan `bin/`, so this is review's. ⚠️ Narrowed by `M1.28` from "the only place a concrete implementation is named", which `oqueue-store` falsifies: it names `S3Store` and `GcsStore` because it defines them |
| No `oqueue-testkit` in `[dependencies]` | `scripts/check-layering.sh` — ⚠️ the *only* dependency rule it enforces here, since a composer is exempt from the star-topology one |
| Depends only on `oqueue-core` and the crates it composes | ⚠️ **No gate.** Composer exemption means `check-layering.sh` accepts any workspace dependency; review's |
| No `unsafe` | `#![forbid(unsafe_code)]`, `scripts/check-unsafe.sh` |
| Every dependency in `Cargo.toml` is named in `## Upstream` below | `scripts/check-readmes.sh` — ⚠️ **only since `M0.21`**; this crate was outside its glob while `M0.13` added the allocator |
| The global allocator is set here and in no library crate | ⚠️ **No gate.** ADR-0007; one `grep` from being checkable, review's until it is |

## Notes for whoever touches this

- **With no arguments it prints a version and exits;** `oqueue serve
  <host:port> [advertise]` binds a listener and serves the wire protocol
  (`M2.25`) over the real write and read paths (`M3.14`): a produce seals one
  bundle, PUTs it once and commits its spans; a fetch resolves offset→object
  through the index. ⚠️ **`OQUEUE_STORE` picks the backend** — `s3`, `gcs`, or
  unset for an in-memory store — and ⚠️ **the metadata and group logs live in
  that same store** (`ADR-0046`, `M6.6`), so positions and committed offsets
  survive a restart exactly as far as the store does: over S3 or GCS, yes;
  over the in-memory store, no. `advertise`
  overrides the identity `Metadata` hands out (doc 02 §7.2: identity is a
  decision); the harness's capture proxy relies on it.
- ⚠️ **`cargo build` links this on x86_64 only.** aarch64 stays at `cargo check`
  until a cross-linker exists, which is `M13`'s work — and `M0`'s completion
  condition says so rather than claiming a link it never performed.
- ⚠️ **The default is `NoOpKeyProvider`, which refuses.** ADR-0006: a deployment
  with no KMS configured should fail the first encryption call, not silently
  write a plaintext data encryption key beside the data it protects.
