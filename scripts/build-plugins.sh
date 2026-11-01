#!/usr/bin/env bash
# Builds the example plugins (examples/plugins/*) for wasm32-wasip2, one .wasm each:
#
#   scripts/build-plugins.sh               → target/plugins/wasm32-wasip2/release/*.wasm
#   scripts/build-plugins.sh --test-hooks  → target/plugins-test/…, with the tools only Meshgrove's
#                                            tests call (endless loop, memory hog)
#
# The host tests (crates/meshgrove-plugins/tests/hello_box.rs) take the test build, or the file
# named by MESHGROVE_TEST_PLUGIN.
#
# Needs `rustup target add wasm32-wasip2` once. A distribution's Rust (Fedora's /usr/bin/cargo and
# rustc) often comes first in PATH and has no wasm targets, so rustup's stable toolchain is used,
# with its rustc named explicitly: otherwise its cargo would still find the distribution's.
set -euo pipefail
root=$(realpath "$(dirname "$0")/..")
target_dir=$root/target/plugins
features=()
if [ "${1:-}" = "--test-hooks" ]; then
    target_dir=$root/target/plugins-test
    features=(--features test-hooks)
fi
rustup=$(command -v rustup || echo "$HOME/.cargo/bin/rustup")
cargo=(cargo)
if [ -x "$rustup" ]; then
    cargo=("$rustup" run stable cargo)
    RUSTC=$("$rustup" which --toolchain stable rustc)
    export RUSTC
fi
for manifest in "$root"/examples/plugins/*/Cargo.toml; do
    # ${a[@]+…}: an empty array under `set -u` in the bash 3 of macOS.
    "${cargo[@]}" build --manifest-path "$manifest" --target wasm32-wasip2 --release \
        --target-dir "$target_dir" ${features[@]+"${features[@]}"}
done
ls -l "$target_dir"/wasm32-wasip2/release/*.wasm
