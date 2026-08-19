#!/usr/bin/env bash
# Install runner packages, retrying the transient failures hosted package
# mirrors produce.
#
# A stalled or briefly unreachable mirror is not a defect in this repository,
# but it fails a job exactly like one and has cancelled a release. Retrying
# absorbs that without hiding a real failure. A hung apt-get is treated the
# same as a failed one: each attempt is bounded so the next try can start,
# and the workflow step timeout remains the outer bound.
#
# Arguments are passed through to `apt-get install` unchanged, so a caller that
# wants --no-install-recommends passes it and one that does not, does not.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: ci-install-packages.sh <apt-get install arguments>" >&2
  exit 2
fi

attempts=3
attempt_seconds=180
term_grace_seconds=20

apt_get() {
  sudo env DEBIAN_FRONTEND=noninteractive apt-get \
    -o Acquire::Retries=2 \
    -o Acquire::http::Timeout=30 \
    -o Acquire::https::Timeout=30 \
    "$@"
}

run_attempt() {
  apt_get update
  apt_get install -y "$@"
}

export -f apt_get run_attempt

for attempt in $(seq 1 "$attempts"); do
  if timeout \
    --signal=TERM \
    --kill-after="${term_grace_seconds}s" \
    "${attempt_seconds}s" \
    bash -c 'run_attempt "$@"' bash "$@"; then
    exit 0
  fi
  if [ "$attempt" -lt "$attempts" ]; then
    delay=$((attempt * 15))
    echo "package install attempt ${attempt} of ${attempts} failed; retrying in ${delay}s" >&2
    sleep "$delay"
  fi
done

echo "package install failed after ${attempts} attempts" >&2
exit 1
