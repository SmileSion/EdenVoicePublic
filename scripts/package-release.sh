#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
DESKTOP_DIR="$REPO_ROOT/desktop"
OUTPUT_ROOT="$REPO_ROOT/dist"
PLATFORM="auto"
RUN_TESTS=1
INSTALL_DEPENDENCIES=1
FORCE=0
CHECK_ONLY=0
WINDOWS_TARGET="x86_64-pc-windows-msvc"
WINDOWS_CROSS=0
WINDOWS_CARGO_HOME=""
WINDOWS_CARGO_TARGET_DIR=""
WINDOWS_OPUS_LIB_DIR=""
WINDOWS_MESON_CROSS_FILE=""
WINDOWS_LIBCLANG_PATH=""

# audiopus_sys vendors an Opus CMake project whose compatibility declaration
# predates current CMake releases. This is a build-time policy only; end users
# do not need CMake or any other development dependency.
export CMAKE_POLICY_VERSION_MINIMUM="${CMAKE_POLICY_VERSION_MINIMUM:-3.5}"

usage() {
    cat <<'EOF'
Usage: ./scripts/package-release.sh [options]

Build the current Tauri desktop client as a native, end-user-ready package:
  macOS   -> DMG
  Windows -> compact NSIS installer that provisions WebView2 automatically
  Linux   -> AppImage with the native EdenVoice audio engine included

Options:
  --platform NAME   auto, macos, windows, or linux (default: auto)
  --output DIR      output root (default: ./dist)
  --skip-tests      skip frontend and Rust tests
  --skip-install    reuse the existing node_modules directory
  --check           validate platform, versions, and release tag, then exit
  --force           replace this version/platform output directory
  -h, --help        show this help

Windows x64 packages may be built locally on macOS with cargo-xwin, LLVM,
Ninja, CMake and NSIS. Native builds are used for the other host/platform pairs.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_command() {
    command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

python_command() {
    if command -v python3 >/dev/null 2>&1; then
        command -v python3
    elif command -v python >/dev/null 2>&1; then
        command -v python
    else
        die "required command not found: python3 or python"
    fi
}

while (($# > 0)); do
    case "$1" in
        --platform)
            (($# >= 2)) || die "--platform requires a value"
            PLATFORM="$2"
            shift 2
            ;;
        --output)
            (($# >= 2)) || die "--output requires a directory"
            OUTPUT_ROOT="$2"
            shift 2
            ;;
        --skip-tests) RUN_TESTS=0; shift ;;
        --skip-install) INSTALL_DEPENDENCIES=0; shift ;;
        --check) CHECK_ONLY=1; shift ;;
        --force) FORCE=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

case "$(uname -s)" in
    Darwin) HOST_PLATFORM="macos" ;;
    Linux) HOST_PLATFORM="linux" ;;
    MINGW*|MSYS*|CYGWIN*) HOST_PLATFORM="windows" ;;
    *) die "unsupported host: $(uname -s)" ;;
esac

if [[ "$HOST_PLATFORM" == "macos" && -z "${LIBCLANG_PATH:-}" && \
      -f /Library/Developer/CommandLineTools/usr/lib/libclang.dylib ]]; then
    export LIBCLANG_PATH=/Library/Developer/CommandLineTools/usr/lib
fi

[[ "$PLATFORM" == "auto" ]] && PLATFORM="$HOST_PLATFORM"
[[ "$PLATFORM" =~ ^(macos|windows|linux)$ ]] || die "invalid platform: $PLATFORM"
if [[ "$HOST_PLATFORM" == "macos" && "$PLATFORM" == "windows" ]]; then
    WINDOWS_CROSS=1
elif [[ "$PLATFORM" != "$HOST_PLATFORM" ]]; then
    die "$PLATFORM packages cannot be built on a $HOST_PLATFORM host"
fi

require_command node
require_command npm
require_command cargo

prepend_path_if_directory() {
    [[ -d "$1" ]] && PATH="$1:$PATH"
}

sha1_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum "$1" | awk '{print $1}'
    else
        sha1sum "$1" | awk '{print $1}'
    fi
}

