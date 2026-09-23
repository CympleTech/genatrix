//! The models Genatrix runs, where they come from, and fetching them.
//!
//! Design: `docs/design/04-model-layer.md`, "模型下载". One built-in catalog,
//! each model pinned to a repository, a commit, the files to take and the
//! size and SHA-256 of every one. A download takes those files from that
//! commit and nothing else, checks each against its hash before putting it
//! in place, and resumes where it stopped. It is the only network activity
//! the core has of its own, and it happens only when the user asks.
//!
//! The fetching is `curl`, which every Mac has, rather than an HTTP stack in
//! the core: it resumes, follows redirects to the storage the repository
//! uses, and a download is a file growing on disk that progress can watch.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::system::System;

/// One file of a model: name, size in bytes, SHA-256 hex.
pub type File = (&'static str, u64, &'static str);

/// One model in the catalog.
#[derive(Debug)]
pub struct Model {
    /// Directory under `models/`, and the id the registry and the inference
    /// process know it by.
    pub id: &'static str,
    /// What it is for, in the words the page shows.
    pub role: &'static str,
    /// Where it comes from.
    pub repo: &'static str,
    /// The commit every file is taken from.
    pub revision: &'static str,
    /// Every file to take, and what it must be.
    pub files: &'static [File],
}

/// The catalog: the chat model and the embedder design 04 settled on, the
/// same weights the local model spike measured.
pub const CATALOG: &[Model] = &[
    Model {
        id: "qwen3-8b-4bit",
        role: "chat",
        repo: "mlx-community/Qwen3-8B-4bit",
        revision: "545dc4251c05440727734bcd94334791f6ab0192",
        files: &[
            (
                "added_tokens.json",
                707,
                "c0284b582e14987fbd3d5a2cb2bd139084371ed9acbae488829a1c900833c680",
            ),
            (
                "config.json",
                939,
                "e5485285fd7e289e76e9cffa112f6dc2e3426519082f7db9b69041589f81a218",
            ),
            (
                "merges.txt",
                1_671_853,
                "8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5",
            ),
            (
                "model.safetensors",
                4_607_835_174,
                "f2d29621aab300336ad645567ff38c42aac755513006ef4e8a579cf7ef5256d8",
            ),
            (
                "model.safetensors.index.json",
                64_065,
                "3fb25463b4078b1fc27159daa605190029c2e965f533bf0b1b594f96cbfceb8a",
            ),
            (
                "special_tokens_map.json",
                613,
                "76862e765266b85aa9459767e33cbaf13970f327a0e88d1c65846c2ddd3a1ecd",
            ),
            (
                "tokenizer.json",
                11_422_654,
                "aeb13307a71acd8fe81861d94ad54ab689df773318809eed3cbe794b4492dae4",
            ),
            (
                "tokenizer_config.json",
                9_706,
                "253153d0738ceb4c668d2eff957714dd2bea0b56de772a9fdccd96cbf517e6a0",
            ),
            (
                "vocab.json",
                2_776_833,
                "ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910",
            ),
        ],
    },
    Model {
        id: "multilingual-e5-small",
        role: "embed",
        repo: "intfloat/multilingual-e5-small",
        revision: "614241f622f53c4eeff9890bdc4f31cfecc418b3",
        files: &[
            (
                "config.json",
                655,
                "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
            ),
            (
                "model.safetensors",
                470_641_600,
                "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
            ),
            (
                "sentencepiece.bpe.model",
                5_069_051,
                "cfc8146abe2a0488e9e2a0c56de7952f7c11ab059eca145a0a727afce0db2865",
            ),
            (
                "special_tokens_map.json",
                167,
                "d05497f1da52c5e09554c0cd874037a083e1dc1b9cfd48034d1c717f1afc07a7",
            ),
            (
                "tokenizer.json",
                17_082_730,
                "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
            ),
            (
                "tokenizer_config.json",
                443,
                "a1d6bc8734a6f635dc158508bef000f8e2e5a759c7d92f984b2c86e5ff53425b",
            ),
        ],
    },
];

