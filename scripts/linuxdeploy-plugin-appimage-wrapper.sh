#!/usr/bin/env bash

set -Eeuo pipefail

cache_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
plugin_root="$cache_dir/linuxdeploy-plugin-appimage-extracted"
export APPDIR="$plugin_root"
export APPIMAGE="$cache_dir/linuxdeploy-plugin-appimage.upstream.AppImage"
exec "$plugin_root/usr/bin/linuxdeploy-plugin-appimage" "$@"
