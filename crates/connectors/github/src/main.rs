//! `genatrix-github`: one request on standard input, one reply on standard
//! output. See the library's documentation.

use std::io::Read as _;

fn main() {
    let mut input = String::new();
    let reply = match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => match serde_json::from_str(&input) {
            Ok(request) => genatrix_connector_github::handle(request),
            Err(e) => genatrix_connector_github::Reply::Refused {
                detail: format!("not a request: {e}"),
            },
        },
        Err(e) => genatrix_connector_github::Reply::Refused {
            detail: format!("could not read the request: {e}"),
        },
    };
    // The token was in `input`; it goes nowhere else.
    drop(input);
    println!(
        "{}",
        serde_json::to_string(&reply).unwrap_or_else(|_| "{\"result\":\"failed\"}".into())
    );
}
