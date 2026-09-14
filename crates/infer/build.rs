//! Re-emit the vendored MLX link flags for this binary.
//!
//! `crabllm-mlx` needs `-force_load` on its static archive so Objective-C
//! class metadata survives dead-stripping, and an rpath for the Swift
//! runtime. Cargo scopes `rustc-link-arg` to the package that emits it, so a
//! binary here would link with neither: no symbols, and no `libswift_*` at
//! load time. The vendored crate publishes the flags as build metadata (see
//! `vendor/crabllm/patches/`); this repeats them.

fn main() {
    println!("cargo:rerun-if-env-changed=DEP_CRABLLM_MLX_LINK_ARGS");
    match std::env::var("DEP_CRABLLM_MLX_LINK_ARGS") {
        Ok(args) if !args.is_empty() => {
            for arg in args.split('\u{1f}') {
                println!("cargo:rustc-link-arg={arg}");
            }
        }
        _ => println!(
            "cargo:warning=crabllm-mlx published no link flags; local inference will not work"
        ),
    }
}
