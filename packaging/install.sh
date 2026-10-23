#!/usr/bin/env bash
# Installs Meshgrove for the current user on Linux (no root needed):
#   ~/.local/bin                     meshgrove, meshgrove-step-mesher, meshgrove-thumbnailer
#   ~/.local/share/applications      menu entry
#   ~/.local/share/icons             icon
#   ~/.local/share/mime              model/step file type
#   ~/.local/share/thumbnailers      previews in Dolphin and file dialogs
#
#   packaging/install.sh             build from source (or take bin/ of a release package) + install
#   packaging/install.sh --uninstall remove everything again (settings, index and cache stay)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
PROGRAMS=(meshgrove meshgrove-step-mesher meshgrove-thumbnailer)
ICON_SIZES=(16 24 32 48 64 128 256 512)
FILES=(
    "${PROGRAMS[@]/#/$BIN/}"
    "$DATA/applications/meshgrove.desktop"
    "$DATA/mime/packages/meshgrove-step.xml"
    "$DATA/thumbnailers/meshgrove.thumbnailer"
)
for size in "${ICON_SIZES[@]}"; do
    FILES+=("$DATA/icons/hicolor/${size}x${size}/apps/meshgrove.png")
done

# Files of earlier versions: the former name "STL-Browser" (settings and index move over on the
# first start) and the old vector icon.
LEGACY=(
    "$BIN/stl-browser" "$BIN/stlb-step-mesher" "$BIN/stl-browser-thumbnailer"
    "$DATA/applications/stl-browser.desktop"
    "$DATA/icons/hicolor/scalable/apps/stl-browser.svg"
    "$DATA/icons/hicolor/scalable/apps/meshgrove.svg"
    "$DATA/mime/packages/stl-browser-step.xml"
    "$DATA/thumbnailers/stl-browser.thumbnailer"
)

refresh() {
    update-mime-database "$DATA/mime" 2>/dev/null || true
    update-desktop-database "$DATA/applications" 2>/dev/null || true
    gtk-update-icon-cache -q -t "$DATA/icons/hicolor" 2>/dev/null || true
    kbuildsycoca6 >/dev/null 2>&1 || true   # KDE menu cache
}

if [[ "${1:-}" == "--uninstall" ]]; then
    rm -f "${FILES[@]}" "${LEGACY[@]}"
    refresh
    echo "Meshgrove removed. Settings (~/.config/meshgrove), index (~/.local/share/meshgrove) and cache (~/.cache/meshgrove) were kept."
    exit 0
fi

if [[ -x bin/meshgrove ]]; then
    SOURCE=bin   # unpacked release package
else
    cargo build --release -p meshgrove -p meshgrove-step -p meshgrove-thumbnailer
    SOURCE=target/release
fi
rm -f "${LEGACY[@]}"
for program in "${PROGRAMS[@]}"; do
    install -Dm755 "$SOURCE/$program" "$BIN/$program"
done
for size in "${ICON_SIZES[@]}"; do
    install -Dm644 "packaging/icons/meshgrove-$size.png" "$DATA/icons/hicolor/${size}x${size}/apps/meshgrove.png"
done
install -Dm644 packaging/mime/meshgrove-step.xml "$DATA/mime/packages/meshgrove-step.xml"
mkdir -p "$DATA/applications" "$DATA/thumbnailers"
sed "s|@BIN@|$BIN|g" packaging/meshgrove.desktop > "$DATA/applications/meshgrove.desktop"
sed "s|@BIN@|$BIN|g" packaging/meshgrove.thumbnailer > "$DATA/thumbnailers/meshgrove.thumbnailer"
refresh
# Old "no preview possible" markers would keep Dolphin from asking our thumbnailer.
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/thumbnails/fail"
cat <<MSG
Installed. Start "Meshgrove" from the application menu.
Dolphin previews: Settings → Configure Dolphin → Interface → Previews: enable the STL/3MF/STEP
entries (if listed) and raise "Skip previews for remote files above"/"local files above" for big
models, then reopen the folder.
3D mouse (SpaceMouse): if Settings → 3D view says you may not read the device, install the udev
rule once (needs root), then replug the mouse:
  sudo install -m644 packaging/linux/70-meshgrove-3dmouse.rules /etc/udev/rules.d/ && sudo udevadm control --reload && sudo udevadm trigger
MSG
