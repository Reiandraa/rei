#!/bin/bash
# Build on the owner's Apple Silicon Mac; no API, accounts, cloud or model download.
set -eu
task_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$task_root"
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "This beta packaging script needs an Apple Silicon Mac."
  exit 1
fi
for tool in node npm cargo codesign lipo ditto; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing build tool: $tool. Set it up before continuing."
    exit 1
  fi
done
node scripts/package-mac-beta.mjs "$@"
