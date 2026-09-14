# Vendored crabllm

Upstream: https://github.com/crabtalk/crabllm
Commit:   8dec5d8d80dd7e2e2d15adc180a3a61edb6b8b17
Date:     2026-08-16 05:58:45 +0800
License:  MIT OR Apache-2.0 (see LICENSE in this directory)

Why vendored: single upstream maintainer, no release cadence, and the `crabllm-mlx`
crate is not published. Genatrix pins this exact tree and maintains its own fixes.

Rules (design doc 04):
- Local changes are kept as patch files under `patches/` and applied on top of the
  upstream tree so they can be reviewed against upstream and offered back.
- Review upstream for security fixes once a quarter; merge selectively.
- Do not build the `crabllm` binary or the proxy crate; Genatrix embeds
  `crabllm-core`, `crabllm-provider`, and `crabllm-mlx` as libraries.

Patches applied:

- `patches/0001-mlx-publish-link-args.patch` — `crabllm-mlx` emits its linker
  flags with `cargo:rustc-link-arg`, which Cargo scopes to the package that
  emits it. A binary in any other package therefore linked without
  `-force_load` on `libCrabllmMlx.a` (no symbols) and without the Swift rpath
  (`Library not loaded: @rpath/libswift_Concurrency.dylib` at startup). The
  patch declares `links = "crabllm_mlx"` and publishes the same flags as build
  metadata so a dependent's build script can re-emit them. Worth offering
  upstream: it affects anyone consuming the crate as a library.
