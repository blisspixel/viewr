#!/bin/sh

set -efu
umask 077
LC_ALL=C
export LC_ALL

REPOSITORY="blisspixel/viewr"
RELEASES_URL="https://github.com/$REPOSITORY/releases"
REQUESTED_VERSION="${VIEWR_VERSION:-latest}"
INSTALL_ROOT="${VIEWR_INSTALL_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/viewr}"
BIN_DIR="${VIEWR_BIN_DIR:-$HOME/.local/bin}"

fail() {
    printf 'viewr installer: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "required command not found: $1"
}

[ -n "${HOME:-}" ] || fail "HOME is not set"
need curl
need awk
need cmp
need grep
need install
need ln
need mktemp
need mkdir
need mv
need readlink
need rm
need sort
need tr
need uname
need unzip
need wc

case "$INSTALL_ROOT" in
    /*) ;;
    *) fail "VIEWR_INSTALL_ROOT must be an absolute path" ;;
esac
case "$BIN_DIR" in
    /*) ;;
    *) fail "VIEWR_BIN_DIR must be an absolute path" ;;
esac
[ "$INSTALL_ROOT" != "/" ] || fail "VIEWR_INSTALL_ROOT must not be the filesystem root"
[ "$BIN_DIR" != "/" ] || fail "VIEWR_BIN_DIR must not be the filesystem root"

os=$(uname -s)
arch=$(uname -m)
case "$os:$arch" in
    Linux:x86_64|Linux:amd64)
        target="x86_64-unknown-linux-gnu"
        ;;
    Darwin:x86_64|Darwin:amd64)
        target="x86_64-apple-darwin"
        ;;
    Darwin:arm64|Darwin:aarch64)
        target="aarch64-apple-darwin"
        ;;
    Linux:arm64|Linux:aarch64)
        fail "Linux ARM64 releases are not published yet; build from source instead"
        ;;
    *)
        fail "unsupported platform: $os $arch"
        ;;
esac

if [ "$REQUESTED_VERSION" = "latest" ]; then
    latest_url=$(curl --proto '=https' --tlsv1.2 -fsSL \
        -o /dev/null -w '%{url_effective}' "$RELEASES_URL/latest") ||
        fail "could not resolve the latest official GitHub release"
    tag=${latest_url##*/}
else
    tag=$REQUESTED_VERSION
    case "$tag" in
        v*) ;;
        *) tag="v$tag" ;;
    esac
fi

printf '%s\n' "$tag" | grep -Eq '^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)([-+][0-9A-Za-z.-]+)?$' ||
    fail "release tag is not a supported semantic version: $tag"
version=${tag#v}
archive="viewr-$version-$target.zip"
download_base="$RELEASES_URL/download/$tag"

temporary=$(mktemp -d "${TMPDIR:-/tmp}/viewr-install.XXXXXX") ||
    fail "could not create a temporary directory"
stage_dir=
temporary_link=
cleanup() {
    case "$stage_dir" in
        "$INSTALL_ROOT/releases/.installing-"*) rm -rf -- "$stage_dir" ;;
    esac
    case "$temporary_link" in
        "$BIN_DIR/.viewr-link-"*) rm -f -- "$temporary_link" ;;
    esac
    rm -rf -- "$temporary"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

archive_path="$temporary/$archive"
checksum_path="$temporary/$archive.sha256"
printf 'Downloading viewr %s for %s...\n' "$version" "$target"
curl --proto '=https' --tlsv1.2 -fL --connect-timeout 15 --retry 3 \
    -o "$archive_path" "$download_base/$archive" ||
    fail "release archive download failed"
curl --proto '=https' --tlsv1.2 -fL --connect-timeout 15 --retry 3 \
    -o "$checksum_path" "$download_base/$archive.sha256" ||
    fail "release checksum download failed"

checksum_line=$(tr -d '\r' < "$checksum_path")
set -- $checksum_line
[ "$#" -eq 2 ] || fail "release checksum has an invalid format"
expected_hash=$1
[ "$2" = "$archive" ] || fail "release checksum names a different archive"
printf '%s\n' "$expected_hash" | grep -Eq '^[0-9a-f]{64}$' ||
    fail "release checksum is not a lowercase SHA-256 digest"

