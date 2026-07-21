#!/usr/bin/env bash
# Privacy verification for viewr (Unix).
# Exit 0 only when the privacy invariants we can check locally all hold.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== cargo deny (network crate ban + licenses) =="
cargo deny check

echo "== packaging artifacts must omit network grants =="
if grep -q -- '--share=network' packaging/flatpak/com.github.blisspixel.viewr.yml; then
  echo "Flatpak manifest must not contain --share=network" >&2
  exit 1
fi
if grep -Eq 'network\.(client|server)' packaging/macos/viewr.entitlements; then
  echo "macOS entitlements must not grant network client/server" >&2
  exit 1
fi
test -f packaging/windows/APPCONTAINER.md

echo "== dependency tree must not pull reqwest/hyper/rustls =="
for crate in reqwest hyper rustls native-tls; do
  if cargo tree -p viewr -i "$crate" >/dev/null 2>&1; then
    echo "Forbidden network-related crate in tree: $crate" >&2
    exit 1
  fi
done

echo "privacy-check: OK"
