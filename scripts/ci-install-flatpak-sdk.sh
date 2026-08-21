#!/usr/bin/env bash
# Install the pinned Flatpak SDK used by the Linux sandbox profile probe.
#
# Flathub Platform and Sdk 25.08 are the sandbox. The rust-stable extension is
# only the compiler that builds viewr inside that SDK. Flathub republishes
# rust-stable on every Rust release and can 404 the ostree ref for hours, which
# is not a defect in this repository but fails the Ubuntu quality job exactly
# like a missing runtime. Retry the Flathub extension first. If it stays
# missing, install the same /usr/lib/sdk/rust-stable layout from the official
# standalone tarball pinned to rust-toolchain.toml. The product manifest is
# unchanged. A hung pull is a failed attempt: each Flathub try is bounded so
# the fallback can start, and the workflow step timeout remains the outer bound.
set -euo pipefail

attempts=3
attempt_seconds=240
term_grace_seconds=20

RUST_VERSION=1.96.0
RUST_DIST_DATE=2026-05-28
RUST_TARBALL="rust-${RUST_VERSION}-x86_64-unknown-linux-gnu.tar.xz"
RUST_URL="https://static.rust-lang.org/dist/${RUST_DIST_DATE}/${RUST_TARBALL}"
RUST_SHA256=c295047583a56238ea06b43f849f4b877fa12bfd4c7103f8d9a74c94c9c4e108

ensure_flathub() {
  flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo
}

install_runtimes() {
  ensure_flathub
  flatpak install --user --assumeyes --noninteractive flathub \
    org.freedesktop.Platform//25.08 \
    org.freedesktop.Sdk//25.08
}

install_flathub_rust_stable() {
  ensure_flathub
  flatpak install --user --assumeyes --noninteractive flathub \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
}

export -f ensure_flathub install_runtimes install_flathub_rust_stable

retry_fn() {
  local label=$1
  local fn=$2
  local attempt delay
  for attempt in $(seq 1 "$attempts"); do
    if timeout \
      --signal=TERM \
      --kill-after="${term_grace_seconds}s" \
      "${attempt_seconds}s" \
      bash -c "$fn"; then
      return 0
    fi
    if [ "$attempt" -lt "$attempts" ]; then
      delay=$((attempt * 15))
      echo "${label} attempt ${attempt} of ${attempts} failed; retrying in ${delay}s" >&2
      sleep "$delay"
    fi
  done
  echo "${label} failed after ${attempts} attempts" >&2
  return 1
}

install_pinned_rust_stable_extension() {
  local work
  work="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$work'" EXIT

  echo "Flathub rust-stable 25.08 unavailable; installing pinned ${RUST_VERSION} as the SDK extension" >&2

  curl --fail --location --proto =https --tlsv1.2 \
    --retry 3 --retry-delay 5 \
    --output "${work}/${RUST_TARBALL}" \
    "$RUST_URL"
  echo "${RUST_SHA256}  ${work}/${RUST_TARBALL}" | sha256sum -c -

  cat > "${work}/org.freedesktop.Sdk.Extension.rust-stable.yml" <<EOF
id: org.freedesktop.Sdk.Extension.rust-stable
branch: "25.08"
runtime: org.freedesktop.Sdk
runtime-version: "25.08"
sdk: org.freedesktop.Sdk
build-extension: true
separate-locales: false
build-options:
  prefix: /usr/lib/sdk/rust-stable
modules:
  - name: rust
    buildsystem: simple
    build-commands:
      - ./install.sh --prefix=/usr/lib/sdk/rust-stable --without=rust-docs --without=rust-docs-json-preview --disable-ldconfig --verbose
    sources:
      - type: archive
        path: ${RUST_TARBALL}
        sha256: ${RUST_SHA256}
        strip-components: 1
EOF

  flatpak-builder --user --force-clean --install --disable-download --disable-rofiles-fuse \
    "${work}/build" \
    "${work}/org.freedesktop.Sdk.Extension.rust-stable.yml"
  flatpak info --user org.freedesktop.Sdk.Extension.rust-stable//25.08 >/dev/null
}

retry_fn "Freedesktop 25.08 runtime" install_runtimes
if retry_fn "Flathub rust-stable" install_flathub_rust_stable; then
  exit 0
fi
install_pinned_rust_stable_extension
