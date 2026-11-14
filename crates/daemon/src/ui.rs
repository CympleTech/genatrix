//! Interfaces (design 15): the built-in one compiled in, the user's in the
//! data directory, each kept by the hash of its manifest.
//!
//! An interface is a set of static files: a page, its script and style, its
//! pictures and fonts. Nothing in it is run by the core; it runs in the
//! shell's sandboxed frame in a browser. What the core decides is which
//! files a version is, whether you approved it, and which version each
//! device loads. A file is served only if the version's manifest names it
//! and its bytes still hash to what the manifest says.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::system::System;

/// The name of the built-in interface. It has no row in the store.
pub const DEFAULT: &str = "default";
/// The interface API version this core speaks (design 15, "数据接口").
pub const API_VERSION: u32 = 1;
/// At most this many files in one interface.
const MAX_FILES: usize = 500;
/// At most this many bytes in all.
const MAX_BYTES: u64 = 30 * 1024 * 1024;

/// The built-in interface's files, as the web build left them.
const DEFAULT_FILES: &[(&str, &[u8])] = &[
    ("index.html", include_bytes!("web/dist/ui/index.html")),
    ("app.js", include_bytes!("web/dist/ui/app.js")),
    ("app.css", include_bytes!("web/dist/ui/app.css")),
];

/// What a version is: the interface API it was written against, and each
/// file's hash.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The interface API version it needs.
    pub api: u32,
    /// Each file, by path within the interface, to the hex SHA-256 of its
    /// bytes.
    pub files: BTreeMap<String, String>,
}

impl Manifest {
    /// The manifest of a set of files.
    #[must_use]
    pub fn of<'a>(files: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Self {
        Self {
            api: API_VERSION,
            files: files
                .into_iter()
                .map(|(p, b)| (p.to_owned(), hex::encode(Sha256::digest(b))))
                .collect(),
        }
    }

    /// The version's hash: of the manifest's JSON, whose keys are in order.
    #[must_use]
    pub fn hash(&self) -> String {
        hex::encode(Sha256::digest(self.json().as_bytes()))
    }

    /// The manifest as it is kept.
    #[must_use]
    pub fn json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// The built-in interface's manifest and hash, worked out once.
fn builtin() -> &'static (Manifest, String) {
    static ONCE: OnceLock<(Manifest, String)> = OnceLock::new();
    ONCE.get_or_init(|| {
        let m = Manifest::of(DEFAULT_FILES.iter().copied());
        let h = m.hash();
        (m, h)
    })
}

/// The built-in interface's hash.
#[must_use]
pub fn default_hash() -> &'static str {
    &builtin().1
}

/// Whether a path within an interface is plain: segments of letters,
/// digits, dots, dashes and underscores, none starting with a dot.
#[must_use]
pub fn plain_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 200
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && !seg.starts_with('.')
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// The content type a file is served with; `None` for a kind an interface
/// may not hold.
#[must_use]
pub fn content_type(path: &str) -> Option<&'static str> {
    let ext = path.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    Some(match ext.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "txt" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

/// Where a version's files are kept.
fn dir_of(system: &System, hash: &str) -> PathBuf {
    system.config.data_dir.join("ui").join(hash)
}

/// One file of a version, if the version is known and its manifest names
/// the file, and the bytes on disk are still the bytes it names.
pub fn file(system: &System, hash: &str, path: &str) -> anyhow::Result<Option<Vec<u8>>> {
    if !plain_path(path) {
        return Ok(None);
    }
    if hash == default_hash() {
        return Ok(DEFAULT_FILES
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, b)| b.to_vec()));
    }
    let Some(json) = system.store.ui_manifest(hash)? else {
        return Ok(None);
    };
    let manifest: Manifest = serde_json::from_str(&json)?;
    let Some(want) = manifest.files.get(path) else {
        return Ok(None);
    };
    let bytes = match std::fs::read(dir_of(system, hash).join(path)) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if &hex::encode(Sha256::digest(&bytes)) != want {
        tracing::warn!(%hash, %path, "an interface file no longer matches its manifest; not served");
        return Ok(None);
    }
    Ok(Some(bytes))
}

