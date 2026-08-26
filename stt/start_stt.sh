#!/usr/bin/env bash
# Start the STT service in the background. Writes PID to .stt.pid, logs to stt.log.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"
STT_DIR="$ROOT/stt"
PID_FILE="$STT_DIR/.stt.pid"
LOG_FILE="$STT_DIR/stt.log"

# Already running?
if [[ -f "$PID_FILE" ]] && kill -0 "$(cat "$PID_FILE")" 2>/dev/null; then
    echo "STT service already running (pid=$(cat "$PID_FILE"))"
    echo "  health: http://127.0.0.1:${STT_PORT:-18787}/health"
    exit 0
fi

# Pick venv python
if [[ ! -x "$ROOT/.venv/bin/python" ]]; then
    echo "error: venv not found at $ROOT/.venv — run scripts/setup_venv.sh first" >&2
    exit 1
fi

# Defaults
export STT_HOST="${STT_HOST:-127.0.0.1}"
export STT_PORT="${STT_PORT:-18787}"
export STT_MODEL="${STT_MODEL:-mlx-community/belle-whisper-large-v3-turbo-zh-fp16}"
export STT_LANG="${STT_LANG:-zh}"

cd "$STT_DIR"
nohup "$ROOT/.venv/bin/python" -m uvicorn stt_server:app \
    --host "$STT_HOST" --port "$STT_PORT" --log-level info \
    > "$LOG_FILE" 2>&1 &
echo $! > "$PID_FILE"

# Wait for /health to come up
for i in {1..40}; do
    sleep 0.5
    if curl -sf "http://$STT_HOST:$STT_PORT/health" >/dev/null; then
        echo "STT service ready (pid=$(cat "$PID_FILE"))"
        echo "  endpoint: http://$STT_HOST:$STT_PORT"
        echo "  log:      $LOG_FILE"
        exit 0
    fi
done

echo "STT service failed to come up within 20s. Last log lines:" >&2
tail -30 "$LOG_FILE" >&2
exit 1
