FROM debian:bookworm-slim

ENV DEBIAN_FRONTEND=noninteractive \
    CARGO_HOME=/usr/local/cargo \
    RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:/usr/local/go/bin:${PATH}

RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake golang ca-certificates curl git \
    && rm -rf /var/lib/apt/lists/*

COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN channel="$(awk -F'"' '/^channel[[:space:]]*=/{print $2; exit}' /tmp/rust-toolchain.toml)" \
    && test -n "$channel" \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
      | sh -s -- -y --profile minimal --default-toolchain "$channel" \
    && rustup show active-toolchain

WORKDIR /workspace
COPY . .
RUN scripts/release-fips-build.sh
RUN cargo clippy --locked -p oqueue --no-default-features --features fips --all-targets -- -D warnings
RUN cargo clippy --locked -p oqueue-crypto --no-default-features --features fips --all-targets -- -D warnings
RUN cargo clippy --locked -p oqueue-store --no-default-features --features fips --all-targets -- -D warnings
RUN cargo clippy --locked -p oqueue-broker --no-default-features --features fips --all-targets -- -D warnings
RUN cargo test --locked -p oqueue-crypto --no-default-features --features fips --test it
RUN cargo test --locked -p oqueue-store --no-default-features --features fips --lib
RUN cargo test --locked -p oqueue-broker --no-default-features --features fips --lib
RUN cargo test --locked -p oqueue --no-default-features --features fips
RUN /workspace/target/release/oqueue