/// Read a built interface from a directory: every file under it, checked.
pub fn read_dir(root: &Path) -> anyhow::Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in
            std::fs::read_dir(&dir).with_context(|| format!("reading {}", dir.display()))?
        {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!(
                    "{} is a link; an interface is plain files",
                    entry.path().display()
                );
            }
            if kind.is_dir() {
                stack.push(entry.path());
                continue;
            }
            let rel = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            let bytes = std::fs::read(entry.path())?;
            total += bytes.len() as u64;
            out.push((rel, bytes));
            if out.len() > MAX_FILES || total > MAX_BYTES {
                bail!(
                    "an interface is at most {MAX_FILES} files and {} MB",
                    MAX_BYTES >> 20
                );
            }
        }
    }
    check_files(&mut out)?;
    Ok(out)
}

/// Whether a set of files can be an interface: plain paths, kinds an
/// interface may hold, within the limits, an index.html at the top. Sorted
/// by path on the way.
pub fn check_files(files: &mut [(String, Vec<u8>)]) -> anyhow::Result<()> {
    let total: u64 = files.iter().map(|(_, b)| b.len() as u64).sum();
    if files.len() > MAX_FILES || total > MAX_BYTES {
        bail!(
            "an interface is at most {MAX_FILES} files and {} MB",
            MAX_BYTES >> 20
        );
    }
    for (rel, _) in files.iter() {
        if !plain_path(rel) {
            bail!("`{rel}` is not a plain path");
        }
        if content_type(rel).is_none() {
            bail!("`{rel}`: an interface holds pages, scripts, styles, pictures and fonts only");
        }
    }
    if !files.iter().any(|(p, _)| p == "index.html") {
        bail!("an interface needs an index.html at its top");
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(())
}

/// Keep a version's files and file it under an interface, waiting for
/// approval. Its hash.
pub fn file_version(
    system: &System,
    ui: &str,
    files: &[(String, Vec<u8>)],
    note: &str,
) -> anyhow::Result<String> {
    let manifest = Manifest::of(files.iter().map(|(p, b)| (p.as_str(), b.as_slice())));
    let hash = manifest.hash();
    if hash != default_hash() {
        let dir = dir_of(system, &hash);
        for (path, bytes) in files {
            let to = dir.join(path);
            if let Some(parent) = to.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(to, bytes)?;
        }
    }
    system.store.add_ui_version(
        ui,
        &hash,
        &manifest.json(),
        note.trim(),
        &chrono::Utc::now().to_rfc3339(),
    )?;
    Ok(hash)
}

/// A short name for an interface from its title, not yet taken.
pub fn new_name(system: &System, title: &str) -> anyhow::Result<String> {
    let base: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() || base == DEFAULT {
        "ui".to_owned()
    } else {
        base
    };
    let base: String = base.chars().take(32).collect();
    for n in 1..1000 {
        let name = if n == 1 {
            base.clone()
        } else {
            format!("{base}-{n}")
        };
        if system.store.ui(&name)?.is_none() {
            return Ok(name);
        }
    }
    bail!("no free name for this interface")
}

/// Make an interface whose first version is another's version in use, and
/// approve it: the same files, already approved once. Its name.
pub fn copy(system: &System, from: &str, title: &str, made_by: &str) -> anyhow::Result<String> {
    let title = title.trim();
    if title.is_empty() || title.chars().count() > 40 {
        bail!("give the copy a name of at most 40 characters");
    }
    let files: Vec<(String, Vec<u8>)> = if from == DEFAULT {
        DEFAULT_FILES
            .iter()
            .map(|(p, b)| ((*p).to_owned(), b.to_vec()))
            .collect()
    } else {
        let source = system
            .store
            .ui(from)?
            .with_context(|| format!("there is no interface `{from}`"))?;
        let hash = source
            .current
            .with_context(|| format!("`{from}` has no approved version to copy"))?;
        let manifest: Manifest = serde_json::from_str(
            &system
                .store
                .ui_manifest(&hash)?
                .context("the version to copy is not on file")?,
        )?;
        let mut out = Vec::new();
        for path in manifest.files.keys() {
            out.push((
                path.clone(),
                file(system, &hash, path)?.with_context(|| {
                    format!("`{path}` of `{from}` is missing or changed on disk")
                })?,
            ));
        }
        out
    };
    let name = new_name(system, title)?;
    let now = chrono::Utc::now().to_rfc3339();
    system.store.create_ui(&name, title, made_by, &now)?;
    let hash = file_version(system, &name, &files, &format!("copied from {from}"))?;
    system
        .store
        .approve_ui_version(&name, &hash, &format!("copy of {from}"), &now)?;
    // Its source comes along, as a project an executor may change. An
    // interface filed from a folder has none; the copy works without.
    let started = if from == DEFAULT {
        builtin_source().and_then(|s| make_project(system, &name, &Start::Builtin(&s)))
    } else if source_dir(system, from).exists() {
        make_project(system, &name, &Start::Copy(from))
    } else {
        Ok(())
    };
    if let Err(e) = started {
        tracing::warn!(ui = %name, error = %e, "the copy has no source project");
    }
    Ok(name)
}

/// The version a device loads: its interface's approved version in use, or
/// the built-in one.
pub fn load_for(system: &System, device: &str) -> anyhow::Result<(String, String)> {
    let chosen = system.store.device_ui(device)?;
    if let Some(name) = chosen.filter(|n| n != DEFAULT)
        && let Some(ui) = system.store.ui(&name)?
        && let Some(hash) = ui.current
    {
        return Ok((name, hash));
    }
    Ok((DEFAULT.to_owned(), default_hash().to_owned()))
}

/// The devices other than `device` that use an interface.
pub fn others_using(system: &System, ui: &str, device: &str) -> anyhow::Result<Vec<String>> {
    Ok(system
        .store
        .device_uis()?
        .into_iter()
        .filter(|(d, u)| u == ui && d != device)
        .map(|(d, _)| d)
        .collect())
}

// --- Design 15, "让 AI 改界面": each interface's source is a project ---

/// The built-in interface's source, as the web build packed it.
const SOURCE: &str = include_str!("web/dist/ui-source.json");

/// The built-in interface's source files, by path.
pub fn builtin_source() -> anyhow::Result<BTreeMap<String, String>> {
    Ok(serde_json::from_str(SOURCE)?)
}

/// The project an interface's source is: `ui-<interface>`.
#[must_use]
pub fn project_name(ui: &str) -> String {
    format!("{}{ui}", genatrix_dispatch::workspace::UI_PROJECT)
}

/// The interface a project is the source of, if it is one.
#[must_use]
pub fn ui_of_project<'a>(workspace: &str, project: &'a str) -> Option<&'a str> {
    (workspace == genatrix_dispatch::workspace::GENATRIX)
        .then(|| project.strip_prefix(genatrix_dispatch::workspace::UI_PROJECT))
        .flatten()
        .filter(|u| !u.is_empty())
}

