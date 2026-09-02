---
title: "Build and Release Portability: Host Toolchain, glibc Floor, Cross-Compilation, and the Artifact Matrix"
slug: build-and-release-portability
status: draft
last_updated: 2026-08-13
tags: [portability, cross-compilation, glibc, musl, cargo-zigbuild, cross, packaging, release-engineering, target-cpu, macos, aarch64, graviton, reproducible-builds]
related: [19-workspace-engineering, 18-rust-performance-methodology, 10-open-questions]
summary: >
  The half of portability doc 19 skipped. Separates "can someone build it"
  (host toolchain, cross-compilation) from "will the artifact run" (glibc
  floor, ISA baseline). Core findings: the glibc floor is the real Linux
  distribution problem and cargo-zigbuild solves it with a target suffix;
  musl's default allocator costs 10-40x under concurrency, which promotes
  doc 18's allocator choice from optimization to mandatory; free arm64 CI
  runners make native builds beat cross-compilation; and macOS should be a
  development platform, not a release target. Resolves open question #29.
---

# Build and Release Portability

*Compiled 2026-08-13. Written because [19](19-workspace-engineering.md) §7 covered test portability and skipped build portability entirely — see §0.*

Marked **[Documented]**, **[Measured]**, **[Practice]** (established practice), or **[Assessment]**.

---

## 0. Two different problems, routinely conflated

**[Assessment]**

| | Question | Fails as |
|---|---|---|
| **Build portability** | Can a person or CI runner *produce* the artifact? | "works on my machine" — a missing host tool, a C toolchain, a cross-linker |
| **Runtime portability** | Will the artifact *run* where it's deployed? | `GLIBC_2.38 not found`, `SIGILL`, silently-wrong SIMD |

They have disjoint solutions, and mixing them produces the classic mistake of statically linking everything (a runtime fix) to solve a build problem, at a large and unnecessary performance cost (§3).

---

## 1. The host toolchain tax

**[Practice]** A mature Rust service's gates routinely require, beyond cargo: `cmake`, `docker`, `go`, `protoc`, `python`, and `valgrind`, plus `cargo-fuzz`, `cargo-mutants`, and `cargo-nextest`. Each is a way for a fresh checkout to fail on someone else's machine.

**[Assessment] The rule that keeps this manageable: `cargo build` must need only cargo and a C compiler. Everything else belongs to gates, and gates must degrade rather than fail.**

The pattern that makes this survivable is `require_tool protoc "apt-get install protobuf-compiler" || finish`, which **skips** the gate with a named remedy rather than failing the build. The distinction that matters: **a missing tool must never be indistinguishable from a failing check.** Without it, an absent `protoc` surfaces as "clippy failed" four lines into a build-script error, and the time is spent debugging the wrong thing.

**Dependency choices are build-portability choices.** The heaviest offenders in this space:

| Dependency | Host requirement |
|---|---|
| `aws-lc-sys` (rustls default) | cmake, and NASM on Windows |
| `ring` | cmake/clang, historically painful to cross-compile |
| `openssl-sys` (non-vendored) | system OpenSSL headers, per-distro |
| anything `bindgen` | libclang |
| `protoc`-based codegen | a `protoc` binary |

**[Assessment]** oqueue has an advantage worth protecting: **the Kafka wire protocol is not protobuf**, so we have no structural need for `protoc` at all. Keep it that way. For TLS, prefer `rustls` with a backend chosen for build simplicity, and treat "adds a C toolchain requirement" as a real cost in the [05](05-rust-ecosystem.md) crate selection, not a footnote.

---

## 2. The glibc floor — the actual Linux distribution problem

**[Documented]** glibc symbol versioning is **forward-compatible only**. A binary linked against glibc 2.43 will not start on a system with 2.34; the reverse is fine. A current rolling-release development host sits well ahead of every deployment target, so a binary built there natively fails on essentially every current LTS server:

| Target | glibc |
|---|---|
| RHEL/Rocky/Alma 9 | 2.34 |
| Ubuntu 22.04 LTS | 2.35 |
| Debian 12 | 2.36 |
| Ubuntu 24.04 LTS | 2.39 |
| **Typical current dev host** | **2.43** |

**[Assessment]** This is *the* Linux binary-distribution problem and it is invisible until someone deploys. Three answers exist:

1. **Build in a container with an old glibc** (the manylinux approach). Reliable, and requires maintaining a build image.
2. **`cargo-zigbuild`** — uses Zig as the linker and takes the floor as a **target suffix**: **[Documented]**
   ```
   cargo zigbuild --target aarch64-unknown-linux-gnu.2.17
   ```
   Without a suffix, Zig picks its own default (2.28 for Zig v12–v14). This is the cleanest available mechanism: no build container, no sysroot wrangling, declared in the command that builds.
