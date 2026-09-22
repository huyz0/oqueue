# M13.4: the default artifact's build environment. FIPS-only build tools are
# intentionally absent; the FIPS job gets its own image and workflow leg.
FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive
ENV CARGO_INCREMENTAL=0
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential ca-certificates curl git \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN toolchain="$(awk -F '"' '$1 ~ /^channel = / { print $2 }' /tmp/rust-toolchain.toml)" \
    && test -n "$toolchain" \
    && curl -fsSL https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "$toolchain"

WORKDIR /work
COPY . .
RUN scripts/release-clean-build.sh
