#!/bin/sh
# Build every agent in this workspace, pack each with its manifest, and
# refresh the host's test fixtures. Needs the wasm32-wasip2 target.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
(cd "$here" && cargo build --release)
(cd "$root" && cargo build -q -p genatrix-host --bin genatrix-pack)
pack="$root/target/debug/genatrix-pack"
out="$here/target/packages"
mkdir -p "$out"
for dir in "$here/hello" "$here"/probes/*; do
    name=$(basename "$dir")
    wasm="$here/target/wasm32-wasip2/release/genatrix_agent_$name.wasm"
    "$pack" "$wasm" "$dir/manifest.toml" -o "$out/$name.wasm"
done
"$pack" "$here/target/wasm32-wasip2/release/genatrix_agent_nz_tax.wasm" \
    "$here/nz-tax/agent/manifest.toml" -o "$out/nz-tax.wasm"
for name in hello probe net nz-tax; do
    cp "$out/$name.wasm" "$root/crates/host/tests/fixtures/$name.wasm"
done
# The core's attachment reader is not a package: it is compiled into the
# core as it is.
cp "$here/target/wasm32-wasip2/release/genatrix_extract.wasm" "$root/crates/host/extractor.wasm"
