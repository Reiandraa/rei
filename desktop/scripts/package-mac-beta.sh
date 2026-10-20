#!/bin/bash
# Build on the owner's Apple Silicon Mac; no API, accounts, cloud or model download.
set -eu
task_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$task_root"
if [ "$(uname -s)" != Darwin ] || [ "$(uname -m)" != arm64 ]; then
  echo "This beta packaging script needs an Apple Silicon Mac."
  exit 1
fi
for tool in node npm cargo; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Missing build tool: $tool. Set it up before continuing."
    exit 1
  fi
done
npm ci
npm run tauri -- build --target aarch64-apple-darwin --bundles app --config tauri.macos-beta.conf.json
task_app="$task_root/src-tauri/target/aarch64-apple-darwin/release/bundle/macos/REI OS.app"
codesign --verify --deep --strict "$task_app"
lipo "$task_app/Contents/MacOS/rei-os" -verify_arch arm64
mkdir -p beta-artifacts
ditto -c -k --sequesterRsrc --keepParent "$task_app" beta-artifacts/REI-OS-macOS-Apple-Silicon-beta2.zip
cd beta-artifacts
shasum -a 256 REI-OS-macOS-Apple-Silicon-beta2.zip > SHA256-macOS-Apple-Silicon.txt
echo "Built and verified Apple Silicon application: $task_app"
echo "ZIP/checksum: $task_root/beta-artifacts"
echo "Try the new app before publishing it. Ad-hoc signed, not Apple-notarized."
