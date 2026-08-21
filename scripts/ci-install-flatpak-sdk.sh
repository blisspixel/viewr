#!/usr/bin/env bash
# Install the pinned Flatpak SDK used by the Linux sandbox profile probe,
# retrying the transient 404s and stalls Flathub produces while a runtime
# is republished.
#
# A Flathub rebuild can delete the ostree commit for
# org.freedesktop.Sdk.Extension.rust-stable before the replacement is
# visible. That 404 is not a defect in this repository, but it fails the
# Ubuntu quality job exactly like one. Retrying absorbs that without
# hiding a real missing runtime. A hung pull is treated the same as a
# failed one: each attempt is bounded so the next try can start, and the
# workflow step timeout remains the outer bound.
set -euo pipefail

attempts=3
attempt_seconds=240
term_grace_seconds=20

run_attempt() {
  flatpak remote-add --user --if-not-exists flathub \
    https://flathub.org/repo/flathub.flatpakrepo
  flatpak install --user --assumeyes --noninteractive flathub \
    org.freedesktop.Platform//25.08 \
    org.freedesktop.Sdk//25.08 \
    org.freedesktop.Sdk.Extension.rust-stable//25.08
}

export -f run_attempt

for attempt in $(seq 1 "$attempts"); do
  if timeout \
    --signal=TERM \
    --kill-after="${term_grace_seconds}s" \
    "${attempt_seconds}s" \
    bash -c run_attempt; then
    exit 0
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    delay=$((attempt * 15))
    echo "flatpak SDK install attempt ${attempt} of ${attempts} failed; retrying in ${delay}s" >&2
    sleep "$delay"
  fi
done

echo "flatpak SDK install failed after ${attempts} attempts" >&2
exit 1