prepare_macos_windows_cross_build() {
    prepend_path_if_directory /opt/homebrew/opt/llvm/bin
    prepend_path_if_directory /opt/homebrew/opt/lld/bin
    prepend_path_if_directory /opt/homebrew/bin
    export PATH

    require_command cargo-xwin
    require_command rustup
    require_command cmake
    require_command ninja
    require_command makensis
    require_command clang-cl
    require_command llvm-lib
    require_command llvm-strip
    require_command llvm-rc

    rustup target add "$WINDOWS_TARGET"
    cargo xwin cache xwin

    local xwin_cache_root toolchain_file opus_source opus_root opus_build python_bin llvm_bin
    xwin_cache_root="${XWIN_CACHE_DIR:-$(node -p "require('node:path').join(require('node:os').homedir(), 'Library/Caches/cargo-xwin')")}"
    toolchain_file="$xwin_cache_root/cmake/clang-cl/$WINDOWS_TARGET-toolchain.cmake"
    [[ -f "$toolchain_file" ]] || die "cargo-xwin CMake toolchain not found: $toolchain_file"

    llvm_bin="$(dirname -- "$(command -v clang-cl)")"
    WINDOWS_LIBCLANG_PATH="$(cd -- "$llvm_bin/../lib" && pwd)"
    [[ -f "$WINDOWS_LIBCLANG_PATH/libclang.dylib" ]] || \
        die "Homebrew libclang was not found: $WINDOWS_LIBCLANG_PATH/libclang.dylib"

    WINDOWS_CARGO_HOME="${EDENVOICE_WINDOWS_CARGO_HOME:-$REPO_ROOT/.build/windows-x64/cargo-home}"
    WINDOWS_CARGO_TARGET_DIR="${EDENVOICE_WINDOWS_CARGO_TARGET_DIR:-$REPO_ROOT/.build/windows-x64/target}"
    mkdir -p -- "$WINDOWS_CARGO_HOME" "$WINDOWS_CARGO_TARGET_DIR"
    CARGO_HOME="$WINDOWS_CARGO_HOME" cargo fetch \
        --manifest-path "$DESKTOP_DIR/src-tauri/Cargo.toml" \
        --target "$WINDOWS_TARGET" --locked
    python_bin="$(python_command)"
    CARGO_HOME="$WINDOWS_CARGO_HOME" \
        "$python_bin" "$REPO_ROOT/scripts/patch-webrtc-apm-clang-cl.py"
    opus_source="$(find "$WINDOWS_CARGO_HOME/registry/src" -type f -path '*/audiopus_sys-0.2.2/opus/CMakeLists.txt' -print -quit)"
    [[ -n "$opus_source" ]] || die "vendored audiopus_sys 0.2.2 Opus source was not found after cargo fetch"
    opus_source="${opus_source%/CMakeLists.txt}"

    opus_root="$REPO_ROOT/.build/windows-x64/opus"
    opus_build="$opus_root/build"
    if [[ ! -f "$opus_root/install/lib/opus.lib" ]]; then
        cmake -S "$opus_source" -B "$opus_build" -G Ninja \
            -DCMAKE_TOOLCHAIN_FILE="$toolchain_file" \
            -DCMAKE_INSTALL_PREFIX="$opus_root/install" \
            -DCMAKE_BUILD_TYPE=Release \
            -DCMAKE_POLICY_VERSION_MINIMUM=3.5 \
            -DOPUS_BUILD_SHARED_LIBRARY=OFF \
            -DOPUS_BUILD_PROGRAMS=OFF \
            -DOPUS_BUILD_TESTING=OFF \
            -DOPUS_DISABLE_INTRINSICS=ON
        cmake --build "$opus_build" --target install
    fi
    WINDOWS_OPUS_LIB_DIR="$opus_root/install"

    WINDOWS_MESON_CROSS_FILE="$REPO_ROOT/.build/windows-x64/meson/webrtc-meson-cross.ini"
    "$python_bin" "$REPO_ROOT/scripts/generate-windows-meson-cross.py" \
        --xwin-root "$xwin_cache_root/xwin" \
        --output-dir "${WINDOWS_MESON_CROSS_FILE%/*}" \
        --clang-cl "$(command -v clang-cl)" \
        --llvm-lib "$(command -v llvm-lib)" \
        --llvm-strip "$(command -v llvm-strip)" \
        --llvm-rc "$(command -v llvm-rc)" >/dev/null
    [[ -f "$WINDOWS_MESON_CROSS_FILE" ]] || die "Meson cross file was not generated"

    # Tauri normally downloads this pinned helper on its first NSIS build. An
    # explicit file makes repeat/offline builds deterministic and is hash checked.
    local tauri_cache helper_target helper_sha1 expected_helper_sha1
    expected_helper_sha1="75197fee3c6a814fe035788d1c34ead39349b860"
    tauri_cache="$(node -p "require('node:path').join(require('node:os').homedir(), 'Library/Caches/tauri')")"
    helper_target="$tauri_cache/NSIS/Plugins/x86-unicode/additional/nsis_tauri_utils.dll"
    if [[ -n "${TAURI_NSIS_UTILS_DLL:-}" ]]; then
        [[ -f "$TAURI_NSIS_UTILS_DLL" ]] || die "TAURI_NSIS_UTILS_DLL does not exist: $TAURI_NSIS_UTILS_DLL"
        helper_sha1="$(sha1_of "$TAURI_NSIS_UTILS_DLL")"
        [[ "$helper_sha1" == "$expected_helper_sha1" ]] || \
            die "TAURI_NSIS_UTILS_DLL has unexpected SHA-1: $helper_sha1"
        mkdir -p -- "${helper_target%/*}"
        [[ "$TAURI_NSIS_UTILS_DLL" == "$helper_target" ]] || cp -- "$TAURI_NSIS_UTILS_DLL" "$helper_target"
    fi
    if [[ -f "$helper_target" ]]; then
        helper_sha1="$(sha1_of "$helper_target")"
        [[ "$helper_sha1" == "$expected_helper_sha1" ]] || \
            die "cached nsis_tauri_utils.dll has unexpected SHA-1: $helper_sha1"
    else
        printf 'Tauri NSIS helper is not cached; Tauri will download it on this first build.\n'
    fi
}

