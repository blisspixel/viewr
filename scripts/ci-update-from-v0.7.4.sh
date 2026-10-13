#!/usr/bin/env bash
# Install published v0.7.4 with its own installer, then update to the candidate
# archive already packaged in target/release-artifacts. No release is published.
set -euo pipefail

repo=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
cd "$repo"

candidate_dir="$repo/target/release-artifacts"
candidate_archive="$candidate_dir/viewr-0.7.4-x86_64-unknown-linux-gnu.zip"
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
  --pattern 'viewr-0.7.4-x86_64-unknown-linux-gnu.zip' \
  --pattern 'viewr-0.7.4-x86_64-unknown-linux-gnu.zip.sha256' \
  --pattern 'install.sh' \
  --dir "$published"

published_archive="$published/viewr-0.7.4-x86_64-unknown-linux-gnu.zip"
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
export XDG_CONFIG_HOME="$HOME/.config"
export XDG_DATA_HOME="$HOME/.local/share"
export VIEWR_INSTALL_ROOT="$sandbox/install"
export VIEWR_BIN_DIR="$sandbox/bin"
export TMPDIR="$sandbox/tmp"
mkdir -p "$HOME" "$XDG_CONFIG_HOME/viewr" "$VIEWR_BIN_DIR" "$TMPDIR"
unset DISPLAY || true
unset WAYLAND_DISPLAY || true

printf 'name\n' > "$XDG_CONFIG_HOME/viewr/folder-sort"
printf 'dark\n' > "$XDG_CONFIG_HOME/viewr/appearance"
printf 'en\n' > "$XDG_CONFIG_HOME/viewr/language"
cat > "$XDG_CONFIG_HOME/mimeapps.list" <<'EOF'
[Default Applications]
image/png=org.example.image.desktop
EOF

mime_query() {
  set +e
  output=$(xdg-mime query default image/png 2>&1)
  status=$?
  set -e
  printf '%s\n%s\n' "$status" "$output"
}
mime_before=$(mime_query)

sh "$published_installer"

old_desktop="$XDG_DATA_HOME/applications/com.github.blisspixel.viewr.desktop"
new_desktop="$XDG_DATA_HOME/applications/io.github.blisspixel.viewr.desktop"
old_icon="$XDG_DATA_HOME/icons/hicolor/scalable/apps/com.github.blisspixel.viewr.svg"
new_icon="$XDG_DATA_HOME/icons/hicolor/scalable/apps/io.github.blisspixel.viewr.svg"
[ -f "$old_desktop" ]
[ -f "$old_icon" ]
[ ! -e "$new_desktop" ]
[ ! -e "$new_icon" ]

VIEWR_VERSION=0.7.4 VIEWR_LOCAL_RELEASE="$candidate_dir" sh "$repo/install.sh"

[ -f "$new_desktop" ]
[ -f "$new_icon" ]
[ ! -e "$old_desktop" ]
[ ! -e "$old_icon" ]

installed_version=$("$VIEWR_INSTALL_ROOT/releases/v0.7.4/viewr" --version)
[ "$installed_version" = "viewr 0.7.4" ]

installed_hash=$(sha256sum "$VIEWR_INSTALL_ROOT/releases/v0.7.4/viewr" | awk '{print $1}')
candidate_hash=$(sha256sum "$candidate_binary" | awk '{print $1}')
[ "$installed_hash" = "$candidate_hash" ]

[ "$(cat "$XDG_CONFIG_HOME/viewr/folder-sort")" = "name" ]
[ "$(cat "$XDG_CONFIG_HOME/viewr/appearance")" = "dark" ]
[ "$(cat "$XDG_CONFIG_HOME/viewr/language")" = "en" ]

leftovers=$(find "$VIEWR_INSTALL_ROOT" \( -name '.installing-*' -o -name '.backup-*' \) -print)
[ -z "$leftovers" ]

mime_after=$(mime_query)
[ "$mime_before" = "$mime_after" ]

printf '%s\n' "updated published v0.7.4 to the candidate; image/png default unchanged"
