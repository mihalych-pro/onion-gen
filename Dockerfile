# syntax=docker/dockerfile:1
#
#   docker build -t onion-gen:latest .                       # this platform
#   docker buildx build --platform linux/amd64,linux/arm64 .  # both

# ----------------------------------------------------------------- kernel ---
# The CUDA kernel, in a stage of its own so that it is rebuilt only when its
# own sources change. It is architecture-independent — PTX, not machine code —
# so one build serves every target platform.
FROM --platform=$BUILDPLATFORM rust:1-slim-trixie AS kernel

WORKDIR /src
COPY cuda-kernel ./cuda-kernel

# The nightly toolchain named in the crate's own rust-toolchain.toml; run from
# that directory, rustup reads the file, so the version lives in one place.
RUN cd cuda-kernel && rustup toolchain install

# Copied out inside the same layer: the target directory is a build cache and
# does not survive it.
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

# Pinned: zig 0.13 cannot handle the `--fix-cortex-a53-843419` flag that Rust
# emits when linking for aarch64.
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

# From the stage above rather than compiled here, which is what keeps a change
# to the host code off the kernel's build. The build script takes this path
# instead of reaching for nightly, so this stage stays on stable.
COPY --from=kernel /kernel.ptx /kernel/cuda-kernel.ptx
ENV ONION_GEN_KERNEL_PTX=/kernel/cuda-kernel.ptx

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src

# The `.2.28` suffix sets the oldest glibc the binary will run on.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    set -eux; \
    triple="$(cat /tmp/triple)"; \
    cargo zigbuild --release --target "${triple}.2.28"; \
    cp "target/${triple}/release/onion-gen" /onion-gen; \
    mkdir -p /keys-empty

# ---------------------------------------------------------------- release ---
# distroless `cc`: a libc and nothing else. The binary links glibc and opens its
# device libraries (CUDA, OpenCL, Vulkan) at run time, finding none here.
FROM gcr.io/distroless/cc-debian13:nonroot AS release

COPY --from=builder --chown=root:root --chmod=0755 /onion-gen /usr/local/bin/onion-gen

# Docker seeds a fresh named volume from the image, ownership included; without
# this the volume arrives owned by root and the process cannot write to it.
COPY --from=builder --chown=65532:65532 /keys-empty /keys

USER 65532:65532
WORKDIR /home/nonroot

VOLUME ["/keys"]

# 8080: a master listens for workers. 9100: a worker's metrics and probes.
EXPOSE 8080 9100

ENTRYPOINT ["/usr/local/bin/onion-gen"]
