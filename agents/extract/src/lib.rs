//! The core's attachment reader: text out of PDFs and plain text files
//! (design 11). Built into the core and run in the same sandbox as agents,
//! with no doors at all: a PDF is a stranger's file, and a parser that goes
//! wrong on one should break nothing but itself.

#[allow(missing_docs, clippy::all)]
mod bindings {
    wit_bindgen::generate!({ world: "extractor", path: "../../wit" });
}

/// The most text handed back, in characters.
const MAX_CHARS: usize = 200_000;

struct Extractor;

impl bindings::Guest for Extractor {
    fn extract(mime: String, bytes: Vec<u8>) -> Result<String, String> {
        let text = match mime.split(';').next().unwrap_or("").trim() {
            "application/pdf" => {
                pdf_extract::extract_text_from_mem(&bytes).map_err(|e| format!("pdf: {e}"))?
            }
            m if m.starts_with("text/") => String::from_utf8_lossy(&bytes).into_owned(),
            other => return Err(format!("no reader for {other}")),
        };
        Ok(tidy(&text))
    }
}

/// Collapse runs of blank lines and trailing spaces, and keep it bounded.
fn tidy(text: &str) -> String {
    let mut out = String::new();
    let mut blank = 0;
    for line in text.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push_str(line);
        out.push('\n');
        if out.len() > MAX_CHARS * 4 {
            break;
        }
    }
    out.trim().chars().take(MAX_CHARS).collect()
}

bindings::export!(Extractor with_types_in bindings);
