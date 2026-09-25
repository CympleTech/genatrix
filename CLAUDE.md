# Genatrix

An AI that runs only on the user's device, understands their digital life, and
acts only with their approval. Start with `docs/design/00-vision.md`.

## Language

`docs/design/` is written in Chinese: it is where design is discussed with the
author. Everything else is English, including `docs/plan/`, code, comments,
commit messages, and CI.

## Design first

Design documents 00–10 are settled; 11 (functional agents) is a draft awaiting the author's rulings. When code and design disagree, change
the design document first, in the same change. Every invariant listed in
`docs/design/02-trust-boundary.md` gets a test in the commit that introduces the
code it constrains.

Each crate's root doc comment names the design document it implements.

## Layout

Dependencies point downward only: daemon → agent → gate → ledger/store → model,
with `keys` at the bottom. The gate also depends on `llm` for the egress ticket
type. Connectors depend only on `model` and the connector protocol crate.

`vendor/crabllm/` is a pinned third-party tree with its own `CLAUDE.md` and
style rules. Those apply to that code, not to Genatrix. Local changes to it are
kept as patches under `vendor/crabllm/patches/` and recorded in
`vendor/crabllm/GENATRIX-VENDOR.md`. `vendor/async-imap/` is carried the same
way, for one accessor, and goes away when upstream ships it.

`apps/menubar/` is the Swift menu bar shell (design 09). It is built with
`swift build`, not Cargo, and holds no data and no keys: it starts the core,
shows what the core's local API says, and opens the page in a window.

`web/` is the interface (design 06): Svelte and Vite, phone first. Its build
lands in `crates/daemon/src/web/dist/` under fixed names and is compiled into
the core; the built files are committed, so changing `web/` means running
`npm run build` there and committing `dist/` in the same change. The core
serves the page and a JSON API; the page holds no state the core does not.

## Checks

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

All three must be clean. The first build compiles SQLCipher, OpenSSL, and the
MLX Swift package, which takes several minutes. When `web/` changes:

```sh
cd web && npm ci && npm run check && npm run build
```
