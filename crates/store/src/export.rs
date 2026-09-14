//! Export and import in the model format of design 01: one JSONL file per
//! entity. The export is the model, not the database file, so another
//! implementation can read it back.
//!
//! Only metadata is handled here. Raw payload bytes and blob bytes live in
//! the file store and are copied by the daemon alongside this metadata.

use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use genatrix_model::{Annotation, Blob, Handle, Item, Person, Raw, Thread};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::error::Result;
use crate::store::Store;

/// Counts written or read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportSummary {
    /// Raw records.
    pub raws: usize,
    /// Threads.
    pub threads: usize,
    /// Persons.
    pub persons: usize,
    /// Handles.
    pub handles: usize,
    /// Blobs.
    pub blobs: usize,
    /// Items, all versions.
    pub items: usize,
    /// Annotations.
    pub annotations: usize,
}

fn write_jsonl<T: Serialize>(path: &Path, rows: &[T]) -> Result<()> {
    let mut w = BufWriter::new(File::create(path)?);
    for row in rows {
        serde_json::to_writer(&mut w, row)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    Ok(())
}

fn read_jsonl<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let r = BufReader::new(File::open(path)?);
    let mut out = Vec::new();
    for line in r.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        out.push(serde_json::from_str(&line)?);
    }
    Ok(out)
}

impl Store {
    /// Write every entity to `dir` as JSONL. Plain text: the caller is
    /// responsible for telling the user so (design 09).
    pub fn export_to(&self, dir: impl AsRef<Path>) -> Result<ExportSummary> {
        let dir = dir.as_ref();
        fs::create_dir_all(dir)?;
        let raws = self.all_raw()?;
        let threads = self.all_threads()?;
        let persons = self.all_persons()?;
        let handles = self.all_handles()?;
        let blobs = self.all_blobs()?;
        let items = self.all_items()?;
        let annotations = self.all_annotations()?;
        write_jsonl(&dir.join("raws.jsonl"), &raws)?;
        write_jsonl(&dir.join("threads.jsonl"), &threads)?;
        write_jsonl(&dir.join("persons.jsonl"), &persons)?;
        write_jsonl(&dir.join("handles.jsonl"), &handles)?;
        write_jsonl(&dir.join("blobs.jsonl"), &blobs)?;
        write_jsonl(&dir.join("items.jsonl"), &items)?;
        write_jsonl(&dir.join("annotations.jsonl"), &annotations)?;
        Ok(ExportSummary {
            raws: raws.len(),
            threads: threads.len(),
            persons: persons.len(),
            handles: handles.len(),
            blobs: blobs.len(),
            items: items.len(),
            annotations: annotations.len(),
        })
    }

    /// Read an export directory into this store. Intended for an empty
    /// store; rows that already exist are skipped, not overwritten.
    pub fn import_from(&self, dir: impl AsRef<Path>) -> Result<ExportSummary> {
        let dir = dir.as_ref();
        let raws: Vec<Raw> = read_jsonl(&dir.join("raws.jsonl"))?;
        let threads: Vec<Thread> = read_jsonl(&dir.join("threads.jsonl"))?;
        let persons: Vec<Person> = read_jsonl(&dir.join("persons.jsonl"))?;
        let handles: Vec<Handle> = read_jsonl(&dir.join("handles.jsonl"))?;
        let blobs: Vec<Blob> = read_jsonl(&dir.join("blobs.jsonl"))?;
        let mut items: Vec<Item> = read_jsonl(&dir.join("items.jsonl"))?;
        let annotations: Vec<Annotation> = read_jsonl(&dir.join("annotations.jsonl"))?;

        // Parents before children: an item that supersedes another must
        // come after it. Sorting by id (ULID) gives insertion order.
        items.sort_by_key(|i| i.id);

        let mut summary = ExportSummary::default();
        for r in &raws {
            if self.insert_raw(r)? {
                summary.raws += 1;
            }
        }
        for t in &threads {
            if self.get_thread(t.id)?.is_none() {
                self.upsert_thread(t)?;
                summary.threads += 1;
            }
        }
        for p in &persons {
            if self.get_person(p.id)?.is_none() {
                self.upsert_person(p)?;
                summary.persons += 1;
            }
        }
        for h in &handles {
            if self.find_handle(h.kind, &h.value)?.is_none() {
                self.insert_handle(h)?;
                summary.handles += 1;
            }
        }
        for b in &blobs {
            if self.get_blob(&b.hash)?.is_none() {
                self.upsert_blob(b)?;
                summary.blobs += 1;
            }
        }
        for i in &items {
            if self.get_item(i.id)?.is_none() {
                self.insert_item(i)?;
                summary.items += 1;
            }
        }
        for a in &annotations {
            if self.get_annotation(a.id)?.is_none() {
                self.insert_annotation(a)?;
                summary.annotations += 1;
            }
        }
        Ok(summary)
    }
}