if command -v sha256sum >/dev/null 2>&1; then
    sha256_tool=sha256sum
elif command -v shasum >/dev/null 2>&1; then
    sha256_tool=shasum
else
    fail "sha256sum or shasum is required to verify the release"
fi
file_sha256() {
    if [ "$sha256_tool" = "sha256sum" ]; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}
actual_hash=$(file_sha256 "$archive_path")
[ "$actual_hash" = "$expected_hash" ] || fail "release archive checksum mismatch"

prefix="viewr-$version-$target"
entries="$temporary/entries.txt"
unzip -Z1 "$archive_path" > "$entries" || fail "could not inspect the release archive"
[ -s "$entries" ] || fail "release archive is empty"
if awk 'seen[$0]++ { duplicate = 1 } END { exit !duplicate }' "$entries"; then
    fail "release archive contains a duplicate path"
fi
while IFS= read -r entry; do
    case "$entry" in
        "$prefix/"*) ;;
        *) fail "release archive contains an unexpected top-level path" ;;
    esac
    case "$entry" in
        *\\*|*:*|*//*|*/../*|*/./*|../*|/*|*/..|*/.|*/ )
            fail "release archive contains an unsafe path"
            ;;
    esac
done < "$entries"

extract_root="$temporary/extract"
mkdir -p "$extract_root"
unzip -q "$archive_path" -d "$extract_root" || fail "could not extract the release archive"
source_root="$extract_root/$prefix"
manifest="$source_root/release-manifest.json"
[ -f "$manifest" ] && [ ! -L "$manifest" ] ||
    fail "release archive is missing a regular release manifest"