VERSION="$(cd -- "$DESKTOP_DIR" && node -p "require('./package.json').version")"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || die "invalid version: $VERSION"

cargo_package_version() {
    awk '
        /^\[package\]$/ { in_package = 1; next }
        /^\[/ { in_package = 0 }
        in_package && /^version[[:space:]]*=/ {
            gsub(/^[^\"]*\"|\".*$/, "", $0)
            print
            exit
        }
    ' "$1"
}

CLIENT_VERSION="$(cargo_package_version "$REPO_ROOT/client/Cargo.toml")"
TAURI_CRATE_VERSION="$(cargo_package_version "$DESKTOP_DIR/src-tauri/Cargo.toml")"
TAURI_CONFIG_VERSION="$(cd -- "$DESKTOP_DIR/src-tauri" && node -p "require('./tauri.conf.json').version")"
for version_source in \
    "client/Cargo.toml:$CLIENT_VERSION" \
    "desktop/src-tauri/Cargo.toml:$TAURI_CRATE_VERSION" \
    "desktop/src-tauri/tauri.conf.json:$TAURI_CONFIG_VERSION"
do
    version_path="${version_source%%:*}"
    version_value="${version_source#*:}"
    [[ "$version_value" == "$VERSION" ]] || \
        die "$version_path version $version_value does not match desktop/package.json $VERSION"
done
if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
    [[ "${GITHUB_REF_NAME:-}" == "v$VERSION" ]] || \
        die "Git tag ${GITHUB_REF_NAME:-<missing>} does not match application version v$VERSION"
fi
if ((CHECK_ONLY == 1)); then
    printf 'Release configuration is valid for EdenVoice v%s on %s.\n' "$VERSION" "$PLATFORM"
    exit 0
fi

require_command meson
require_command ninja
require_command nm

# The locked WebRTC APM crate always passes Meson's --reconfigure flag, including
# for a clean build directory. Apply the target-aware build patch on every native
# platform after fetching the exact dependency. Cross builds use an isolated
# Cargo home and apply the same patch during cross-toolchain preparation.
if ((WINDOWS_CROSS == 0)); then
    if [[ "$PLATFORM" == "windows" ]]; then
        require_command clang-cl
        export CC="${CC:-clang-cl}"
        export CXX="${CXX:-clang-cl}"
    fi
    cargo fetch --manifest-path "$DESKTOP_DIR/src-tauri/Cargo.toml" --locked
    if [[ "${EDENVOICE_SKIP_APM_PATCH:-0}" != 1 ]]; then
        "$(python_command)" "$REPO_ROOT/scripts/patch-webrtc-apm-clang-cl.py"
    fi
fi

if [[ "$OUTPUT_ROOT" != /* ]]; then OUTPUT_ROOT="$PWD/$OUTPUT_ROOT"; fi
mkdir -p -- "$OUTPUT_ROOT"
OUTPUT_ROOT="$(cd -- "$OUTPUT_ROOT" && pwd)"
RELEASE_DIR="$OUTPUT_ROOT/v$VERSION/$PLATFORM"
if [[ -e "$RELEASE_DIR" ]]; then
    ((FORCE == 1)) || die "$RELEASE_DIR already exists; pass --force to replace it"
    [[ "$RELEASE_DIR" == "$OUTPUT_ROOT"/v*/"$PLATFORM" ]] || die "refusing to replace unexpected path: $RELEASE_DIR"
    rm -rf -- "$RELEASE_DIR"
fi
mkdir -p -- "$RELEASE_DIR"

if ((INSTALL_DEPENDENCIES == 1)); then
    (cd -- "$DESKTOP_DIR" && npm ci)
fi

if ((RUN_TESTS == 1)); then
    (cd -- "$DESKTOP_DIR" && npm run build)
    cargo test --manifest-path "$REPO_ROOT/client/Cargo.toml" --lib --locked
    cargo test --manifest-path "$DESKTOP_DIR/src-tauri/Cargo.toml" --locked
fi

if ((WINDOWS_CROSS == 1)); then
    prepare_macos_windows_cross_build
fi

if ((WINDOWS_CROSS == 1)); then
    CARGO_TARGET_ROOT="$WINDOWS_CARGO_TARGET_DIR"
else
    CARGO_TARGET_ROOT="${CARGO_TARGET_DIR:-$DESKTOP_DIR/src-tauri/target}"
fi

case "$PLATFORM" in
    macos)
        BUNDLE="app,dmg"
        BUNDLE_DIR="$CARGO_TARGET_ROOT/release/bundle/dmg"
        ;;
    windows)
        BUNDLE="nsis"
        if ((WINDOWS_CROSS == 1)); then
            BUNDLE_DIR="$CARGO_TARGET_ROOT/$WINDOWS_TARGET/release/bundle/nsis"
        else
            BUNDLE_DIR="$CARGO_TARGET_ROOT/release/bundle/nsis"
        fi
        ;;
    linux)
        BUNDLE="appimage"
        BUNDLE_DIR="$CARGO_TARGET_ROOT/release/bundle/appimage"
        ;;