3. **Static musl** — sidesteps glibc entirely, at the cost in §3.

**[Assessment] Recommendation: `cargo-zigbuild` with an explicit floor of glibc 2.28.** That covers RHEL 8 and everything newer, it's a declared constant rather than an emergent property of the build host, and it eliminates the entire class of "works in CI, fails on the customer's box." Pin the floor in CI config so it can't drift.

⚠️ **Caveat [Documented]:** `cargo-zigbuild` handles the C *toolchain*; it does not rescue dependencies with elaborate native build scripts. This is another reason to keep §1's dependency list short. There is also a known gap around static glibc builds with an explicit `--target` ([#231](https://github.com/rust-cross/cargo-zigbuild/issues/231)).

---

## 3. musl: a trap for this workload specifically

**[Measured]** musl's default allocator does not scale across cores — the reported penalty for async, concurrent, or performance-sensitive applications is **10× to 40×**, with the bottleneck in malloc's thread synchronization. Replacing it with mimalloc or jemalloc recovers nearly all of it, landing within roughly **13%** of glibc.

**[Assessment] This is the most consequential finding in this document, because it changes an existing recommendation.** [18](18-rust-performance-methodology.md) §3.5 rates the allocator choice at "5–15% on residual small-object traffic" — true on glibc. **On a musl static build it is worth 10–40×, and a broker is the most allocator-contended workload imaginable**: every produce path allocates a buffer on a network thread and frees it on a flush thread.

So: **if we ship a musl artifact, a non-default global allocator is not an optimization, it is a correctness-of-performance requirement**, and shipping musl without one would produce a binary that looks fine in single-threaded benchmarks and collapses under concurrent load — the worst possible failure shape.

Recommendation: **glibc-with-pinned-floor is the primary artifact; musl static is secondary**, for scratch/distroless containers, and it carries a mandatory `#[global_allocator]`. Add a CI assertion that the musl build has one.

---

## 4. Cross-compilation: the answer changed

**[Documented]** GitHub's **arm64 Linux hosted runners are generally available and free for public repositories** (4 vCPU), having gone GA in Aug 2025 and reached private repos in Jan 2026. Standard GitHub-hosted runners are **free and unmetered on public repos**.

**[Assessment] For an open-source project this inverts the usual advice: native compilation beats cross-compilation.** The historical reason to cross-compile — no ARM machine — no longer applies, and native builds avoid the sysroot and native-dependency failure modes entirely.

| Tool | Use it for |
|---|---|
| **Native runners** | Every release artifact. Free, no cross toolchain, no emulation. |
| **`cargo-zigbuild`** | The glibc floor (§2), even when building natively |
| **`cross`** | Cross-*testing* via QEMU for a target with no runner, or local aarch64 checks from an x86 box |

`cross` remains the right tool for running a test suite on an architecture you don't have, since it wires up QEMU inside the container. It is the wrong tool for producing releases when a native runner is free.

---

## 5. `target-cpu` — resolving open question #29

**[Assessment]** #29 asked `x86-64-v2` vs `v3`. It was never answerable in isolation, because **the baseline is a function of the artifact strategy**, which is this document's subject.

The constraints, from [18](18-rust-performance-methodology.md) §3.4:

- **v4 is not a candidate** — it SIGILLs on every Intel consumer chip since Alder Lake (AVX-512 fused off) and on Zen 2/3 cloud instances.
- **v3 (AVX2, 2013)** excludes pre-Haswell, which for a self-hosted OSS broker is a real user population.
- **v2 (SSE4.2, 2008)** is effectively free — and critically **makes the SSE4.2 `crc32` instruction statically available**, removing the single most important dispatch decision.

**Resolution: ship one binary per `(os, arch)` at `x86-64-v2` / aarch64 `+lse,+crc`, and put everything above the baseline behind runtime dispatch.**

The reasoning is that **we need runtime dispatch regardless** — [18](18-rust-performance-methodology.md) §4.2–4.3 requires it for CRC-32C, where the spread between baseline and VPCLMULQDQ is roughly 12 GB/s to 97 GB/s. Once the dispatch machinery exists, raising the *static* baseline buys very little and costs users. A `-v3` variant can be added later as an opt-in artifact if measurement justifies it; it is not a day-one decision.

The aarch64 side is not symmetric: `+lse` is worth **[Measured, AWS]** "over 3x" on atomics-heavy code on larger Graviton systems, and Rust already enables `outline-atomics` by default on aarch64-linux so a generic binary runtime-dispatches to LSE. Setting the feature lets LLVM inline it.

---

## 6. macOS: a development platform, not a release target

**[Assessment]** This is a scoping decision that removes a large amount of work, and it should be made explicitly.

