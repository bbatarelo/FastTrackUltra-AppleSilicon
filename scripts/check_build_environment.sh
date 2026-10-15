#!/bin/sh
set -eu
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
    echo 'Build on an Apple Silicon Mac in a native (not Rosetta) terminal.' >&2
    exit 1
fi
for tool in clang make python3 cargo rustc; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing $tool. See docs/INSTALLATION.md for prerequisites." >&2
        exit 1
    fi
done
xcrun --find clang >/dev/null
case "$(rustc -vV)" in
    *'host: aarch64-apple-darwin'*) ;;
    *) echo 'Use the native Apple Silicon Rust toolchain.' >&2; exit 1 ;;
esac
echo 'Build prerequisites found: native Apple Silicon, Xcode tools, Python 3 and Rust.'