esac

# Keep the signing key local or provide it through the environment. Never print its contents.
if [[ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
    [[ -f "$REPO_ROOT/.release-keys/updater.key" ]] || die "Set TAURI_SIGNING_PRIVATE_KEY to build signed updates"
    export TAURI_SIGNING_PRIVATE_KEY="$REPO_ROOT/.release-keys/updater.key"
fi
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}"

printf 'Building EdenVoice %s for %s (%s)\n' "$VERSION" "$PLATFORM" "$BUNDLE"
if ((WINDOWS_CROSS == 1)); then
    (cd -- "$DESKTOP_DIR" && \
        CARGO_HOME="$WINDOWS_CARGO_HOME" \
        CARGO_TARGET_DIR="$WINDOWS_CARGO_TARGET_DIR" \
        LIBCLANG_PATH="$WINDOWS_LIBCLANG_PATH" \
        OPUS_LIB_DIR="$WINDOWS_OPUS_LIB_DIR" \
        OPUS_STATIC=1 \
        CMAKE_GENERATOR=Ninja \
        MESON_CROSS_FILE="$WINDOWS_MESON_CROSS_FILE" \
        npm run tauri -- build \
        --runner cargo-xwin --target "$WINDOWS_TARGET" --bundles "$BUNDLE" --ci --verbose)
else
    (cd -- "$DESKTOP_DIR" && npm run tauri -- build --bundles "$BUNDLE" --ci --verbose)
fi

case "$PLATFORM" in
    macos) find "$BUNDLE_DIR" -maxdepth 1 -type f -name "*_${VERSION}_*.dmg" -exec cp -- {} "$RELEASE_DIR/" \; ;;
    windows) find "$BUNDLE_DIR" -maxdepth 1 -type f -name "*_${VERSION}_*-setup.exe" -exec cp -- {} "$RELEASE_DIR/" \; ;;
    linux) find "$BUNDLE_DIR" -maxdepth 1 -type f -name "*_${VERSION}_*.AppImage" -exec cp -- {} "$RELEASE_DIR/" \; ;;
