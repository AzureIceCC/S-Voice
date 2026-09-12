#!/bin/sh
set -eu

# Keep compiler caches out of user-global directories so this probe is safe in
# CI/sandboxes and does not require changing the selected Xcode toolchain.
probe_dir="${TMPDIR:-/tmp}/svoice-apple-speech-probe"
mkdir -p "$probe_dir/clang" "$probe_dir/swift"

CLANG_MODULE_CACHE_PATH="$probe_dir/clang" \
SWIFT_MODULECACHE_PATH="$probe_dir/swift" \
    xcrun swift "$(dirname "$0")/apple_speech_capabilities.swift"