An object-storage-backed broker is deployed on Linux. Nobody runs it in production on macOS. So:

- **macOS must** build the workspace and run the T0/T1 test tiers (per [19](19-workspace-engineering.md) §7), because that's where much local development happens.
- **macOS need not** produce release artifacts — which means **no codesigning, no notarization, no universal binaries, no Gatekeeper handling**. All of that is real, tedious work avoided by writing the decision down.
- Windows is not a target at all; WSL2 is the supported path for development on Windows.

The cost of this decision is that macOS-only build breakage is caught by CI rather than by a release gate — which is exactly what the free `macos-latest` runner in [19](19-workspace-engineering.md) §7.2 is for.

---

## 7. The artifact matrix

**[Assessment]**

| Artifact | Built on | Notes |
|---|---|---|
| `x86_64-unknown-linux-gnu` | ubuntu native + zigbuild | glibc floor 2.28, `target-cpu=x86-64-v2` |
| `aarch64-unknown-linux-gnu` | **ubuntu-24.04-arm native** | glibc floor 2.28, `+lse,+crc` |
| `x86_64-unknown-linux-musl` | ubuntu native | **mandatory `#[global_allocator]`** (§3) |
| `aarch64-unknown-linux-musl` | ubuntu-24.04-arm native | same |
| OCI image, multi-arch | both | manifest list; distroless or scratch from the musl build |
| — | — | **no macOS, no Windows release artifacts** (§6) |

Both Linux architectures are first-class. Graviton is a plausible deployment target for a cost-sensitive broker, and [18](18-rust-performance-methodology.md) §3.4's LSE finding means the ARM build is not an afterthought.

---

## 8. Build determinism

**[Assessment]** Complements [19](19-workspace-engineering.md) §8.4:

- **`rust-toolchain.toml`** — the prerequisite for everything else here. Without it the compiler version is whatever `rustup` last fetched, which makes every other reproducibility measure moot. An MSRV field plus a CI install step is not a substitute.
- **`--locked` in CI** so a stale `Cargo.lock` fails loudly.
- **`--remap-path-prefix`** to strip absolute build paths from binaries and panic messages — also a small privacy win, since otherwise the builder's home-directory path ships inside the binary.
- **`SOURCE_DATE_EPOCH`** for timestamp determinism.
- **Pin the glibc floor and `target-cpu` in CI config**, not in a developer's shell. Both are properties of the artifact and both drift silently.

---

## 9. Open questions

- ~~**Which TLS backend?**~~ — **resolved 2026-08-17, `ADR-0012`: `ring`** for the default build, with `aws-lc-rs` behind a `fips` feature for `M8`'s artifact only — `docs/internal/product/decisions/0012-object-store-tls-provider.md`, struck the same way in `docs/researches/10-open-questions.md` #36 (build) by `M1.45`. This entry was never updated when that one was; both should read the same now. ⚠️ **`M1.42` found a cost the ADR itself missed**: `ring`'s build script needs a *target* `cc`, so any crate holding it drops out of the aarch64 check without one.
- **Does `object_store`'s dependency tree cross-compile cleanly to aarch64-musl?** The four-way matrix in §7 is asserted, not verified. One CI run settles it, and it should be done before the matrix is promised.
- **Is a `-v3` artifact worth publishing later?** Only measurement answers it, and only after runtime dispatch (§5) is in place so the comparison is meaningful.
- **glibc floor: 2.28 or 2.34?** 2.28 covers RHEL 8, which reaches EOL in 2029. If we don't intend to support RHEL 8, 2.34 is a freer choice. A product decision, not a technical one.

---

## Sources

[cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) and its [glibc version targeting](https://github.com/rust-cross/cargo-zigbuild/blob/main/README.md) · [cargo-zigbuild #231 (static glibc + explicit target)](https://github.com/rust-cross/cargo-zigbuild/issues/231) · [cross](https://github.com/cross-rs/cross) · [Rust Project Primer: cross-compiling](https://rustprojectprimer.com/building/cross.html) · [Performance of static Rust with MUSL](https://raniz.blog/2025-02-06_rust-musl-malloc/) · [Tweag: Supercharging Rust static executables with mimalloc](https://www.tweag.io/blog/2023-08-10-rust-static-link-with-mimalloc/) · [arm64 hosted runners GA for public repos](https://github.blog/changelog/2025-08-07-arm64-hosted-runners-for-public-repositories-are-now-generally-available/) · [arm64 standard runners in private repos](https://github.blog/changelog/2026-01-29-arm64-standard-runners-are-now-available-in-private-repositories/) · [AWS Graviton getting-started: Rust](https://github.com/aws/aws-graviton-getting-started/blob/main/rust.md) · [x86-64 microarchitecture levels](https://en.opensuse.org/X86-64_microarchitecture_levels)