esac

# macOS updates use app.tar.gz; Windows/Linux reuse their native installer artifact.
UPDATE_DIR="$BUNDLE_DIR"
UPDATE_PATTERN="*_${VERSION}_*.AppImage"
UPDATE_OS="$PLATFORM"
case "$PLATFORM" in
    macos) UPDATE_DIR="$(dirname "$BUNDLE_DIR")/macos"; UPDATE_PATTERN='*.app.tar.gz'; UPDATE_OS=darwin ;;
    windows) UPDATE_PATTERN="*_${VERSION}_*-setup.exe" ;;
esac
UPDATE_ARCH="$(uname -m)"
[[ "$UPDATE_ARCH" != arm64 ]] || UPDATE_ARCH=aarch64
if ((WINDOWS_CROSS == 1)); then UPDATE_ARCH="${WINDOWS_TARGET%%-*}"; fi
UPDATE_ARTIFACT=""
while IFS= read -r artifact; do
    [[ -z "$UPDATE_ARTIFACT" ]] || die "Multiple updater artifacts found; clean bundle output first"
    UPDATE_ARTIFACT="$artifact"
done < <(find "$UPDATE_DIR" -maxdepth 1 -type f -name "$UPDATE_PATTERN")
[[ -n "$UPDATE_ARTIFACT" && -s "$UPDATE_ARTIFACT.sig" ]] || die "Signed updater artifact missing"
cp -- "$UPDATE_ARTIFACT" "$UPDATE_ARTIFACT.sig" "$RELEASE_DIR/"
node "$SCRIPT_DIR/update-manifest.mjs" create "$VERSION" "$UPDATE_OS-$UPDATE_ARCH" "$RELEASE_DIR/$(basename "$UPDATE_ARTIFACT")" "$RELEASE_DIR/admin-update-$UPDATE_OS-$UPDATE_ARCH.json"

cp -- "$REPO_ROOT/LICENSE" "$REPO_ROOT/THIRD_PARTY_NOTICES.md" "$REPO_ROOT/THIRD_PARTY_LICENSES.md" "$RELEASE_DIR/"
python3 "$SCRIPT_DIR/package-source.py" --scope client --output "$RELEASE_DIR/EdenVoice-v$VERSION-corresponding-source.tar.gz"

find "$RELEASE_DIR" -maxdepth 1 -type f | grep -q . || die "no package was produced in $BUNDLE_DIR"
(
    cd -- "$RELEASE_DIR"
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 ./* >SHA256SUMS.txt
    else
        sha256sum ./* >SHA256SUMS.txt
    fi
    ls -lh
)

printf 'Ready-to-use package written to %s\n' "$RELEASE_DIR"