impl Model {
    /// Bytes of all its files.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.files.iter().map(|f| f.1).sum()
    }

    fn dir(&self, config: &Config) -> PathBuf {
        config.data_dir.join("models").join(self.id)
    }

    /// Bytes of it already in place: every file present at its full size.
    /// A file there at the right size was checked when it was put there.
    #[must_use]
    pub fn present(&self, config: &Config) -> u64 {
        let dir = self.dir(config);
        self.files
            .iter()
            .filter(|(name, size, _)| {
                std::fs::metadata(dir.join(name)).is_ok_and(|m| m.len() == *size)
            })
            .map(|f| f.1)
            .sum()
    }
}

/// The gateway configuration the catalog implies, for a data directory that
/// has none (design 04: nobody writes this file by hand any more).
#[must_use]
pub fn default_gateway_config(config: &Config) -> String {
    let run = config.data_dir.join("run");
    let infer = run.join("infer.sock");
    format!(
        "# Written by Genatrix from its built-in model catalog (design 04).\n\
         # It is yours to edit; Genatrix does not overwrite a file that is here.\n\
         socket = \"{gateway}\"\n\n\
         [[models]]\n\
         name = \"local\"\n\
         model = \"{chat}\"\n\
         context_length = 32768\n\
         purposes = [\"classify\", \"extract\", \"identity_suggestion\", \"summarize\", \"draft\", \"translate\", \"search_rewrite\", \"plan\"]\n\
         endpoint = {{ kind = \"local_socket\", path = \"{infer}\" }}\n\n\
         [[models]]\n\
         name = \"embed\"\n\
         model = \"{embed}\"\n\
         context_length = 512\n\
         purposes = [\"embed\"]\n\
         endpoint = {{ kind = \"local_socket\", path = \"{infer}\" }}\n",
        gateway = run.join("gateway.sock").display(),
        chat = CATALOG[0].id,
        embed = CATALOG[1].id,
        infer = infer.display(),
    )
}

/// Where a download stands, for the page.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Progress {
    /// A download is under way.
    pub running: bool,
    /// The file being fetched or checked, as `model/file`.
    pub current: String,
    /// `fetching` or `checking`.
    pub step: String,
    /// Bytes in place, over every model.
    pub done: u64,
    /// Bytes in all, over every model.
    pub total: u64,
    /// Why the last attempt stopped, when it did not finish.
    pub error: Option<String>,
}

/// Free bytes on the volume that holds the data directory.
pub fn free_space(dir: &Path) -> Option<u64> {
    // `df -k` on the directory; std has no statvfs, and this crate has no
    // unsafe code to call it with.
    let out = std::process::Command::new("df")
        .arg("-k")
        .arg(dir)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?;
    let line = text.lines().nth(1)?;
    let available: u64 = line.split_whitespace().nth(3)?.parse().ok()?;
    Some(available * 1024)
}

/// Bytes as gigabytes to one decimal, for a sentence.
#[allow(clippy::cast_precision_loss)] // shown to one decimal place
fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Start fetching whatever of the catalog is missing, in the background.
/// Refuses when one is already running or the disk cannot take it.
pub fn start(system: &Arc<System>) -> anyhow::Result<()> {
    let config = &system.config;
    let total: u64 = CATALOG.iter().map(Model::size).sum();
    let present: u64 = CATALOG.iter().map(|m| m.present(config)).sum();
    let missing = total - present;
    {
        let mut progress = system
            .download
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if progress.running {
            anyhow::bail!("a download is already under way");
        }
        std::fs::create_dir_all(config.data_dir.join("models"))?;
        // A little room beyond the files themselves, for the partial file
        // being checked and for the store to keep growing meanwhile.
        let need = missing + missing / 10;
        if let Some(free) = free_space(&config.data_dir.join("models"))
            && free < need
        {
            anyhow::bail!(
                "not enough disk: the models need {} more and {} is free",
                gb(need),
                gb(free)
            );
        }
        *progress = Progress {
            running: true,
            done: present,
            total,
            ..Progress::default()
        };
    }
    let system = Arc::clone(system);
    tokio::spawn(async move {
        let outcome = fetch_all(&system).await;
        let mut progress = system
            .download
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        progress.running = false;
        progress.step.clear();
        progress.current.clear();
        match outcome {
            Ok(()) => {
                progress.error = None;
                drop(progress);
                tracing::info!("models downloaded and checked");
                // The model side was off for want of weights; it can start now.
                if matches!(
                    *system.model_state.borrow(),
                    crate::models::ModelState::Off { .. }
                ) && let Err(e) = crate::models::start(Arc::clone(&system), &system.ticket_key)
                {
                    tracing::warn!(error = %e, "the model side did not start after the download");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "model download stopped");
                progress.error = Some(e.to_string());
            }
        }
    });
    Ok(())
}