/// Where an interface's source repository is.
#[must_use]
pub fn source_dir(system: &System, ui: &str) -> PathBuf {
    system.config.data_dir.join("ui").join("src").join(ui)
}

/// Where the packages every interface builds with are: beside the mirrors,
/// where the executor's group may read (design 15, ruling 1).
#[must_use]
pub fn deps_dir(system: &System) -> PathBuf {
    system.exec.config().root.join("mirrors").join("ui-deps")
}

/// Whether the packages are there to build with.
#[must_use]
pub fn deps_ready(system: &System) -> bool {
    deps_dir(system).join("node_modules/.bin/vite").exists()
}

fn git_bytes(dir: &Path, args: &[&str]) -> anyhow::Result<Vec<u8>> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("git is not there")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    Ok(String::from_utf8_lossy(&git_bytes(dir, args)?)
        .trim()
        .to_owned())
}

/// Where an interface's source starts from.
pub enum Start<'a> {
    /// The built-in interface's source, as compiled into this core.
    Builtin(&'a BTreeMap<String, String>),
    /// Another interface's source repository, as it is now.
    Copy(&'a str),
}

/// Start an interface's source repository and file it as a project in the
/// `genatrix` space, so work on it is a dispatch like any other. Its
/// executors see this source and the interface API, nothing else
/// (invariant 38): the space has no accounts, and the repository holds no
/// data.
pub fn make_project(system: &System, ui: &str, start: &Start<'_>) -> anyhow::Result<()> {
    let dir = source_dir(system, ui);
    if dir.exists() {
        bail!("`{}` is there already", dir.display());
    }
    std::fs::create_dir_all(dir.parent().context("no parent")?)?;
    match start {
        Start::Builtin(files) => {
            std::fs::create_dir_all(&dir)?;
            for (path, text) in *files {
                if !plain_source_path(path) {
                    bail!("`{path}` is not a plain path");
                }
                let to = dir.join(path);
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(to, text)?;
            }
            git(&dir, &["init", "--quiet", "-b", "main"])?;
        }
        Start::Copy(from) => {
            let src = source_dir(system, from);
            if !src.exists() {
                bail!("`{from}` has no source to copy");
            }
            let out = std::process::Command::new("git")
                .args(["clone", "--quiet", "--no-hardlinks"])
                .arg(&src)
                .arg(&dir)
                .output()?;
            if !out.status.success() {
                bail!(
                    "could not copy the source: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
            }
            git(&dir, &["remote", "remove", "origin"])?;
        }
    }
    git(&dir, &["config", "user.name", "Genatrix"])?;
    git(&dir, &["config", "user.email", "genatrix@genatrix.invalid"])?;
    // The interface API as this core speaks it, for whoever changes it.
    std::fs::write(dir.join("API.json"), crate::web::interface_schema())?;
    // The packages, shared and read only: the same link in every copy, so a
    // workspace cloned from the mirror has them too.
    let link = dir.join("node_modules");
    if link.symlink_metadata().is_ok() {
        std::fs::remove_file(&link).ok();
    }
    std::os::unix::fs::symlink(deps_dir(system).join("node_modules"), &link)?;
    git(&dir, &["add", "--all"])?;
    git(&dir, &["add", "--force", "node_modules"])?;
    let what = match start {
        Start::Builtin(_) => "Start from the built-in interface".to_owned(),
        Start::Copy(from) => format!("Start from interface {from}"),
    };
    if !git(&dir, &["status", "--porcelain"])?.is_empty() {
        git(&dir, &["commit", "--quiet", "-m", &what])?;
    }
    let config = system.exec.config();
    let executors: Vec<String> = config
        .executors
        .iter()
        .map(|e| format!("{:?}", e.name))
        .collect();
    if executors.is_empty() {
        bail!("no executors are set up in executors.toml");
    }
    let text = format!(
        "name = \"{name}\"\nworkspace = \"{space}\"\nrepo = {repo:?}\nbase = \"main\"\n\
         executors = [{executors}]\ncommands = [\"npm run build\", \"npm run check\"]\n\
         delegate_personal = true\n",
        name = project_name(ui),
        space = genatrix_dispatch::workspace::GENATRIX,
        repo = dir.to_string_lossy(),
        executors = executors.join(", "),
    );
    crate::executors::file_definition(&system.store, "project", &text)?;
    Ok(())
}

/// A path in an interface's source: plain segments, dot files allowed
/// (`.gitignore`, `.npmrc`).
fn plain_source_path(path: &str) -> bool {
    !path.is_empty()
        && path.split('/').all(|seg| {
            !seg.is_empty()
                && seg != "."
                && seg != ".."
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        })
}

/// The files in an interface's source that are Genatrix's, not the
/// interface's: how it is built and the rules for whoever changes it.
const MANAGED: &[&str] = &["GENATRIX.md", "vite.config.ts", ".npmrc", ".gitignore"];

/// Bring the files Genatrix keeps in an interface's source up to this
/// core's: the guide, the interface API, the build settings, and the build
/// and check scripts (design 15 v0.6). A source made by an earlier core
/// builds and reads like a new one. Committed when anything changed.
pub fn refresh_source(system: &System, ui: &str) -> anyhow::Result<bool> {
    let dir = source_dir(system, ui);
    if !dir.exists() {
        return Ok(false);
    }
    let builtin = builtin_source()?;
    for name in MANAGED {
        if let Some(text) = builtin.get(*name) {
            std::fs::write(dir.join(name), text)?;
        }
    }
    std::fs::write(dir.join("API.json"), crate::web::interface_schema())?;
    let pkg_path = dir.join("package.json");
    if let (Ok(text), Some(want)) = (
        std::fs::read_to_string(&pkg_path),
        builtin.get("package.json"),
    ) {
        let mut pkg: serde_json::Value = serde_json::from_str(&text)?;
        let want: serde_json::Value = serde_json::from_str(want)?;
        if let (Some(scripts), Some(want)) = (
            pkg.get_mut("scripts").and_then(|v| v.as_object_mut()),
            want.get("scripts").and_then(|v| v.as_object()),
        ) {
            for key in ["build", "check"] {
                if let Some(v) = want.get(key) {
                    scripts.insert(key.to_owned(), v.clone());
                }
            }
        }
        std::fs::write(&pkg_path, serde_json::to_string_pretty(&pkg)? + "\n")?;
    }
    git(&dir, &["add", "--all"])?;
    if git(&dir, &["status", "--porcelain"])?.is_empty() {
        return Ok(false);
    }
    git(
        &dir,
        &[
            "commit",
            "--quiet",
            "-m",
            "Bring Genatrix's own files up to date",
        ],
    )?;
    Ok(true)
}

/// The built interface a commit left in `dist/`, filed as a version of
/// `ui` waiting for approval, and the source moved on to that commit so the
/// next change starts from it (design 15 v0.4). `filed_as` is the mirror's
/// ref for the work. Its hash.
pub fn file_from_commit(
    system: &System,
    ui: &str,
    mirror: &Path,
    filed_as: &str,
    tip: &str,
    note: &str,
) -> anyhow::Result<String> {
    let hash = build_at(system, ui, mirror, tip, note)?;
    system.store.set_ui_version_commit(ui, &hash, tip)?;
    let dir = source_dir(system, ui);
    let mirror_path = mirror.to_string_lossy().into_owned();
    git(
        &dir,
        &[
            "fetch",
            "--quiet",
            &mirror_path,
            &format!("+{filed_as}:refs/genatrix/work"),
        ],
    )?;
    move_source(system, ui, tip)?;
    Ok(hash)
}

/// Put an interface's source at a commit it already has: where the next
/// change starts. Nothing when the commit is not there.
pub fn move_source(system: &System, ui: &str, commit: &str) -> anyhow::Result<()> {
    let dir = source_dir(system, ui);
    if !dir.exists() || git(&dir, &["cat-file", "-e", &format!("{commit}^{{commit}}")]).is_err() {
        return Ok(());
    }
    git(&dir, &["checkout", "--quiet", "main"])?;
    git(&dir, &["reset", "--quiet", "--hard", commit])?;
    Ok(())
}

fn build_at(
    system: &System,
    ui: &str,
    mirror: &Path,
    tip: &str,
    note: &str,
) -> anyhow::Result<String> {
    let listing = git_bytes(mirror, &["ls-tree", "-r", "-z", tip, "--", "dist"])?;
    let mut files = Vec::new();
    for entry in listing.split(|b| *b == 0).filter(|e| !e.is_empty()) {
        let entry = String::from_utf8_lossy(entry);
        let Some((meta, path)) = entry.split_once('\t') else {
            continue;
        };
        let mut parts = meta.split_whitespace();
        let (mode, kind, sha) = (parts.next(), parts.next(), parts.next());
        if kind != Some("blob") || mode == Some("120000") {
            continue;
        }
        let Some(rel) = path.strip_prefix("dist/") else {
            continue;
        };
        let sha = sha.context("a tree entry without an id")?;
        files.push((
            rel.to_owned(),
            git_bytes(mirror, &["cat-file", "blob", sha])?,
        ));
    }
    if files.is_empty() {
        bail!("the work left no build in dist/; it has to run `npm run build` and commit dist/");
    }
    check_files(&mut files)?;
    file_version(system, ui, &files, note)
}

/// Put in place the packages every interface builds with: the built-in
/// interface's locked packages, installed once with the network, or copied
/// from a folder that has them; then readable by the executor's group.
pub fn deps(system: &System, from: Option<&Path>) -> anyhow::Result<PathBuf> {
    let dir = deps_dir(system);
    std::fs::create_dir_all(&dir)?;
    let source = builtin_source()?;
    for f in ["package.json", "package-lock.json"] {
        if let Some(text) = source.get(f) {
            std::fs::write(dir.join(f), text)?;
        }
    }
    let modules = dir.join("node_modules");
    if modules.exists() {
        std::fs::remove_dir_all(&modules)?;
    }
    let status = match from {
        Some(src) => std::process::Command::new("/bin/cp")
            .arg("-R")
            .arg(src)
            .arg(&modules)
            .status()?,
        None => std::process::Command::new("npm")
            .args(["ci", "--no-audit", "--no-fund"])
            .current_dir(&dir)
            .status()
            .context("npm is not there")?,
    };
    if !status.success() {
        bail!("the packages could not be put in {}", dir.display());
    }
    let path = dir.to_string_lossy().into_owned();
    for (program, args) in [
        (
            "/usr/bin/chgrp",
            vec!["-R", genatrix_exec::isolation::EXEC_GROUP, path.as_str()],
        ),
        ("/bin/chmod", vec!["-R", "g+rX,o-rwx", path.as_str()]),
    ] {
        let ok = std::process::Command::new(program)
            .args(&args)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            tracing::warn!(%program, "the executor's group may not read the packages; run `genatrix exec setup` first");
        }
    }
    Ok(dir)
}

/// Whether an interface has a source project an executor may change.
#[must_use]
pub fn has_project(system: &System, ui: &str) -> bool {
    crate::executors::definitions(&system.store).is_ok_and(|d| {
        d.projects.iter().any(|(p, _)| {
            p.name == project_name(ui) && p.workspace == genatrix_dispatch::workspace::GENATRIX
        })
    })
}

/// `genatrix ui`.
#[derive(Debug, clap::Subcommand)]
pub enum UiAction {
    /// File a built interface (a folder with an index.html at its top) as a
    /// new version of an interface, made if it is not there. It waits for
    /// approval in the shell's Interface panel.
    Add {
        /// The interface's short name.
        name: String,
        /// The folder the build left.
        dir: PathBuf,
        /// What it is called, for a new interface.
        #[arg(long)]
        title: Option<String>,
        /// What changed, in a line.
        #[arg(long, default_value = "")]
        note: String,
    },
    /// The interfaces, their versions, and which device uses which.
    List,
    /// Put in place the packages every interface builds with, so an
    /// executor can build one with no network. Installs them with npm, or
    /// copies a `node_modules` folder that already has them.
    Deps {
        /// A `node_modules` folder to copy, such as the one in `web/`.
        #[arg(long)]
        from: Option<PathBuf>,
    },
}

/// Carry out `genatrix ui`.
pub fn terminal(system: &System, action: UiAction) -> anyhow::Result<()> {
    match action {
        UiAction::Add {
            name,
            dir,
            title,
            note,
        } => {
            if name == DEFAULT || !plain_path(&name) || name.contains('/') {
                bail!("`{name}` cannot name an interface");
            }
            let files = read_dir(&dir)?;
            if system.store.ui(&name)?.is_none() {
                system.store.create_ui(
                    &name,
                    title.as_deref().unwrap_or(&name),
                    "local",
                    &chrono::Utc::now().to_rfc3339(),
                )?;
            }
            let hash = file_version(system, &name, &files, &note)?;
            println!(
                "Filed {} files as version {} of `{name}`.",
                files.len(),
                &hash[..12]
            );
            println!("Look at it and approve it in the shell: Settings → Interface.");
        }
        UiAction::Deps { from } => {
            let dir = deps(system, from.as_deref())?;
            println!(
                "The packages interfaces build with are in {}.",
                dir.display()
            );
        }
        UiAction::List => {
            println!("{} (built in) {}", DEFAULT, &default_hash()[..12]);
            for u in system.store.uis()? {
                println!("{} \"{}\"", u.name, u.title);
                for v in system.store.ui_versions(&u.name)? {
                    let mark = if u.current.as_deref() == Some(v.hash.as_str()) {
                        "in use"
                    } else if v.approved_at.is_some() {
                        "approved"
                    } else {
                        "waiting"
                    };
                    println!("  {} {mark} {} {}", &v.hash[..12], v.created_at, v.note);
                }
            }
            for (device, ui) in system.store.device_uis()? {
                println!("device {device} uses {ui}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_in_an_interface_is_plain() {
        assert!(plain_path("index.html"));
        assert!(plain_path("assets/app-1.js"));
        assert!(!plain_path("../secret"));
        assert!(!plain_path("a/../b"));
        assert!(!plain_path(".env"));
        assert!(!plain_path("/etc/passwd"));
        assert!(!plain_path("a//b"));
        assert!(!plain_path("a b.js"));
        assert_eq!(content_type("x.exe"), None);
        assert_eq!(content_type("x.JS"), Some("text/javascript; charset=utf-8"));
    }

    #[test]
    fn a_version_is_its_files_and_nothing_else() {
        let a = Manifest::of([("index.html", b"<p>a" as &[u8]), ("app.js", b"1")]);
        let b = Manifest::of([("app.js", b"1" as &[u8]), ("index.html", b"<p>a")]);
        assert_eq!(a.hash(), b.hash(), "order does not matter");
        let c = Manifest::of([("index.html", b"<p>b" as &[u8]), ("app.js", b"1")]);
        assert_ne!(a.hash(), c.hash(), "a byte does");
        assert_eq!(default_hash().len(), 64);
    }

    #[test]
    fn a_directory_becomes_an_interface_only_if_it_is_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("app.js"), b"1").unwrap();
        assert!(read_dir(dir.path()).is_err(), "no index.html");
        std::fs::write(dir.path().join("index.html"), b"<p>").unwrap();
        std::fs::create_dir(dir.path().join("img")).unwrap();
        std::fs::write(dir.path().join("img/logo.svg"), b"<svg/>").unwrap();
        let files = read_dir(dir.path()).unwrap();
        assert_eq!(
            files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["app.js", "img/logo.svg", "index.html"]
        );
        std::fs::write(dir.path().join("run.sh"), b"rm").unwrap();
        assert!(
            read_dir(dir.path()).is_err(),
            "not an interface's kind of file"
        );
    }
}
