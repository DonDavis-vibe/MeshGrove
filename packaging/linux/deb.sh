#!/usr/bin/env bash
# Builds the .deb from a staged tree (packaging/linux/stage.sh) on the Debian or Ubuntu
# release it is meant for: the dependencies (libarchive, OpenCascade, …) are read from the
# binaries with dpkg-shlibdeps, so they match that release's packages.
#   packaging/linux/deb.sh STAGE_DIR VERSION DISTRO OUTPUT_DIR
# DISTRO names the release, e.g. ubuntu24.04; it goes into the package version and file name.
set -euo pipefail
stage=$(realpath "$1")
version=$2
distro=$3
out=$(realpath "$4")
here=$(cd "$(dirname "$0")" && pwd)

root=$(mktemp -d)
work=$(mktemp -d)
trap 'rm -rf "$root" "$work"' EXIT
cp -a "$stage/." "$root/"
install -Dm644 "$here/debian-copyright" "$root/usr/share/doc/meshgrove/copyright"

# dpkg-shlibdeps wants to run inside a source package.
mkdir -p "$work/debian"
printf 'Source: meshgrove\n\nPackage: meshgrove\nArchitecture: any\n' > "$work/debian/control"
depends=$(cd "$work" && dpkg-shlibdeps -O "$root/usr/bin/meshgrove" "$root/usr/bin/meshgrove-step-mesher" \
    "$root/usr/bin/meshgrove-thumbnailer" | sed -n 's/^shlibs:Depends=//p')

mkdir -p "$root/DEBIAN"
(cd "$root" && find usr -type f -exec md5sum {} + | sort -k2) > "$root/DEBIAN/md5sums"
cat > "$root/DEBIAN/control" <<EOF
Package: meshgrove
Version: $version-1~$distro
Architecture: amd64
Maintainer: Davis (German Redneck Engineering) <260029956+DonDavis-vibe@users.noreply.github.com>
Installed-Size: $(du -sk --exclude=DEBIAN "$root" | cut -f1)
Depends: $depends
Recommends: poppler-utils
Section: graphics
Priority: optional
Homepage: https://github.com/DonDavis-vibe/MeshGrove
Description: Gallery for 3D-print files (STL, 3MF, STEP, OBJ, PLY, glTF) with previews
 Meshgrove browses folders of 3D-print files like a photo gallery: every STL,
 3MF, STEP, OBJ, PLY and glTF model, every model inside ZIP, 7z and RAR
 archives and every PDF gets a real preview. It has a 3D viewer with
 measuring, a searchable library with tags and notes, a duplicate finder, a
 print bed check and hands files over to the slicer. Includes thumbnails for
 Dolphin and other file managers.
EOF
dpkg-deb --root-owner-group -Zxz --build "$root" "$out/meshgrove_${version}-1_${distro}_amd64.deb"
