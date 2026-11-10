ARG BUILD_IMAGE=node:22-bookworm
FROM ${BUILD_IMAGE} AS build

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        clang \
        cmake \
        curl \
        file \
        libclang-dev \
        mingw-w64 \
        nsis \
        pkg-config \
        wget \
    && rm -rf /var/lib/apt/lists/*

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain 1.95.0 \
    && /root/.cargo/bin/rustup target add x86_64-pc-windows-gnu

ENV PATH=/root/.cargo/bin:$PATH \
    CMAKE_POLICY_VERSION_MINIMUM=3.5 \
    CMAKE_TOOLCHAIN_FILE=/opt/edenvoice/windows-mingw-toolchain.cmake \
    CMAKE_TOOLCHAIN_FILE_x86_64_pc_windows_gnu=/opt/edenvoice/windows-mingw-toolchain.cmake \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar \
    CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc \
    CXX_x86_64_pc_windows_gnu=x86_64-w64-mingw32-g++ \
    RANLIB_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ranlib

WORKDIR /src
COPY packaging/windows-mingw-toolchain.cmake /opt/edenvoice/windows-mingw-toolchain.cmake

RUN curl --proto '=https' --tlsv1.2 -fsSL \
        https://downloads.xiph.org/releases/opus/opus-1.3.1.tar.gz \
        -o /tmp/opus-1.3.1.tar.gz \
    && tar -xzf /tmp/opus-1.3.1.tar.gz -C /tmp \
    && cd /tmp/opus-1.3.1 \
    && ./configure \
        --host=x86_64-w64-mingw32 \
        --prefix=/opt/opus \
        --disable-shared \
        --enable-static \
        --disable-doc \
        --disable-extra-programs \
        --disable-stack-protector \
    && make -j2 \
    && make install \
    && rm -rf /tmp/opus-1.3.1 /tmp/opus-1.3.1.tar.gz

ENV LIBOPUS_LIB_DIR=/opt/opus \
    LIBOPUS_NO_PKG=1 \
    LIBOPUS_STATIC=1

COPY client/ ./client/
COPY desktop/ ./desktop/

RUN --mount=type=cache,target=/root/.npm \
    cd desktop && npm ci
RUN --mount=type=cache,target=/root/.cargo/registry \
    --mount=type=cache,target=/root/.cargo/git \
    --mount=type=cache,id=edenvoice-windows-target-v3,target=/src/desktop/src-tauri/target \
    cd desktop \
    && npm run tauri -- build --target x86_64-pc-windows-gnu --bundles nsis --ci --verbose \
    && mkdir -p /out \
    && cp src-tauri/target/x86_64-pc-windows-gnu/release/bundle/nsis/*setup.exe /out/

FROM scratch AS export
COPY --from=build /out/*setup.exe /
