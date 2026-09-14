//! Development helper: mint an egress ticket for a request body.
//!
//! In the real system only the egress gate mints tickets, after it has
//! computed a level, redacted, and written the ledger entry. This example
//! exists so the gateway can be exercised by hand before that gate is built.
//! It is an example, not a binary: it never ships.
//!
//! ```text
//! echo '{"model":"local-qwen3","messages":[...]}' \
//!   | cargo run -p genatrix-llm --example mint_ticket -- \
//!       --target local-qwen3 --purpose classify --level personal
//! ```

use std::io::Read;

use genatrix_keys::TicketKey;
use genatrix_llm::ticket::{Purpose, Ticket, TicketLevel};

const KEY_ENV: &str = "GENATRIX_TICKET_KEY";

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn main() {
    let key = TicketKey::from_hex(
        &std::env::var(KEY_ENV).unwrap_or_else(|_| panic!("{KEY_ENV} is not set")),
    )
    .expect("ticket key must be 64 hex characters");

    let target = arg("--target").expect("--target <registry model name>");
    let purpose = match arg("--purpose").as_deref().unwrap_or("summarize") {
        "classify" => Purpose::Classify,
        "extract" => Purpose::Extract,
        "embed" => Purpose::Embed,
        "identity_suggestion" => Purpose::IdentitySuggestion,
        "summarize" => Purpose::Summarize,
        "draft" => Purpose::Draft,
        "translate" => Purpose::Translate,
        "search_rewrite" => Purpose::SearchRewrite,
        "plan" => Purpose::Plan,
        other => panic!("unknown purpose `{other}`"),
    };
    let level = match arg("--level").as_deref().unwrap_or("personal") {
        "public" => TicketLevel::Public,
        "redacted" => TicketLevel::Redacted,
        "personal" => TicketLevel::Personal,
        "secret" => TicketLevel::Secret,
        other => panic!("unknown level `{other}`"),
    };

    let mut body = Vec::new();
    std::io::stdin()
        .read_to_end(&mut body)
        .expect("read request body from stdin");

    let ticket =
        Ticket::issue(&body, target, "-", purpose, level, "mint_ticket example").expect("mint");
    println!("{}", ticket.encode(&key));
}
