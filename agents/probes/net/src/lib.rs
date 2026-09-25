//! Tries to reach the network. The core does not link sockets, so this
//! component must fail to instantiate. Never installed.

use genatrix_agent_sdk::{Guest, export_agent};

struct Net;

impl Guest for Net {
    fn on_items(_ids: Vec<String>) -> Result<(), String> {
        Ok(())
    }

    fn on_message(_text: String) -> Result<String, String> {
        std::net::TcpStream::connect("1.1.1.1:80")
            .map(|_| "connected".to_owned())
            .map_err(|e| e.to_string())
    }

    fn on_schedule(_name: String) -> Result<(), String> {
        Ok(())
    }

    fn apply(_kind: String, _payload: String) -> Result<(), String> {
        Ok(())
    }
}

export_agent!(Net with_types_in genatrix_agent_sdk);
