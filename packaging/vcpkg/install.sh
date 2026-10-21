#!/usr/bin/env bash
# Builds the C libraries that the Windows, macOS and AppImage packages link statically
# (libarchive and OpenCascade) with a pinned vcpkg release:
#   packaging/vcpkg/install.sh VCPKG_DIR TRIPLET...
# VCPKG_DIR may already hold an installed/ folder from a cache; the libraries end up in
# VCPKG_DIR/installed/<triplet>. The triplets live in packaging/vcpkg/triplets.
set -euo pipefail
VCPKG_TAG=2026.10.16
PORTS=(
    "libarchive[core,bzip2,lz4,lzma,zstd]"
    "opencascade[core]"
)
here=$(cd "$(dirname "$0")" && pwd)
root=$1
shift

mkdir -p "$root"
if [[ ! -e "$root/.vcpkg-root" ]]; then
    # init + fetch instead of clone: a restored installed/ folder may already be there.
    git -C "$root" init -q
    git -C "$root" fetch -q --depth 1 https://github.com/microsoft/vcpkg "refs/tags/$VCPKG_TAG"
    git -C "$root" checkout -q FETCH_HEAD
fi
if [[ -x "$root/vcpkg" || -x "$root/vcpkg.exe" ]]; then
    :
elif [[ "${OS:-}" == Windows_NT ]]; then
    (cd "$root" && cmd //c bootstrap-vcpkg.bat -disableMetrics)
else
    "$root/bootstrap-vcpkg.sh" -disableMetrics
fi

for triplet in "$@"; do
    "$root/vcpkg" install --triplet "$triplet" --overlay-triplets "$here/triplets" \
        --clean-after-build "${PORTS[@]}"
done
