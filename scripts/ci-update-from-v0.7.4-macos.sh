#!/usr/bin/env bash
# Install published v0.7.4 with its own installer, then update to the candidate
# archive already packaged in target/release-artifacts. No release is published.
#
# This checks the portable Apple Silicon command. It does not register a file
# type, build the application bundle, or record an original-M1 run. The hosted
# macos-latest image is not that receipt.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$repo"

os=$(uname -s)
arch=$(uname -m)
[ "$os" = "Darwin" ] || {
  echo "this update check runs on macOS" >&2
  exit 1
}
case "$arch" in
  arm64|aarch64) ;;
  *)
    echo "this update check runs on Apple Silicon ($arch)" >&2
    exit 1
    ;;
esac

candidate_dir="$repo/target/release-artifacts"
candidate_archive="$candidate_dir/viewr-0.7.8-aarch64-apple-darwin.zip"
candidate_binary="$repo/target/release/viewr"
[ -f "$candidate_archive" ] || {
  echo "missing candidate archive: $candidate_archive" >&2
  exit 1
}
[ -f "$candidate_binary" ] || {
  echo "missing candidate binary: $candidate_binary" >&2
  exit 1
}
python -B scripts/release_artifact.py verify "$candidate_archive"

published=$(mktemp -d)
sandbox=$(mktemp -d)
cleanup() {
  rm -rf -- "$published" "$sandbox"
}
trap cleanup EXIT

gh release download v0.7.4 --repo blisspixel/viewr --clobber \
  --pattern 'viewr-0.7.4-aarch64-apple-darwin.zip' \
  --pattern 'viewr-0.7.4-aarch64-apple-darwin.zip.sha256' \
  --pattern 'install.sh' \
  --dir "$published"

published_archive="$published/viewr-0.7.4-aarch64-apple-darwin.zip"
published_installer="$published/install.sh"
gh release verify v0.7.4 --repo blisspixel/viewr
gh release verify-asset v0.7.4 "$published_archive" --repo blisspixel/viewr
gh release verify-asset v0.7.4 "$published_installer" --repo blisspixel/viewr
gh attestation verify "$published_archive" \
  --repo blisspixel/viewr \
  --signer-workflow blisspixel/viewr/.github/workflows/release.yml \
  --source-ref refs/tags/v0.7.4 \
  --deny-self-hosted-runners
gh attestation verify "$published_installer" \
  --repo blisspixel/viewr \
  --signer-workflow blisspixel/viewr/.github/workflows/release.yml \
  --source-ref refs/tags/v0.7.4 \
  --deny-self-hosted-runners
python -B scripts/release_artifact.py verify "$published_archive"

export HOME="$sandbox/home"
export VIEWR_INSTALL_ROOT="$sandbox/install"
export VIEWR_BIN_DIR="$sandbox/bin"
export TMPDIR="$sandbox/tmp"
preference_dir="$HOME/Library/Application Support/viewr"
mkdir -p "$preference_dir" "$VIEWR_BIN_DIR" "$TMPDIR"
printf 'name\n' > "$preference_dir/folder-sort"
printf 'dark\n' > "$preference_dir/appearance"
printf 'en\n' > "$preference_dir/language"

file_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

# The published installer defaults to the latest stable release. Pass 0.7.4
# so this job still installs that archive after a newer release exists.
VIEWR_VERSION=0.7.4 sh "$published_installer"

published_release="$VIEWR_INSTALL_ROOT/releases/v0.7.4/viewr"
[ -f "$published_release" ] || {
  echo "published install did not write $published_release" >&2
  exit 1
}
published_version=$("$published_release" --version)
[ "$published_version" = "viewr 0.7.4" ]
[ "$(readlink "$VIEWR_BIN_DIR/viewr")" = "$published_release" ]

VIEWR_VERSION=0.7.8 VIEWR_LOCAL_RELEASE="$candidate_dir" sh "$repo/install.sh"

installed_release="$VIEWR_INSTALL_ROOT/releases/v0.7.8/viewr"
installed_version=$("$installed_release" --version)
[ "$installed_version" = "viewr 0.7.8" ]
[ "$(readlink "$VIEWR_BIN_DIR/viewr")" = "$installed_release" ]
retained_version=$("$published_release" --version)
[ "$retained_version" = "viewr 0.7.4" ]

installed_hash=$(file_sha256 "$installed_release")
candidate_hash=$(file_sha256 "$candidate_binary")
if [ "$installed_hash" != "$candidate_hash" ]; then
  printf 'installed binary %s does not match candidate %s\n' "$installed_hash" "$candidate_hash" >&2
  exit 1
fi

[ "$(cat "$preference_dir/folder-sort")" = "name" ]
[ "$(cat "$preference_dir/appearance")" = "dark" ]
[ "$(cat "$preference_dir/language")" = "en" ]

leftovers=$(find "$VIEWR_INSTALL_ROOT" \( -name '.installing-*' -o -name '.backup-*' \) -print)
[ -z "$leftovers" ]
# Launch Services is outside this sandbox, so this is not a file-association
# acceptance check. It only fails if the portable installer starts writing a
# desktop entry or an application bundle.
associations=$(find "$HOME" "$VIEWR_INSTALL_ROOT" \( -name '*.desktop' -o -name '*.app' \) -print)
[ -z "$associations" ]

printf '%s\n' "updated published v0.7.4 to the candidate on Apple Silicon; preferences unchanged"
