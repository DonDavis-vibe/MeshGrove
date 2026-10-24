#!/usr/bin/env bash
# Lays out Meshgrove as a system-wide install under DEST_DIR/usr, the common ground of the
# .deb, the .rpm and the AppImage:
#   packaging/linux/stage.sh BIN_DIR DEST_DIR
# BIN_DIR holds meshgrove, meshgrove-step-mesher, meshgrove-thumbnailer and meshgrove-mcp.
set -euo pipefail
bin=$(realpath "$1")
dest=$(realpath -m "$2")
cd "$(dirname "$0")/../.."

for program in meshgrove meshgrove-step-mesher meshgrove-thumbnailer meshgrove-mcp; do
    install -Dm755 "$bin/$program" "$dest/usr/bin/$program"
    strip --strip-unneeded "$dest/usr/bin/$program"
done
mkdir -p "$dest/usr/share/applications" "$dest/usr/share/thumbnailers"
sed "s|@BIN@|/usr/bin|g" packaging/meshgrove.desktop > "$dest/usr/share/applications/meshgrove.desktop"
sed "s|@BIN@|/usr/bin|g" packaging/meshgrove.thumbnailer > "$dest/usr/share/thumbnailers/meshgrove.thumbnailer"
chmod 644 "$dest/usr/share/applications/meshgrove.desktop" "$dest/usr/share/thumbnailers/meshgrove.thumbnailer"
install -Dm644 packaging/mime/meshgrove-step.xml "$dest/usr/share/mime/packages/meshgrove-step.xml"
install -Dm644 packaging/linux/70-meshgrove-3dmouse.rules "$dest/usr/lib/udev/rules.d/70-meshgrove-3dmouse.rules"
for size in 16 24 32 48 64 128 256 512; do
    install -Dm644 "packaging/icons/meshgrove-$size.png" "$dest/usr/share/icons/hicolor/${size}x${size}/apps/meshgrove.png"
done
for doc in README.md LICENSE THIRD-PARTY-LICENSES.md; do
    install -Dm644 "$doc" "$dest/usr/share/doc/meshgrove/$doc"
done
