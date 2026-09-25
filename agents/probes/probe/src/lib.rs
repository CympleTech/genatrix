//! Tests the sandbox from the inside. Each message is a command; the
//! answer says what the agent could and could not do. Never installed.

use genatrix_agent_sdk::types::{Card, Filter, Purpose};
use genatrix_agent_sdk::{Guest, actions, blobs, export_agent, host, items, model, space, user};

struct Probe;

impl Guest for Probe {
    fn on_items(_ids: Vec<String>) -> Result<(), String> {
        Ok(())
    }

    fn on_message(text: String) -> Result<String, String> {
        let (cmd, arg) = text.split_once(':').unwrap_or((text.as_str(), ""));
        Ok(match cmd {
            "loop" => {
                let mut n: u64 = 0;
                loop {
                    n = std::hint::black_box(n.wrapping_add(1));
                }
            }
            "alloc" => {
                let v: Vec<u8> = vec![1; 512 << 20];
                format!("allocated {}", std::hint::black_box(v).len())
            }
            "panic" => panic!("probe panicked"),
            "file" => match std::fs::read_to_string(arg) {
                Ok(s) => format!("read {} bytes", s.len()),
                Err(e) => format!("no file: {e}"),
            },
            "dir" => match std::fs::read_dir(".") {
                Ok(_) => "listed".into(),
                Err(e) => format!("no dir: {e}"),
            },
            "env" => format!("{} variables", std::env::vars().count()),
            "time" => {
                let wall = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_millis());
                format!("{wall} {}", host::now_ms())
            }
            "query" => {
                let found = items::query(&Filter {
                    since_ms: None,
                    until_ms: None,
                    text: None,
                    limit: arg.parse().unwrap_or(1000),
                });
                found
                    .iter()
                    .map(|i| i.id.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            }
            "get" => items::get(arg).map_or_else(|| "absent".into(), |i| i.id),
            "blob" => blobs::text(arg).unwrap_or_else(|e| format!("refused: {e}")),
            "model" => {
                let purpose = if arg == "draft" {
                    Purpose::Draft
                } else {
                    Purpose::Summarize
                };
                model::call(purpose, &[user("hello")]).unwrap_or_else(|e| format!("refused: {e}"))
            }
            "propose" => {
                let card = Card {
                    title: "t".into(),
                    fields: Vec::new(),
                    evidence: Vec::new(),
                };
                actions::propose(arg, &card, "p").unwrap_or_else(|e| format!("refused: {e}"))
            }
            "sql" => {
                space::execute(arg, &[]).map_or_else(|e| format!("refused: {e}"), |n| n.to_string())
            }
            _ => format!("unknown {cmd}"),
        })
    }

    fn on_schedule(_name: String) -> Result<(), String> {
        Ok(())
    }

    fn apply(_kind: String, _payload: String) -> Result<(), String> {
        let card = Card {
            title: "t".into(),
            fields: Vec::new(),
            evidence: Vec::new(),
        };
        actions::propose("note", &card, "from apply").map(|_| ())
    }
}

export_agent!(Probe with_types_in genatrix_agent_sdk);
