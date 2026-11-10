#!/usr/bin/env bash

set -Eeuo pipefail

PROCESS_NAME="EdenVoice"
TARGET_PID=""
DURATION_SECONDS=7200
INTERVAL_SECONDS=900
OUTPUT_PATH=""
NOTE=""

usage() {
    cat <<'EOF'
Usage: ./scripts/capture-client-soak.sh [options]

Record EdenVoice process CPU and resident memory as CSV during a real-session soak test.

Options:
  --pid PID           Monitor one exact process ID
  --process NAME      Resolve one exact process name (default: EdenVoice)
  --duration SECONDS  Total observation time (default: 7200)
  --interval SECONDS  Sampling interval (default: 900)
  --output FILE       CSV output path (default: ./edenvoice-soak-<timestamp>.csv)
  --note TEXT         Static, non-sensitive environment note added to each row
  -h, --help          Show this help

Use separate macOS identities for the two-client authorization test. Never put server
passwords, provision tokens, private keys, or identity-file contents in --note.
EOF
}

die() {
    printf 'error: %s\n' "$*" >&2
    exit 1
}

require_unsigned_integer() {
    [[ "$2" =~ ^[0-9]+$ ]] || die "$1 must be a non-negative integer"
}

while (($# > 0)); do
    case "$1" in
        --pid)
            (($# >= 2)) || die "--pid requires a value"
            TARGET_PID="$2"
            shift 2
            ;;
        --process)
            (($# >= 2)) || die "--process requires a value"
            PROCESS_NAME="$2"
            shift 2
            ;;
        --duration)
            (($# >= 2)) || die "--duration requires a value"
            DURATION_SECONDS="$2"
            shift 2
            ;;
        --interval)
            (($# >= 2)) || die "--interval requires a value"
            INTERVAL_SECONDS="$2"
            shift 2
            ;;
        --output)
            (($# >= 2)) || die "--output requires a value"
            OUTPUT_PATH="$2"
            shift 2
            ;;
        --note)
            (($# >= 2)) || die "--note requires a value"
            NOTE="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown option: $1"
            ;;
    esac
done

require_unsigned_integer "--duration" "$DURATION_SECONDS"
require_unsigned_integer "--interval" "$INTERVAL_SECONDS"
[[ "$INTERVAL_SECONDS" -gt 0 ]] || die "--interval must be greater than zero"

if [[ -n "$TARGET_PID" ]]; then
    require_unsigned_integer "--pid" "$TARGET_PID"
    [[ "$TARGET_PID" -gt 0 ]] || die "--pid must be greater than zero"
else
    command -v pgrep >/dev/null 2>&1 || die "pgrep is required when --pid is omitted"
    mapfile_supported=0
    if [[ -n "${BASH_VERSINFO[0]:-}" && "${BASH_VERSINFO[0]}" -ge 4 ]]; then
        mapfile_supported=1
    fi
    if ((mapfile_supported == 1)); then
        mapfile -t matching_pids < <(pgrep -x "$PROCESS_NAME" || true)
    else
        matching_pids=()
        while IFS= read -r candidate; do
            [[ -n "$candidate" ]] && matching_pids+=("$candidate")
        done < <(pgrep -x "$PROCESS_NAME" || true)
    fi
    ((${#matching_pids[@]} == 1)) \
        || die "expected exactly one '$PROCESS_NAME' process; found ${#matching_pids[@]}. Pass --pid explicitly"
    TARGET_PID="${matching_pids[0]}"
fi

kill -0 "$TARGET_PID" 2>/dev/null || die "process $TARGET_PID is not running or is not accessible"

if [[ -z "$OUTPUT_PATH" ]]; then
    OUTPUT_PATH="$PWD/edenvoice-soak-$(date '+%Y%m%d-%H%M%S').csv"
fi
OUTPUT_DIRECTORY="$(dirname -- "$OUTPUT_PATH")"
mkdir -p -- "$OUTPUT_DIRECTORY"
OUTPUT_DIRECTORY="$(cd -- "$OUTPUT_DIRECTORY" && pwd)"
OUTPUT_PATH="$OUTPUT_DIRECTORY/$(basename -- "$OUTPUT_PATH")"

escaped_note=${NOTE//\"/\"\"}
printf 'timestamp,pid,process_elapsed,cpu_percent,rss_kib,status,note\n' >"$OUTPUT_PATH"

started_at=$(date +%s)
deadline=$((started_at + DURATION_SECONDS))
while true; do
    timestamp=$(date -u '+%Y-%m-%dT%H:%M:%SZ')
    if kill -0 "$TARGET_PID" 2>/dev/null; then
        sample=$(LC_ALL=C ps -p "$TARGET_PID" -o etime=,%cpu=,rss= | awk '{$1=$1; print}')
        if [[ -n "$sample" ]]; then
            read -r process_elapsed cpu_percent rss_kib <<<"$sample"
            status="running"
        else
            process_elapsed=""
            cpu_percent=""
            rss_kib=""
            status="unavailable"
        fi
    else
        process_elapsed=""
        cpu_percent=""
        rss_kib=""
        status="exited"
    fi
    printf '%s,%s,%s,%s,%s,%s,"%s"\n' \
        "$timestamp" "$TARGET_PID" "$process_elapsed" "$cpu_percent" "$rss_kib" "$status" "$escaped_note" \
        >>"$OUTPUT_PATH"

    now=$(date +%s)
    if [[ "$status" == "exited" || "$now" -ge "$deadline" ]]; then
        break
    fi
    remaining=$((deadline - now))
    sleep_for=$INTERVAL_SECONDS
    if ((remaining < sleep_for)); then
        sleep_for=$remaining
    fi
    ((sleep_for > 0)) || break
    sleep "$sleep_for"
done

printf 'EdenVoice soak samples written to %s\n' "$OUTPUT_PATH"
