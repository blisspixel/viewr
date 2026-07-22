#!/usr/bin/env bash
# Build and ad-hoc sign a local App Sandbox bundle for profile verification.
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "macOS App Sandbox packaging requires macOS" >&2
  exit 1
fi
if [[ $# -gt 1 ]]; then
  echo "usage: $0 [binary-directory]" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd -P)"
BINARY_DIR="${1:-$ROOT/target/debug}"

if [[ "$BINARY_DIR" != /* ]]; then
  BINARY_DIR="$ROOT/$BINARY_DIR"
fi
PROFILE_ROOT="$ROOT/target/profile-check/macos"
for parent in "$ROOT/target" "$ROOT/target/profile-check" "$PROFILE_ROOT"; do
  if [[ -L "$parent" ]]; then
    echo "Refusing a symlinked App Sandbox staging path: $parent" >&2
    exit 1
  fi
done
mkdir -p "$PROFILE_ROOT"
if [[ "$(cd "$PROFILE_ROOT" && pwd -P)" != "$PROFILE_ROOT" ]]; then
  echo "Refusing App Sandbox staging outside target/profile-check/macos" >&2
  exit 1
fi
BUNDLE="$PROFILE_ROOT/viewr.app"
if [[ -L "$BUNDLE" ]]; then
  echo "Refusing to replace a symlinked App Sandbox bundle" >&2
  exit 1
fi

for binary in viewr viewr-decode; do
  if [[ ! -f "$BINARY_DIR/$binary" ]]; then
    echo "Missing packaged binary: $BINARY_DIR/$binary" >&2
    exit 1
  fi
done

rm -rf -- "$BUNDLE"
mkdir -p "$BUNDLE/Contents/MacOS"
cp "$ROOT/packaging/macos/Info.plist" "$BUNDLE/Contents/Info.plist"
cp "$BINARY_DIR/viewr" "$BUNDLE/Contents/MacOS/viewr"
cp "$BINARY_DIR/viewr-decode" "$BUNDLE/Contents/MacOS/viewr-decode"

codesign --force --sign - \
  --entitlements "$ROOT/packaging/macos/viewr-decode.entitlements" \
  "$BUNDLE/Contents/MacOS/viewr-decode"
codesign --force --sign - \
  --entitlements "$ROOT/packaging/macos/viewr.entitlements" \
  "$BUNDLE"
codesign --verify --deep --strict "$BUNDLE"
plutil -lint "$BUNDLE/Contents/Info.plist"
"$BUNDLE/Contents/MacOS/viewr" doctor >/dev/null

echo "App Sandbox bundle validated: $BUNDLE"
