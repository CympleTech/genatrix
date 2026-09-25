//! Make a package: a built component plus its manifest, prompts and
//! fixtures in custom sections (design 11).
//!
//! ```text
//! genatrix-pack <component.wasm> <manifest.toml> [--prompts <file>] [--fixtures <file>] -o <out.wasm>
//! ```

use std::process::ExitCode;

use genatrix_host::Package;

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("genatrix-pack: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<String, String> {
    let mut args = std::env::args().skip(1);
    let mut positional = Vec::new();
    let (mut prompts, mut fixtures, mut out) = (None, None, None);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--prompts" => prompts = args.next(),
            "--fixtures" => fixtures = args.next(),
            "-o" => out = args.next(),
            _ => positional.push(a),
        }
    }
    let [component, manifest] = positional.as_slice() else {
        return Err("usage: genatrix-pack <component.wasm> <manifest.toml> [--prompts f] [--fixtures f] -o <out.wasm>".into());
    };
    let out = out.ok_or("missing -o <out.wasm>")?;
    let read = |p: &str| std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"));
    let component = std::fs::read(component).map_err(|e| format!("{component}: {e}"))?;
    let manifest = read(manifest)?;
    let prompts = prompts.as_deref().map(read).transpose()?;
    let fixtures = fixtures.as_deref().map(read).transpose()?;
    let bytes = Package::pack(
        &component,
        &manifest,
        prompts.as_deref(),
        fixtures.as_deref(),
    )
    .map_err(|e| e.to_string())?;
    let package = Package::read(bytes).map_err(|e| e.to_string())?;
    std::fs::write(&out, &package.bytes).map_err(|e| format!("{out}: {e}"))?;
    Ok(format!("{out} {} {}", package.manifest.name, package.hash))
}
