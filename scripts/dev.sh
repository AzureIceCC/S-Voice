#!/usr/bin/env bash
# Dev runner: starts the STT bridge in the background, then execs the Tauri app
# in the foreground. Ctrl-C kills the app; the STT bridge stays up and can be
# stopped with ./stt/stop_stt.sh.
#
# If you want both processes tied to this terminal, run in two separate
# terminals: ./stt/start_stt.sh + ./src-tauri/target/debug/s-voice.
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT="$(pwd)"

# 1. STT bridge (background, written PID to .stt.pid)
./stt/start_stt.sh

# 2. Tauri app (foreground — blocks until app exits)
cd src-tauri
if [[ ! -x ./target/debug/s-voice ]]; then
    echo "building Tauri app first (this may take a few minutes)..."
    cargo build
fi

echo
echo "launching Tauri app (Ctrl-C to quit, STT bridge keeps running)..."
echo
exec ./target/debug/s-voice
