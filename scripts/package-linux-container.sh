#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
OUTPUT_ROOT="$REPO_ROOT/dist"
FORCE=0
RUN_TESTS=1

while (($# > 0)); do
    case "$1" in
        --output) (($# >= 2)) || { echo "error: --output requires a directory" >&2; exit 1; }; OUTPUT_ROOT="$2"; shift 2 ;;
        --skip-tests) RUN_TESTS=0; shift ;;
        --force) FORCE=1; shift ;;
        -h|--help)
            echo "Usage: ./scripts/package-linux-container.sh [--output DIR] [--skip-tests] [--force]"
            exit 0
            ;;
        *) echo "error: unknown option: $1" >&2; exit 1 ;;
    esac
done

command -v docker >/dev/null 2>&1 || { echo "error: docker is required" >&2; exit 1; }
if [[ "$OUTPUT_ROOT" != /* ]]; then OUTPUT_ROOT="$PWD/$OUTPUT_ROOT"; fi
mkdir -p -- "$OUTPUT_ROOT"
OUTPUT_ROOT="$(cd -- "$OUTPUT_ROOT" && pwd)"

image="edenvoice-linux-release:debian-bookworm"
docker build --platform linux/amd64 -f "$SCRIPT_DIR/linux-release.Dockerfile" -t "$image" "$SCRIPT_DIR"

package_args=(--platform linux --output /output)
((FORCE == 1)) && package_args+=(--force)
((RUN_TESTS == 0)) && package_args+=(--skip-tests)

model_args=()
if [[ -n "${RNNOISE_MODEL_PATH:-}" ]]; then
    model_args=(--volume "$RNNOISE_MODEL_PATH:/rnnoise-model.tar.gz:ro" --env RNNOISE_MODEL_PATH=/rnnoise-model.tar.gz)
fi

docker run --rm --platform linux/amd64 ${model_args[@]+"${model_args[@]}"} \
    --volume "$REPO_ROOT:/source:ro" \
    --volume "$OUTPUT_ROOT:/output" \
    --volume edenvoice-linux-cargo-registry:/root/.cargo/registry \
    --volume edenvoice-linux-cargo-git:/root/.cargo/git \
    --volume edenvoice-linux-npm-cache:/root/.npm \
    --volume edenvoice-linux-tauri-cache:/root/.cache/tauri \
    --volume edenvoice-linux-client-target:/work/client/target \
    --volume edenvoice-linux-desktop-target:/work/desktop/src-tauri/target \
    "$image" bash -lc '
        tar -C /source \
            --exclude=.git --exclude=.codegraph --exclude=.build --exclude=dist \
            --exclude=desktop/node_modules --exclude=target \
            -cf - . | tar -C /work -xf -

        plugin_url="${EDENVOICE_APPIMAGE_PLUGIN_URL:-https://github.com/linuxdeploy/linuxdeploy-plugin-appimage/releases/download/continuous/linuxdeploy-plugin-appimage-x86_64.AppImage}"
        plugin_sha256="${EDENVOICE_APPIMAGE_PLUGIN_SHA256:-0441769ab38009504d2678c38cd7e526955388dd30a215b4a20afaa5471652f2}"
        plugin_launcher=/root/.cache/tauri/linuxdeploy-plugin-appimage.AppImage
        plugin_source=/root/.cache/tauri/linuxdeploy-plugin-appimage.upstream.AppImage
        if [[ ! -f "$plugin_source" ]]; then
            cached_plugin_sha256=""
            [[ ! -f "$plugin_launcher" ]] || read -r cached_plugin_sha256 _ < <(sha256sum "$plugin_launcher")
            if [[ "$cached_plugin_sha256" == "$plugin_sha256" ]]; then
                mv -- "$plugin_launcher" "$plugin_source"
            else
                curl --fail --location --retry 5 --retry-all-errors \
                    "$plugin_url" --output "$plugin_source.download"
                mv -- "$plugin_source.download" "$plugin_source"
            fi
        fi
        read -r actual_plugin_sha256 _ < <(sha256sum "$plugin_source")
        [[ "$actual_plugin_sha256" == "$plugin_sha256" ]] || {
            echo "error: unexpected linuxdeploy AppImage plugin SHA-256: $actual_plugin_sha256" >&2
            exit 1
        }
        python3 ./scripts/prepare-linux-appimage-plugin.py \
            --source "$plugin_source" \
            --destination /root/.cache/tauri/linuxdeploy-plugin-appimage-extracted \
            --launcher "$plugin_launcher" \
            --wrapper ./scripts/linuxdeploy-plugin-appimage-wrapper.sh
        ./scripts/package-release.sh "$@"
    ' bash "${package_args[@]}"
