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
            echo "Usage: ./scripts/package-all-release.sh [--output DIR] [--skip-tests] [--force]"
            exit 0
            ;;
        *) echo "error: unknown option: $1" >&2; exit 1 ;;
    esac
done

common=(--output "$OUTPUT_ROOT")
((FORCE == 1)) && common+=(--force)
skip_tests=()
((RUN_TESTS == 0)) && skip_tests+=(--skip-tests)

"$SCRIPT_DIR/package-release.sh" --platform macos "${common[@]}" "${skip_tests[@]}"
"$SCRIPT_DIR/package-release.sh" --platform windows --skip-install --skip-tests "${common[@]}"
"$SCRIPT_DIR/package-linux-container.sh" --skip-tests "${common[@]}"
"$SCRIPT_DIR/package-server-release.sh" "${common[@]}" "${skip_tests[@]}"
