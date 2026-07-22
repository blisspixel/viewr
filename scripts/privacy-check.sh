#!/usr/bin/env bash
# Privacy verification for viewr (Unix).
# Exit 0 only when the privacy invariants we can check locally all hold.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

echo "== cargo deny (remote-client ban + confined Linux D-Bus + licenses) =="
cargo deny check

echo "== packaging artifacts must omit network grants =="
# Only flag real finish-args, not comments that say "do NOT add --share=network".
if grep -v '^\s*#' packaging/flatpak/com.github.blisspixel.viewr.yml | grep -q -- '--share=network'; then
  echo "Flatpak manifest must not grant --share=network" >&2
  exit 1
fi
# Real grants are <key>...network.client|server</key> outside XML comments.
for entitlements in \
  packaging/macos/viewr.entitlements \
  packaging/macos/viewr-decode.entitlements; do
  if sed '/<!--/,/-->/d' "$entitlements" | grep -Eq '<key>com\.apple\.security\.network\.(client|server)</key>'; then
    echo "$entitlements must not grant network client/server" >&2
    exit 1
  fi
done

appx=packaging/windows/AppxManifest.xml
test -f "$appx"
grep -q 'uap10:TrustLevel="appContainer"' "$appx"
grep -q 'uap10:RuntimeBehavior="packagedClassicApp"' "$appx"
grep -Eq '<Capabilities[[:space:]]*/>' "$appx"
if grep -Eq 'Name="(internetClient|internetClientServer|privateNetworkClientServer|broadFileSystemAccess|runFullTrust)"' "$appx"; then
  echo "Windows AppContainer must not grant network, broad filesystem, or full-trust capabilities" >&2
  exit 1
fi

echo "== dependency tree must not pull remote-service client stacks =="
for crate in reqwest hyper rustls native-tls; do
  if cargo tree -p viewr -i "$crate" >/dev/null 2>&1; then
    echo "Forbidden network-related crate in tree: $crate" >&2
    exit 1
  fi
done

echo "== source must not write activity side-files or always-on logging =="
if grep -q 'OpenOptions' crates/viewr/src/app.rs; then
  echo "app.rs must not use OpenOptions (activity side-files are forbidden)" >&2
  exit 1
fi
if grep -q 'default_filter_or' crates/viewr/src/main.rs; then
  echo "main.rs must not enable env_logger by default (opt-in only via RUST_LOG/VIEWR_LOG)" >&2
  exit 1
fi
test -f crates/viewr/src/ephemeral.rs
grep -q 'scrub_stale_viewr_temps' crates/viewr/src/ephemeral.rs
grep -q 'load_from_memory' crates/viewr/src/cli.rs
grep -q 'scrub_stale_viewr_temps' crates/viewr/src/main.rs

echo "privacy-check: OK"
