#!/usr/bin/env bash
# One-shot setup: create the uv venv and install Python deps.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ ! -d .venv ]]; then
    uv venv --python 3.12 .venv
fi

# shellcheck disable=SC1091
source .venv/bin/activate
uv pip install -r requirements.txt

echo "venv ready at .venv"
