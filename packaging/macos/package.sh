#!/usr/bin/env bash
# Builds Meshgrove-VERSION-macos-universal.dmg: one app bundle for Apple Silicon and Intel
# (lipo of both builds), ad-hoc signed, next to a link to /Applications to drag it onto.
#   packaging/macos/package.sh VERSION ARM64_BIN_DIR X86_64_BIN_DIR OUTPUT_DIR
# Run it after THIRD-PARTY-LICENSES.md is complete; it goes into the app.
set -euo pipefail
version=$1
arm=$(cd "$2" && pwd)
intel=$(cd "$3" && pwd)
out=$(cd "$4" && pwd)
cd "$(dirname "$0")/../.."

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
app="$work/dmg/Meshgrove.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
for program in meshgrove meshgrove-step-mesher; do
    lipo -create "$arm/$program" "$intel/$program" -output "$app/Contents/MacOS/$program"
done
sed "s/@VERSION@/$version/g" packaging/macos/Info.plist > "$app/Contents/Info.plist"

iconset="$work/meshgrove.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
    double=$((size * 2))
    cp "packaging/icons/meshgrove-$size.png" "$iconset/icon_${size}x${size}.png"
    if [[ -f "packaging/icons/meshgrove-$double.png" ]]; then
        cp "packaging/icons/meshgrove-$double.png" "$iconset/icon_${size}x${size}@2x.png"
    else
        sips -z $double $double packaging/icons/meshgrove-1024.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
    fi
done
iconutil -c icns "$iconset" -o "$app/Contents/Resources/meshgrove.icns"
cp LICENSE THIRD-PARTY-LICENSES.md "$app/Contents/Resources/"

# No Apple developer account yet: ad-hoc signatures, so Gatekeeper asks once on the first start.
codesign --force --sign - "$app/Contents/MacOS/meshgrove-step-mesher"
codesign --force --sign - "$app"
codesign --verify --strict "$app"

ln -s /Applications "$work/dmg/Applications"
hdiutil create -volname "Meshgrove $version" -srcfolder "$work/dmg" -fs HFS+ -format UDZO -ov \
    "$out/Meshgrove-$version-macos-universal.dmg"