async fn fetch_all(system: &Arc<System>) -> anyhow::Result<()> {
    for model in CATALOG {
        let dir = model.dir(&system.config);
        std::fs::create_dir_all(&dir)?;
        for (name, size, sha) in model.files {
            let target = dir.join(name);
            if std::fs::metadata(&target).is_ok_and(|m| m.len() == *size) {
                continue;
            }
            let label = format!("{}/{name}", model.id);
            fetch_one(system, model, name, *size, sha, &target, &label).await?;
        }
    }
    Ok(())
}

async fn fetch_one(
    system: &Arc<System>,
    model: &Model,
    name: &str,
    size: u64,
    sha: &str,
    target: &Path,
    label: &str,
) -> anyhow::Result<()> {
    let partial = target.with_extension(format!(
        "{}part",
        target
            .extension()
            .map(|e| format!("{}.", e.to_string_lossy()))
            .unwrap_or_default()
    ));
    let url = format!(
        "https://huggingface.co/{}/resolve/{}/{name}",
        model.repo, model.revision
    );
    set_step(system, label, "fetching");
    let before: u64 = CATALOG.iter().map(|m| m.present(&system.config)).sum();
    let mut child = tokio::process::Command::new("/usr/bin/curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--continue-at",
            "-",
        ])
        .arg("--output")
        .arg(&partial)
        .arg(&url)
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("could not start curl: {e}"))?;
    // Progress is the partial file growing.
    let status = loop {
        tokio::select! {
            status = child.wait() => break status?,
            () = tokio::time::sleep(std::time::Duration::from_millis(500)) => {
                let got = std::fs::metadata(&partial).map_or(0, |m| m.len());
                set_done(system, before + got.min(size));
            }
        }
    };
    if !status.success() {
        anyhow::bail!(
            "fetching {label} failed (curl exit {}); press download again to resume",
            status.code().unwrap_or(-1)
        );
    }
    set_step(system, label, "checking");
    let path = partial.clone();
    let digest = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
    .await??;
    if digest != sha {
        let _ = std::fs::remove_file(&partial);
        anyhow::bail!(
            "{label} did not match its pinned hash and was deleted; press download to fetch it again"
        );
    }
    std::fs::rename(&partial, target)?;
    set_done(system, before + size);
    Ok(())
}

fn set_step(system: &System, current: &str, step: &str) {
    let mut p = system
        .download
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    current.clone_into(&mut p.current);
    step.clone_into(&mut p.step);
}

fn set_done(system: &System, done: u64) {
    system
        .download
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .done = done;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_file_in_the_catalog_is_pinned_to_a_size_and_a_full_hash() {
        for model in CATALOG {
            assert_eq!(model.revision.len(), 40, "{}: a full commit", model.id);
            assert!(
                model.files.iter().any(|f| f.0 == "config.json"),
                "{}",
                model.id
            );
            for (name, size, sha) in model.files {
                assert!(*size > 0, "{name}");
                assert_eq!(sha.len(), 64, "{name}: a full SHA-256");
                assert!(sha.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
            }
        }
        assert_eq!(CATALOG[0].role, "chat");
        assert_eq!(CATALOG[1].role, "embed");
    }

    #[test]
    fn the_default_configuration_names_both_models_and_the_run_directory() {
        let config = Config::under("/tmp/genatrix-test");
        let text = default_gateway_config(&config);
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        let models = parsed["models"].as_array().unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models[0]["model"].as_str(), Some(CATALOG[0].id));
        assert_eq!(models[1]["purposes"].as_array().unwrap().len(), 1);
        assert!(text.contains("/tmp/genatrix-test/run/gateway.sock"));
    }
}
