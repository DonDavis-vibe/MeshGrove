#!/usr/bin/env bash
# Builds Meshgrove-VERSION-x86_64.AppImage from a staged tree (packaging/linux/stage.sh). The
# programs must get by with the base system's libraries: the release workflow links libarchive
# and OpenCascade statically (packaging/vcpkg).
#   APPIMAGETOOL=… APPIMAGE_RUNTIME=… packaging/linux/appimage.sh STAGE_DIR VERSION OUTPUT_DIR
# APPIMAGETOOL is appimagetool, APPIMAGE_RUNTIME the type2 runtime it puts in front.
set -euo pipefail
stage=$(realpath "$1")
version=$2
out=$(realpath "$3")
here=$(cd "$(dirname "$0")" && pwd)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/Meshgrove.AppDir"
mkdir -p "$app"
cp -a "$stage/usr" "$app/"
# A thumbnailer inside an AppImage cannot be registered with the system.
rm -rf "$app/usr/share/thumbnailers"
install -m755 "$here/AppRun" "$app/AppRun"
sed 's|^Exec=.*|Exec=meshgrove %f|' "$stage/usr/share/applications/meshgrove.desktop" > "$app/meshgrove.desktop"
cp "$stage/usr/share/icons/hicolor/256x256/apps/meshgrove.png" "$app/meshgrove.png"
ln -s meshgrove.png "$app/.DirIcon"
APPIMAGE_EXTRACT_AND_RUN=1 ARCH=x86_64 "$APPIMAGETOOL" --no-appstream --runtime-file "$APPIMAGE_RUNTIME" \
    "$app" "$out/Meshgrove-$version-x86_64.AppImage"
