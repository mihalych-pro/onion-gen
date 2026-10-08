# syntax=docker/dockerfile:1
#
#   docker build -t onion-gen:latest .                       # this platform
#   docker buildx build --platform linux/amd64,linux/arm64 .  # both

# Named once, used by two stages below.
ARG BASE=gcr.io/distroless/cc-debian13:nonroot

# ----------------------------------------------------------------- kernel ---
# A stage of its own, so it rebuilds only when its own sources change. PTX is
# architecture-independent: one build serves every platform.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS kernel

WORKDIR /src
COPY cuda-kernel ./cuda-kernel

# Nightly, from the crate's own rust-toolchain.toml — rustup reads it here.
RUN cd cuda-kernel && rustup toolchain install

# Copied out in the same layer: the target directory is a cache and does not
# survive it.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/cuda-kernel/target \
    set -eux; \
    cd cuda-kernel; \
    cargo build --release --target nvptx64-nvidia-cuda; \
    cp target/nvptx64-nvidia-cuda/release/ogkernel.ptx /kernel.ptx; \
    test -s /kernel.ptx

# ---------------------------------------------------------------- compile ---
# Builds on the builder's own architecture and cross-compiles with zig.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS builder

ARG TARGETPLATFORM
ARG BUILDPLATFORM

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Pinned: zig 0.13 chokes on `--fix-cortex-a53-843419`, which Rust emits for
# aarch64.
ARG ZIG_VERSION=0.17.0
RUN set -eux; \
    case "$(uname -m)" in \
      x86_64)  zig_arch=x86_64 ;; \
      aarch64) zig_arch=aarch64 ;; \
      *) echo "unsupported builder architecture $(uname -m)" >&2; exit 1 ;; \
    esac; \
    mkdir -p /opt/zig; \
    curl -fsSL --retry 3 --retry-delay 2 \
      "https://ziglang.org/download/${ZIG_VERSION}/zig-${zig_arch}-linux-${ZIG_VERSION}.tar.xz" \
      | tar -xJ -C /opt/zig --strip-components=1; \
    ln -s /opt/zig/zig /usr/local/bin/zig; \
    zig version

RUN cargo install cargo-zigbuild --locked --version 0.23.4

WORKDIR /src

RUN set -eux; \
    case "${TARGETPLATFORM}" in \
      linux/amd64) echo x86_64-unknown-linux-gnu  > /tmp/triple ;; \
      linux/arm64) echo aarch64-unknown-linux-gnu > /tmp/triple ;; \
      *) echo "unsupported target ${TARGETPLATFORM}" >&2; exit 1 ;; \
    esac; \
    rustup target add "$(cat /tmp/triple)"

# From the stage above, so host changes do not rebuild the kernel and this
# stage stays on stable.
COPY --from=kernel /kernel.ptx /kernel/cuda-kernel.ptx
ENV ONION_GEN_KERNEL_PTX=/kernel/cuda-kernel.ptx

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

# `.2.28`: the oldest glibc this will run on.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eux; \
    triple="$(cat /tmp/triple)"; \
    cargo zigbuild --release --target "${triple}.2.28"; \
    cp "target/${triple}/release/onion-gen" /onion-gen

# Runs nothing. `COPY --from` takes a stage name but not a variable, and
# `/home/nonroot` here is the empty directory the keys one starts as.
FROM ${BASE} AS staging

# ---------------------------------------------------------------- release ---
# distroless `cc`: a libc and nothing else. Device libraries (CUDA, OpenCL,
# Vulkan) are opened at run time and are absent here.
FROM ${BASE} AS release

COPY --from=builder --chown=root:root --chmod=0755 /onion-gen /usr/local/bin/onion-gen

# A mount point Docker has to create itself arrives owned by root, and the run
# then dies with `Permission denied`.
COPY --from=staging --chown=65532:65532 /home/nonroot /home/nonroot/keys

USER 65532:65532
WORKDIR /home/nonroot

VOLUME ["/home/nonroot/keys"]

# 8080: master, for workers. 9100: a worker's metrics and probes.
EXPOSE 8080 9100

ENTRYPOINT ["/usr/local/bin/onion-gen"]
