FROM node:22-bookworm

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential ca-certificates clang cmake curl file git libasound2-dev \
    libayatana-appindicator3-dev libclang-dev libfuse2 libgtk-3-dev librsvg2-dev \
    libssl-dev libudev-dev libwebkit2gtk-4.1-dev libxdo-dev meson ninja-build \
    patchelf pkg-config wget xdg-utils zsync \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 --fail --silent --show-error https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain 1.95.0 --profile minimal --component llvm-tools

# linuxdeploy validates desktop entries. squashfs-tools lets the local builder
# unpack its static-PIE AppImage plugin when running under Docker emulation.
RUN apt-get update && apt-get install -y --no-install-recommends \
    desktop-file-utils squashfs-tools \
    && rm -rf /var/lib/apt/lists/*

ENV PATH="/root/.cargo/bin:${PATH}"
WORKDIR /work
