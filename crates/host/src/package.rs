//! A package is one `.wasm` component. The manifest, prompts and fixtures
//! ride in custom sections, so the core reads them without running
//! anything, and the file's SHA-256 is the version (design 11).

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};
use wasmparser::{Parser, Payload};

use crate::HostError;
use crate::manifest::Manifest;

/// Custom section holding the manifest, as TOML.
pub const MANIFEST_SECTION: &str = "genatrix:manifest";
/// Custom section holding the prompts, as TOML: `name = "text"`.
pub const PROMPTS_SECTION: &str = "genatrix:prompts";
/// Custom section holding test inputs, as the author wrote them.
pub const FIXTURES_SECTION: &str = "genatrix:fixtures";

/// A package, read and checked.
#[derive(Clone, Debug)]
pub struct Package {
    /// The whole file.
    pub bytes: Vec<u8>,
    /// SHA-256 of `bytes`, hex. The version's identity.
    pub hash: String,
    /// What it may do.
    pub manifest: Manifest,
    /// Its prompts by name.
    pub prompts: BTreeMap<String, String>,
    /// Its fixtures, if it carries any.
    pub fixtures: Option<String>,
}

impl Package {
    /// Read a package without instantiating it.
    pub fn read(bytes: Vec<u8>) -> Result<Self, HostError> {
        let sections = top_level_sections(&bytes)?;
        let text = |name: &str| -> Result<Option<String>, HostError> {
            let found: Vec<&[u8]> = sections
                .iter()
                .filter(|(n, _)| n == name)
                .map(|(_, d)| d.as_slice())
                .collect();
            match found.as_slice() {
                [] => Ok(None),
                [one] => String::from_utf8(one.to_vec())
                    .map(Some)
                    .map_err(|_| HostError::Package(format!("{name} is not UTF-8"))),
                _ => Err(HostError::Package(format!("{name} appears more than once"))),
            }
        };
        let manifest = text(MANIFEST_SECTION)?
            .ok_or_else(|| HostError::Package("no manifest".into()))
            .and_then(|t| Manifest::parse(&t))?;
        let prompts = match text(PROMPTS_SECTION)? {
            Some(t) => toml::from_str(&t)
                .map_err(|e| HostError::Package(format!("prompts: {}", e.message())))?,
            None => BTreeMap::new(),
        };
        let fixtures = text(FIXTURES_SECTION)?;
        let hash = hex::encode(Sha256::digest(&bytes));
        Ok(Self {
            bytes,
            hash,
            manifest,
            prompts,
            fixtures,
        })
    }

    /// Make a package from a built component and its texts. Refuses a
    /// component that already carries any of Genatrix's sections, so a
    /// package cannot hide a second manifest.
    pub fn pack(
        component: &[u8],
        manifest: &str,
        prompts: Option<&str>,
        fixtures: Option<&str>,
    ) -> Result<Vec<u8>, HostError> {
        Manifest::parse(manifest)?;
        if let Some(p) = prompts {
            toml::from_str::<BTreeMap<String, String>>(p)
                .map_err(|e| HostError::Package(format!("prompts: {}", e.message())))?;
        }
        let existing = top_level_sections(component)?;
        if existing.iter().any(|(n, _)| n.starts_with("genatrix:")) {
            return Err(HostError::Package("the component is already packed".into()));
        }
        if !Parser::is_component(component) {
            return Err(HostError::Package("not a WASM component".into()));
        }
        let mut out = component.to_vec();
        append_custom(&mut out, MANIFEST_SECTION, manifest.as_bytes());
        if let Some(p) = prompts {
            append_custom(&mut out, PROMPTS_SECTION, p.as_bytes());
        }
        if let Some(f) = fixtures {
            append_custom(&mut out, FIXTURES_SECTION, f.as_bytes());
        }
        Ok(out)
    }
}

/// The component inside a package: the same bytes without Genatrix's
/// sections, ready to be packed again with another manifest.
pub fn unpack(bytes: &[u8]) -> Result<Vec<u8>, HostError> {
    let mut cut: Vec<std::ops::Range<usize>> = Vec::new();
    let mut depth = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| HostError::Package(e.to_string()))?;
        match payload {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(reader)
                if depth == 1 && reader.name().starts_with("genatrix:") =>
            {
                let body = reader.range();
                let mut size = Vec::new();
                leb128(&mut size, body.len());
                let start = body.start - size.len() - 1;
                if bytes.get(start) != Some(&0) || bytes[start + 1..body.start] != size[..] {
                    return Err(HostError::Package(
                        "a section header is not canonical".into(),
                    ));
                }
                cut.push(start..body.end);
            }
            _ => {}
        }
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    for r in cut {
        out.extend_from_slice(&bytes[at..r.start]);
        at = r.end;
    }
    out.extend_from_slice(&bytes[at..]);
    Ok(out)
}

/// The custom sections of the outermost component, not of the modules and
/// components nested inside it.
fn top_level_sections(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>, HostError> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    for payload in Parser::new(0).parse_all(bytes) {
        let payload = payload.map_err(|e| HostError::Package(e.to_string()))?;
        match payload {
            Payload::Version { .. } => depth += 1,
            Payload::End(_) => depth = depth.saturating_sub(1),
            Payload::CustomSection(reader) if depth == 1 => {
                out.push((reader.name().to_owned(), reader.data().to_vec()));
            }
            _ => {}
        }
    }
    Ok(out)
}

fn append_custom(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut body = Vec::with_capacity(name.len() + data.len() + 5);
    leb128(&mut body, name.len());
    body.extend_from_slice(name.as_bytes());
    body.extend_from_slice(data);
    out.push(0);
    leb128(out, body.len());
    out.extend_from_slice(&body);
}

fn leb128(out: &mut Vec<u8>, mut n: usize) {
    loop {
        let byte = u8::try_from(n & 0x7f).unwrap_or(0);
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}
