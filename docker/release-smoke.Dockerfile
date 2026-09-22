# M13.7: build and exercise the default artifact on the glibc 2.28 floor.
FROM rockylinux:8

ENV CARGO_INCREMENTAL=0
ENV RUSTUP_HOME=/usr/local/rustup
ENV CARGO_HOME=/usr/local/cargo
ENV PATH=/usr/local/cargo/bin:$PATH

RUN dnf module enable -y python39 \
    && dnf install -y \
        ca-certificates \
        curl \
        gcc \
        gcc-c++ \
        git \
        make \
        python39 \
        tar \
    && dnf clean all

COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN toolchain="$(awk -F '"' '$1 ~ /^channel = / { print $2 }' /tmp/rust-toolchain.toml)" \
    && test -n "$toolchain" \
    && curl -fsSL https://sh.rustup.rs \
      | sh -s -- -y --no-modify-path --profile minimal \
        --default-toolchain "$toolchain"

WORKDIR /work
COPY . .
RUN cargo build --locked --release -p oqueue --no-default-features --features software-aead,ring
ENV OQUEUE_BIN=/work/target/release/oqueue
ENTRYPOINT ["python3.9", "scripts/harness/role_smoke.py"]
