#!/usr/bin/env bash
# Install the pinned Flatpak SDK used by the Linux sandbox profile probe.
#
# Flathub Platform and Sdk 25.08 are the sandbox. The rust-stable extension is
# only the compiler that builds viewr inside that SDK. Flathub republishes
# rust-stable on every Rust release and can 404 the ostree ref for hours, which
# is not a defect in this repository but fails the Ubuntu quality job exactly
# like a missing runtime. Retry the Flathub extension first. A Flathub compiler
# older than rust-toolchain.toml cannot build this workspace, so that case uses
# the same fallback as a missing ref: the official standalone tarball, installed
# at /usr/lib/sdk/rust-stable. The product manifest is unchanged. A hung pull is
# a failed attempt: each Flathub try is bounded so the fallback can start, and
# the workflow step timeout remains the outer bound.
set -euo pipefail

attempts=3
attempt_seconds=240
term_grace_seconds=20

RUST_VERSION=1.99.1
RUST_DIST_DATE=2026-10-15
RUST_TARBALL="rust-${RUST_VERSION}-x86_64-unknown-linux-gnu.tar.xz"
RUST_URL="https://static.rust-lang.org/dist/${RUST_DIST_DATE}/${RUST_TARBALL}"
RUST_SHA256=231fcae47ee950621374725b5fc3b5441d2fbda2955ac45dd7dc816d8876cfe7

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

  echo "Installing pinned ${RUST_VERSION} as the SDK extension" >&2

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

# rustc is mounted at the same path flatpak-builder uses. The probe runs the
# SDK, not a host binary. An empty result means the compiler could not be read.
flathub_rustc_version() {
  local line
  line="$(
    flatpak run --user --command=/usr/lib/sdk/rust-stable/bin/rustc \
      org.freedesktop.Sdk//25.08 --version 2>/dev/null || true
  )"
  printf '%s\n' "$line" | awk 'NR==1 && $1 == "rustc" { print $2 }'
}

# True when installed is present and not strictly older than required.
# sort -C reads every line, so pipefail does not see a SIGPIPE.
version_meets_pin() {
  local installed=$1
  local required=$2
  [ -n "$installed" ] || return 1
  printf '%s\n%s\n' "$required" "$installed" | sort -C -V
}

retry_fn "Freedesktop 25.08 runtime" install_runtimes
if retry_fn "Flathub rust-stable" install_flathub_rust_stable; then
  installed="$(flathub_rustc_version)"
  if version_meets_pin "$installed" "$RUST_VERSION"; then
    echo "Flathub rust-stable ${installed} meets pinned ${RUST_VERSION}" >&2
    exit 0
  fi
  echo "Flathub rust-stable ${installed:-unknown} is older than pinned ${RUST_VERSION}; replacing it" >&2
  flatpak uninstall --user --assumeyes --noninteractive \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
fi
install_pinned_rust_stable_extension