manifest_records="$temporary/manifest-records.txt"
awk '
    BEGIN { in_files = 0; finished = 0; state = 0; count = 0; failed = 0 }
    $0 == "  \"files\": [" {
        if (in_files || finished || state != 0) failed = 1
        in_files = 1
        next
    }
    in_files && $0 == "  ]," {
        if (state != 0 || count == 0) failed = 1
        in_files = 0
        finished = 1
        next
    }
    !in_files { next }
    state == 0 {
        if ($0 != "    {") { failed = 1; next }
        state = 1
        next
    }
    state == 1 {
        value = $0
        if (sub(/^      \"mode\": \"/, "", value) != 1 ||
            sub(/\",$/, "", value) != 1 ||
            value !~ /^[0-7][0-7][0-7][0-7]$/) {
            failed = 1
        }
        mode = value
        state = 2
        next
    }
    state == 2 {
        value = $0
        if (sub(/^      \"path\": \"/, "", value) != 1 ||
            sub(/\",$/, "", value) != 1 ||
            value !~ /^[A-Za-z0-9._\/-]+$/) {
            failed = 1
        }
        path = value
        state = 3
        next
    }
    state == 3 {
        value = $0
        if (sub(/^      \"sha256\": \"/, "", value) != 1 ||
            sub(/\",$/, "", value) != 1 ||
            length(value) != 64 || value !~ /^[0-9a-f]+$/) {
            failed = 1
        }
        digest = value
        state = 4
        next
    }
    state == 4 {
        value = $0
        if (sub(/^      \"size\": /, "", value) != 1 ||
            value !~ /^(0|[1-9][0-9]*)$/) {
            failed = 1
        }
        size = value
        state = 5
        next
    }
    state == 5 {
        if ($0 != "    }," && $0 != "    }") failed = 1
        print mode "\t" path "\t" digest "\t" size
        count++
        state = 0
        next
    }
    END {
        if (failed || in_files || !finished || state != 0 || count == 0) exit 1
    }
' "$manifest" > "$manifest_records" || fail "release manifest has an invalid file list"

[ "$(grep -Fxc '  "package_name": "viewr",' "$manifest")" -eq 1 ] ||
    fail "release manifest has the wrong package name"
[ "$(grep -Fxc "  \"version\": \"$version\"" "$manifest")" -eq 1 ] ||
    fail "release manifest has the wrong version"
[ "$(grep -Fxc "  \"target\": \"$target\"," "$manifest")" -eq 1 ] ||
    fail "release manifest has the wrong target"

tab=$(printf '\t')
expected_entries="$temporary/expected-entries.txt"
printf '%s/release-manifest.json\n' "$prefix" > "$expected_entries"
while IFS="$tab" read -r record_mode relative record_hash record_size; do
    case "$relative" in
        ''|*\\*|*:*|*//*|../*|*/../*|*/..|./*|*/./*|*/.)
            fail "release manifest contains an unsafe path"
            ;;
        bin/*) required_mode=0755 ;;
        *) required_mode=0644 ;;
    esac
    [ "$record_mode" = "$required_mode" ] ||
        fail "release manifest has the wrong mode for $relative"
    printf '%s/%s\n' "$prefix" "$relative" >> "$expected_entries"
done < "$manifest_records"
sort "$entries" > "$temporary/entries.sorted"
sort "$expected_entries" > "$temporary/expected-entries.sorted"
cmp -s "$temporary/entries.sorted" "$temporary/expected-entries.sorted" ||
    fail "release archive file set does not match its manifest"

while IFS="$tab" read -r _ relative record_hash record_size; do
    payload="$source_root/$relative"
    [ -f "$payload" ] && [ ! -L "$payload" ] ||
        fail "release manifest names a missing regular file: $relative"
    actual_size=$(wc -c < "$payload" | tr -d '[:space:]')
    [ "$actual_size" = "$record_size" ] ||
        fail "release manifest size does not match $relative"
    [ "$(file_sha256 "$payload")" = "$record_hash" ] ||
        fail "release manifest checksum does not match $relative"
done < "$manifest_records"

for required in \
    "bin/viewr" \
    "bin/viewr-decode" \
    "LICENSE" \
    "NOTICE" \
    "README.md" \
    "THIRD_PARTY_LICENSES.txt" \
    "release-manifest.json"
do
    [ -f "$source_root/$required" ] && [ ! -L "$source_root/$required" ] ||
        fail "release archive is missing a required regular file: $required"
done
if [ "$os" = "Linux" ]; then
    for required in "assets/icon.svg" "assets/linux/viewr.desktop"; do
        [ -f "$source_root/$required" ] && [ ! -L "$source_root/$required" ] ||
            fail "release archive is missing a required regular file: $required"
    done
fi

releases_dir="$INSTALL_ROOT/releases"
release_dir="$releases_dir/$tag"
stage_dir="$releases_dir/.installing-$tag-$$"
mkdir -p "$releases_dir" "$BIN_DIR"
[ ! -e "$stage_dir" ] || fail "installer staging path already exists"
mkdir "$stage_dir"
install -m 0755 "$source_root/bin/viewr" "$stage_dir/viewr"
install -m 0755 "$source_root/bin/viewr-decode" "$stage_dir/viewr-decode"
install -m 0644 "$source_root/LICENSE" "$stage_dir/LICENSE"
install -m 0644 "$source_root/NOTICE" "$stage_dir/NOTICE"
install -m 0644 "$source_root/README.md" "$stage_dir/README.md"
install -m 0644 "$source_root/THIRD_PARTY_LICENSES.txt" "$stage_dir/THIRD_PARTY_LICENSES.txt"
install -m 0644 "$source_root/release-manifest.json" "$stage_dir/release-manifest.json"
printf 'repository=%s\nversion=%s\ntarget=%s\n' "$REPOSITORY" "$version" "$target" > "$stage_dir/.viewr-install"
staged_version=$("$stage_dir/viewr" --version) ||
    fail "staged binary did not report its version"
[ "$staged_version" = "viewr $version" ] ||
    fail "staged binary version does not match the selected release"
# Keep the report: doctor names the missing desktop library when a session
# cannot open a window, and a hidden reason leaves the user guessing.
doctor_report=$("$stage_dir/viewr" doctor 2>&1) || {
    printf '%s\n' "$doctor_report" >&2
    fail "staged binaries did not pass viewr doctor"
}

command_link="$BIN_DIR/viewr"
if [ -e "$command_link" ] || [ -L "$command_link" ]; then
    [ -L "$command_link" ] || fail "refusing to replace a non-symlink command: $command_link"
    existing_target=$(readlink "$command_link" || true)
    case "$existing_target" in
        "$releases_dir/"*) ;;
        *) fail "refusing to replace a symlink not owned by the viewr installer: $command_link" ;;
    esac
    existing_release=${existing_target%/viewr}
    [ "$existing_release" != "$existing_target" ] &&
        [ -f "$existing_release/.viewr-install" ] &&
        [ ! -L "$existing_release/.viewr-install" ] &&
        grep -Fqx "repository=$REPOSITORY" "$existing_release/.viewr-install" ||
        fail "refusing to replace a symlink without a valid viewr ownership marker"
fi
temporary_link="$BIN_DIR/.viewr-link-$$"
[ ! -e "$temporary_link" ] && [ ! -L "$temporary_link" ] ||
    fail "installer command staging path already exists"
ln -s "$release_dir/viewr" "$temporary_link" ||
    fail "could not stage the viewr command"

backup_dir=
if [ -e "$release_dir" ]; then
    [ -f "$release_dir/.viewr-install" ] && [ ! -L "$release_dir/.viewr-install" ] ||
        fail "refusing to replace an installation not owned by the viewr installer: $release_dir"
    grep -Fqx "repository=$REPOSITORY" "$release_dir/.viewr-install" ||
        fail "refusing to replace an installation with a foreign ownership marker"
    backup_dir="$releases_dir/.backup-$tag-$$"
    [ ! -e "$backup_dir" ] || fail "installer backup path already exists"
    mv "$release_dir" "$backup_dir" || fail "could not stage the previous installation"
    if ! mv "$stage_dir" "$release_dir"; then
        mv "$backup_dir" "$release_dir" || true
        rm -f -- "$temporary_link"
        fail "could not activate the new release; the previous release was restored"
    fi
else
    if ! mv "$stage_dir" "$release_dir"; then
        rm -f -- "$temporary_link"
        fail "could not activate the installed release"
    fi
fi

if ! mv -f -- "$temporary_link" "$command_link"; then
    rm -rf -- "$release_dir"
    if [ -n "$backup_dir" ]; then
        mv "$backup_dir" "$release_dir" || true
    fi
    fail "could not activate the viewr command; the previous release was restored when available"
fi
if [ -n "$backup_dir" ]; then
    rm -rf -- "$backup_dir"
fi

if [ "$os" = "Linux" ] && [ -f "$source_root/assets/linux/viewr.desktop" ]; then
    applications_dir="${XDG_DATA_HOME:-$HOME/.local/share}/applications"
    icons_dir="${XDG_DATA_HOME:-$HOME/.local/share}/icons/hicolor/scalable/apps"
    mkdir -p "$applications_dir" "$icons_dir"
    install -m 0644 "$source_root/assets/linux/viewr.desktop" \
        "$applications_dir/com.github.blisspixel.viewr.desktop"
    install -m 0644 "$source_root/assets/icon.svg" \
        "$icons_dir/com.github.blisspixel.viewr.svg"
    if command -v update-desktop-database >/dev/null 2>&1; then
        update-desktop-database "$applications_dir" >/dev/null 2>&1 || true
    fi
fi

installed_version=$("$release_dir/viewr" --version) || fail "installed binary did not report its version"
[ "$installed_version" = "viewr $version" ] ||
    fail "installed binary version does not match the selected release"
printf '%s\n' "Installed $installed_version in $release_dir"
if ! command -v viewr >/dev/null 2>&1; then
    printf '%s\n' "Add $BIN_DIR to PATH, then run: viewr"
else
    printf '%s\n' "Run: viewr"
fi
printf '%s\n' "Updates are explicit: run this installer again. viewr performs no background checks."
