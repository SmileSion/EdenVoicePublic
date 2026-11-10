#!/usr/bin/env bash

set -Eeuo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
SERVER_DIR="$REPO_ROOT/server"
OUTPUT_ROOT="$REPO_ROOT/dist"
ARCHITECTURES=(amd64 arm64)
RUN_TESTS=1
FORCE=0
CHECK_ONLY=0

die() { printf 'error: %s\n' "$*" >&2; exit 1; }
require_command() { command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"; }

usage() {
    cat <<'EOF'
Usage: ./scripts/package-server-release.sh [options]

Build static Linux server archives for AMD64 and ARM64.

Options:
  --arch NAME    amd64, arm64, or all (default: all)
  --output DIR   output root (default: ./dist)
  --skip-tests   skip go test and go vet
  --check        validate tools, version, and release tag, then exit
  --force        replace existing architecture output directories
EOF
}

while (($# > 0)); do
    case "$1" in
        --arch)
            (($# >= 2)) || die "--arch requires a value"
            case "$2" in
                all) ARCHITECTURES=(amd64 arm64) ;;
                amd64|arm64) ARCHITECTURES=("$2") ;;
                *) die "invalid architecture: $2" ;;
            esac
            shift 2
            ;;
        --output) (($# >= 2)) || die "--output requires a directory"; OUTPUT_ROOT="$2"; shift 2 ;;
        --skip-tests) RUN_TESTS=0; shift ;;
        --check) CHECK_ONLY=1; shift ;;
        --force) FORCE=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown option: $1" ;;
    esac
done

require_command go
require_command tar
VERSION="$(awk -F'"' '/"version":/ { print $4; exit }' "$REPO_ROOT/desktop/package.json")"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$ ]] || die "invalid version: $VERSION"
if [[ "${GITHUB_REF_TYPE:-}" == "tag" ]]; then
    [[ "${GITHUB_REF_NAME:-}" == "v$VERSION" ]] || \
        die "Git tag ${GITHUB_REF_NAME:-<missing>} does not match application version v$VERSION"
fi
if ((CHECK_ONLY == 1)); then
    printf 'Server release configuration is valid for EdenVoice v%s (%s).\n' "$VERSION" "${ARCHITECTURES[*]}"
    exit 0
fi

if [[ "$OUTPUT_ROOT" != /* ]]; then OUTPUT_ROOT="$PWD/$OUTPUT_ROOT"; fi
mkdir -p -- "$OUTPUT_ROOT"
OUTPUT_ROOT="$(cd -- "$OUTPUT_ROOT" && pwd)"

if ((RUN_TESTS == 1)); then
    (cd -- "$SERVER_DIR" && go test ./... && go vet ./...)
fi

STAGING_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/edenvoice-server-release.XXXXXX")"
trap 'rm -rf -- "$STAGING_ROOT"' EXIT

for architecture in "${ARCHITECTURES[@]}"; do
    package_name="EdenVoice-server-v${VERSION}-linux-${architecture}"
    staging_dir="$STAGING_ROOT/$package_name"
    output_dir="$OUTPUT_ROOT/v$VERSION/server/linux-$architecture"
    if [[ -e "$output_dir" ]]; then
        ((FORCE == 1)) || die "$output_dir already exists; pass --force to replace it"
        [[ "$output_dir" == "$OUTPUT_ROOT"/v*/server/linux-* ]] || die "refusing to replace unexpected path: $output_dir"
        rm -rf -- "$output_dir"
    fi
    mkdir -p -- "$staging_dir" "$output_dir"
    (
        cd -- "$SERVER_DIR"
        CGO_ENABLED=0 GOOS=linux GOARCH="$architecture" \
            go build -trimpath -ldflags="-s -w" -o "$staging_dir/voice-server" ./cmd/voice-server
    )
    cp -- "$SERVER_DIR/config.example.toml" "$staging_dir/"
    cp -- "$SERVER_DIR/deploy/edenvoice.service" "$staging_dir/"
    cp -- "$SERVER_DIR/deploy/config.7000-7001.toml" "$staging_dir/"
    cp -- "$REPO_ROOT/LICENSE" "$REPO_ROOT/THIRD_PARTY_NOTICES.md" "$REPO_ROOT/README.md" "$REPO_ROOT/README.zh-CN.md" "$staging_dir/"
    cp -- "$REPO_ROOT/THIRD_PARTY_LICENSES.md" "$staging_dir/"
    python3 "$SCRIPT_DIR/package-source.py" --scope server --output "$staging_dir/corresponding-source.tar.gz"
    COPYFILE_DISABLE=1 tar --no-xattrs -C "$STAGING_ROOT" -czf "$output_dir/$package_name.tar.gz" "$package_name"
    (
        cd -- "$output_dir"
        if command -v shasum >/dev/null 2>&1; then
            shasum -a 256 "$package_name.tar.gz" >SHA256SUMS.txt
        else
            sha256sum "$package_name.tar.gz" >SHA256SUMS.txt
        fi
    )
done

printf 'Ready-to-use server packages written to %s/v%s/server\n' "$OUTPUT_ROOT" "$VERSION"
