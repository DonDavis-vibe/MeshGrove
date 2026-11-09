# Test fixtures

`hello_box_api_0_1.wasm` is `examples/plugins/hello-box` as Meshgrove 0.8.3 shipped it, built
against plugin API 0.1 (without the test hooks). It shows that plugins installed before API 0.2
keep running (`tests/hello_box.rs`). To build it again:

```sh
git archive v0.8.3 Cargo.toml Cargo.lock crates examples | tar -x -C /tmp/mg-0.8.3
cargo build --manifest-path /tmp/mg-0.8.3/examples/plugins/hello-box/Cargo.toml \
    --target wasm32-wasip2 --release
```
