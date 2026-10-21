#!/usr/bin/env bash
# Builds and installs the STL browser for the current user (no root needed):
#   ~/.local/bin                     stl-browser, stlb-step-mesher, stl-browser-thumbnailer
#   ~/.local/share/applications      menu entry
#   ~/.local/share/icons             icon
#   ~/.local/share/mime              model/step file type
#   ~/.local/share/thumbnailers      previews in Dolphin and file dialogs
#
#   packaging/install.sh             build + install
#   packaging/install.sh --uninstall remove everything again (settings, index and cache stay)
set -euo pipefail
cd "$(dirname "$0")/.."
BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
FILES=(
    "$BIN/stl-browser" "$BIN/stlb-step-mesher" "$BIN/stl-browser-thumbnailer"
    "$DATA/applications/stl-browser.desktop"
    "$DATA/icons/hicolor/scalable/apps/stl-browser.svg"
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
    rm -f "${FILES[@]}"
    refresh
    echo "STL browser removed. Settings (~/.config/stl-browser), index (~/.local/share/stl-browser) and cache (~/.cache/stl-browser) were kept."
    exit 0
fi

cargo build --release -p stl-browser -p stlb-step -p stl-browser-thumbnailer
install -Dm755 target/release/stl-browser "$BIN/stl-browser"
install -Dm755 target/release/stlb-step-mesher "$BIN/stlb-step-mesher"
install -Dm755 target/release/stl-browser-thumbnailer "$BIN/stl-browser-thumbnailer"
install -Dm644 packaging/stl-browser.svg "$DATA/icons/hicolor/scalable/apps/stl-browser.svg"
install -Dm644 packaging/mime/stl-browser-step.xml "$DATA/mime/packages/stl-browser-step.xml"
mkdir -p "$DATA/applications" "$DATA/thumbnailers"
sed "s|@BIN@|$BIN|g" packaging/stl-browser.desktop > "$DATA/applications/stl-browser.desktop"
sed "s|@BIN@|$BIN|g" packaging/stl-browser.thumbnailer > "$DATA/thumbnailers/stl-browser.thumbnailer"
refresh
# Old "no preview possible" markers would keep Dolphin from asking our thumbnailer.
rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/thumbnails/fail"
cat <<MSG
Installed. Start "STL-Browser" from the application menu.
Dolphin previews: Settings → Configure Dolphin → Interface → Previews: enable the STL/3MF/STEP
entries (if listed) and raise "Skip previews for remote files above"/"local files above" for big
models, then reopen the folder.
MSG
