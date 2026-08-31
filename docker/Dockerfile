# The container every heavy command in this repository runs inside.
#
# ⚠️ **It exists for containment, not convenience.** On WSL2 a build or test
# that allocates without bound exhausts the VM, which takes the running session
# with it and needs a manual `wsl --shutdown` to recover — the VM has no
# per-process ceiling to hit first. `scripts/docker-test.sh` gives this image
# hard memory, CPU, pid and `/tmp` limits, so the kernel kills something
# *inside* the container and the host never notices.
#
# ⚠️ **It is also the more capable environment, not a reduced one**, which is
# the part worth checking before assuming a skip here is normal:
#
#   - A cross toolchain is installed, so `m0-complete.sh` checks the **whole**
#     workspace on both targets. That gate excludes `oqueue` and `oqueue-store`
#     from the aarch64 leg only where a cross `cc` is absent — `ring` and
#     `mimalloc` both run `cc` from a build script — so a host without one is
#     held to less. Measured: this machine has no `aarch64-linux-gnu-gcc` and
#     the container's run prints `cargo check --workspace passes for
#     aarch64-unknown-linux-gnu`.
#   - A JDK and `confluent-kafka` are here, so `kafka-client-harness.sh` runs
#     both real clients rather than skipping them and withholding
#     `m2-complete.sh`'s verdict.
#   - A nightly toolchain and `cargo-fuzz` are here, because `fuzz.sh` is the
#     tier most able to exhaust a machine and the one that most needs a bound.
#     ⚠️ **Nightly is the one thing here that is not pinned** and cannot be:
#     rustup resolves `nightly` to that day's build, so an image rebuilt later
#     fuzzes under a different compiler. That is tolerable because `fuzz.sh` is
#     a nightly *tier* rather than a commit gate — a finding is a crash with a
#     reproducing artifact, not a threshold — but it is the one reproducibility
#     hole in this file and is written down rather than left to be discovered.
#
# ⚠️ **One leg genuinely cannot run inside**: `m1-complete.sh` starts MinIO
# through a Docker daemon, and this container has no socket. It prints
# `skip conformance suite against MinIO` and `this gate proved nothing about
# the S3 backend`, which is the honest outcome and the reason that gate is
# written to skip rather than pass. ⚠️ **But it still exits 0** — `lib.sh`'s
# `finish` exits on the failure count and a `skip` does not raise it — so a
# wrapper reading only the status records M1 green having proved nothing. Read
# the output, or run that gate on the host.

# ⚠️ **Pinned by digest, not by `:21-jdk`**, and `m1-complete.sh` already made
# this argument for MinIO: "`docker run` does not re-pull an image that is
# already local, so `:latest` means two machines can run different builds while
# both record verified". It bites harder here — the rolling tag rebases onto
# new Ubuntu releases, changing gcc, glibc and Python underneath a gate suite
# whose `portability.md` rule 5 pins glibc at 2.28 and whose `m0-complete.sh`
# exercises the link. The tag is kept in the comment because a digest says
# nothing about what it is.
FROM eclipse-temurin:21-jdk@sha256:85f00967bcc624fc19fa9c2cf124ea426a5363898e267141726f31f358c2e14b

ENV DEBIAN_FRONTEND=noninteractive

# `build-essential` for the C compilers `ring` and `mimalloc` invoke from build
# scripts (NFR-42 asks for "cargo and a C compiler"); `gcc-aarch64-linux-gnu`
# for the cross leg above. ⚠️ **`libc6-dev-arm64-cross` is not optional beside
# it**: the compiler alone gets as far as `#include <stdint.h>` and dies on
# `bits/libc-header-start.h`, so a container with one and not the other is
# *worse* than a host with neither — `m0-complete.sh` excludes the two `cc`
# crates only where a cross compiler is **absent**, so half a toolchain turns a
# conditional skip into a hard failure. Measured, by building it that way
# first. ⚠️ **No OpenSSL packages**, and the reason is
# worth stating because their absence looks like an omission: `ADR-0012` chose
# `ring` with `rustls`, `Cargo.lock` holds neither `openssl-sys` nor
# `native-tls`, and `ring` builds its own assembly through the `cc` crate — so
# nothing here links libssl or asks pkg-config for it.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git \
        gcc-aarch64-linux-gnu libc6-dev-arm64-cross \
        python3 python3-pip python3-venv \
        procps \
    && rm -rf /var/lib/apt/lists/*

# ⚠️ **Pinned to `rust-toolchain.toml`, and a bump is a two-file diff reviewed
# together.** `rustup` would otherwise read that file at first invocation and
# download a second toolchain on the first `cargo` call, which is a network
# dependency inside a container built to be reproducible.
ARG RUST_VERSION=1.97.1
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH
RUN curl -fsSL https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "$RUST_VERSION" \
        -c rustfmt -c clippy -c llvm-tools-preview \
    && rustup target add x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu \
    && chmod -R a+w "$RUSTUP_HOME" "$CARGO_HOME"

# The two gates that build with a changed fingerprint.
#
# ⚠️ **`--version`, not `--locked` alone**, and the difference is the whole
# point: `--locked` pins the *tool's own dependency lockfile* and never the
# tool, so `cargo install cargo-llvm-cov --locked` installs whatever is newest
# on crates.io that day. This comment claimed the opposite until review
# measured it — 0.9.0 in the image against 0.8.7 on the host, two different
# programs deciding one `COVERAGE_FLOOR`. Two builds of a digest-pinned
# Dockerfile weeks apart would otherwise compute the same image tag and hold
# different `check-mutants` behaviour, which is the divergence the base-image
# pin above cites `m1-complete.sh` to rule out.
ARG LLVM_COV_VERSION=0.9.0
ARG MUTANTS_VERSION=27.1.0
RUN cargo install cargo-llvm-cov --version "$LLVM_COV_VERSION" --locked \
    && cargo install cargo-mutants --version "$MUTANTS_VERSION" --locked

# ⚠️ **Nightly and `cargo-fuzz` are for `fuzz.sh` only.** libFuzzer needs
# `-Z sanitizer`, which is nightly-only, and `fuzz.sh` invokes
# `cargo +nightly fuzz`. Nothing else in the workspace may use nightly —
# `rust-toolchain.toml` is what every other command resolves to.
ARG FUZZ_VERSION=0.13.2
RUN rustup toolchain install nightly --profile minimal \
    && cargo install cargo-fuzz --version "$FUZZ_VERSION" --locked

# `kafka-client-harness.sh` discovers librdkafka by importing `confluent-kafka`,
# because the wheel bundles the C library — so importability *is* the library
# being present. `--break-system-packages` because Debian marks the system
# interpreter externally managed and a venv would not be on the harness's path.
RUN pip3 install --no-cache-dir --break-system-packages \
        confluent-kafka pre-commit

# The repo is bind-mounted from the host, so git may see its files as someone
# else's and refuse to operate — and every script here finds the repo root
# through `git rev-parse`. The mount is the thing we came here to work on, so
# trusting it is correct.
#
# ⚠️ **Kept for a root shell only.** `M10.24` runs the container as the
# invoking uid with `HOME=/tmp/home`, so this file — `/root/.gitconfig` — is
# not read on the commit path, and `docker-test.sh` passes the same exception
# through `GIT_CONFIG_PARAMETERS` instead. It still covers `docker run` against
# this image without those flags, which is how the image is used by hand.
RUN git config --global --add safe.directory /work

WORKDIR /work
