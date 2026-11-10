ARG BUILD_IMAGE=node:22-bookworm
FROM ${BUILD_IMAGE} AS build

RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get -o Acquire::Retries=5 update \
    && apt-get -o Acquire::Retries=5 install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        desktop-file-utils \
        file \
        libappindicator3-dev \
        libasound2-dev \
        libclang-dev \
        libgstreamer-plugins-base1.0-0 \
        libgstreamer1.0-0 \
        librsvg2-dev \
        libssl-dev \
        libudev-dev \
        libwebkit2gtk-4.1-dev \
        libxdo-dev \
        patchelf \
        pkg-config \
        wget \
        xdg-utils \
        zsync \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain 1.95.0

ENV PATH=/root/.cargo/bin:$PATH \
    CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    APPIMAGE_EXTRACT_AND_RUN=1

WORKDIR /src
COPY client/ ./client/
COPY desktop/ ./desktop/

RUN --mount=type=cache,target=/root/.npm \
    cd desktop && npm ci
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,target=/src/desktop/src-tauri/target \
    cd desktop \
    && npm run tauri -- build --bundles deb --ci --verbose \
    && mkdir -p /out \
    && cp src-tauri/target/release/bundle/deb/*.deb /out/

FROM scratch AS export
COPY --from=build /out/*.deb /
