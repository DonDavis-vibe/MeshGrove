#!/usr/bin/env bash
# Appends the licenses of the C/C++ libraries that a release package carries:
#   packaging/licenses/native-licenses.sh vcpkg DIR >> THIRD-PARTY-LICENSES.md
#       Windows, macOS, AppImage: libarchive, its codecs and OpenCascade, linked statically from
#       vcpkg. DIR is vcpkg's installed/<triplet> folder.
#   packaging/licenses/native-licenses.sh system >> THIRD-PARTY-LICENSES.md
#       .deb and .rpm: these libraries come from the distribution instead.
set -euo pipefail

section() { # name, license file
    printf '\n---\n\n## %s\n\n```text\n' "$1"
    cat "$2"
    printf '\n```\n'
}

case "${1:-}" in
    vcpkg)
        root=${2:?usage: $0 vcpkg DIR}
        command -v cygpath >/dev/null && root=$(cygpath -u "$root")   # Git Bash on Windows
        printf '\n# Native libraries\n\nThese C and C++ libraries are built into this package.\n'
        for dir in "$root/share/"*/; do
            port=$(basename "$dir")
            # vcpkg's build helpers and the system OpenGL stub are not part of the programs.
            if [[ -f "$dir/copyright" && "$port" != vcpkg-* && "$port" != opengl ]]; then
                section "$port (vcpkg)" "$dir/copyright"
            fi
            if [[ "$port" == opencascade ]]; then
                # LGPL: say where the source code of this very version is.
                version=$(awk '$0 == "Package: opencascade" { p = 1 } p && /^Version:/ { print $2; exit }' \
                    "$root/../vcpkg/status")
                printf '\nOpenCascade %s is built into meshgrove-step-mesher. Its source code, with the vcpkg\nbuild files used, is attached to each Meshgrove release as `opencascade-%s-source.tar.gz`;\nupstream: https://github.com/Open-Cascade-SAS/OCCT\n' "$version" "$version"
            fi
        done
        ;;
    system)
        printf '\n# Native libraries\n\nlibarchive and OpenCascade come from the distribution'"'"'s own packages, under their\nlicenses as stated there.\n'
        ;;
    *)
        echo "usage: $0 vcpkg DIR | system" >&2
        exit 1
        ;;
esac
