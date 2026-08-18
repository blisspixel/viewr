#!/usr/bin/env bash
# Install runner packages, retrying the transient failures hosted package
# mirrors produce.
#
# A stalled or briefly unreachable mirror is not a defect in this repository,
# but it fails a job exactly like one and has cancelled a release. Retrying
# absorbs that without hiding a real failure: a mirror that is genuinely broken
# still fails, just after three attempts instead of one, and the step timeout in
# each workflow remains the outer bound.
#
# Arguments are passed through to `apt-get install` unchanged, so a caller that
# wants --no-install-recommends passes it and one that does not, does not.
set -euo pipefail

if [ "$#" -eq 0 ]; then
  echo "usage: ci-install-packages.sh <apt-get install arguments>" >&2
  exit 2
fi

attempts=3
for attempt in $(seq 1 "$attempts"); do
  if sudo apt-get update && sudo apt-get install -y "$@"; then
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
