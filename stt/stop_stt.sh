#!/usr/bin/env bash
# Stop the STT service started by start_stt.sh.
set -euo pipefail

cd "$(dirname "$0")/.."
STT_DIR="$(pwd)/stt"
PID_FILE="$STT_DIR/.stt.pid"

if [[ ! -f "$PID_FILE" ]]; then
    echo "STT service not running (no pid file)"
    exit 0
fi

PID="$(cat "$PID_FILE")"
if kill -0 "$PID" 2>/dev/null; then
    kill "$PID"
    echo "sent SIGTERM to pid $PID"
    for _ in {1..10}; do
        sleep 0.5
        if ! kill -0 "$PID" 2>/dev/null; then break; fi
    done
    if kill -0 "$PID" 2>/dev/null; then
        kill -9 "$PID"
        echo "forced SIGKILL on pid $PID"
    fi
fi
rm -f "$PID_FILE"
echo "STT service stopped"
