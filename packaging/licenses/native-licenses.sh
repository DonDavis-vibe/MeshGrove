#!/usr/bin/env bash
# Appends the licenses of the C libraries that a release package ships inside the program
# (Windows: static libarchive and its codecs from vcpkg; macOS: the dylibs bundled into the app).
#   packaging/licenses/native-licenses.sh windows|macos >> THIRD-PARTY-LICENSES.md
set -euo pipefail

section() { # name, license file
    printf '\n---\n\n## %s\n\n```text\n' "$1"
    cat "$2"
    printf '\n```\n'
}

printf '\n# Native libraries\n\nThese C libraries are built into this package.\n'
case "${1:-}" in
    windows)
        triplet="${VCPKGRS_TRIPLET:-x64-windows-static}"
        root="$VCPKG_ROOT"
        command -v cygpath >/dev/null && root=$(cygpath -u "$root")   # Git Bash on Windows
        for dir in "$root/installed/$triplet/share/"*/; do
            port=$(basename "$dir")
            [[ -f "$dir/copyright" && "$port" != vcpkg-* ]] && section "$port (vcpkg)" "$dir/copyright"
        done
        ;;
    macos)
        for formula in libarchive libb2 lz4 xz zstd; do
            prefix=$(brew --prefix "$formula")
            for file in "$prefix"/{COPYING,LICENSE,COPYRIGHT}*; do
                [[ -f "$file" ]] && section "$formula ($(basename "$file"))" "$file"
            done
        done
        ;;
    *)
        echo "usage: $0 windows|macos" >&2
        exit 1
        ;;
esac
