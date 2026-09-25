//! Write a Genatrix functional agent (design 11).
//!
//! An agent implements [`Guest`] and exports it:
//!
//! ```ignore
//! use genatrix_agent_sdk::{Guest, export_agent};
//!
//! struct Hello;
//!
//! impl Guest for Hello {
//!     fn on_items(ids: Vec<String>) -> Result<(), String> { Ok(()) }
//!     fn on_message(text: String) -> Result<String, String> { Ok(text) }
//!     fn on_schedule(_name: String) -> Result<(), String> { Ok(()) }
//!     fn apply(_kind: String, _payload: String) -> Result<(), String> { Ok(()) }
//! }
//!
//! export_agent!(Hello with_types_in genatrix_agent_sdk);
//! ```
//!
//! Everything the agent can reach is in the modules below. There is no
//! network and no file system; the core decides what each door returns.

#[allow(missing_docs, clippy::all)]
mod bindings {
    wit_bindgen::generate!({
        world: "agent",
        path: "../../wit",
        pub_export_macro: true,
        export_macro_name: "export_agent",
        default_bindings_module: "genatrix_agent_sdk",
    });
}

pub use bindings::Guest;
pub use bindings::genatrix::agent::{actions, blobs, host, items, model, space, types};
#[doc(hidden)]
pub use bindings::*;

/// A system message.
pub fn system(content: impl Into<String>) -> types::Message {
    types::Message {
        role: "system".into(),
        content: content.into(),
    }
}

/// A user message.
pub fn user(content: impl Into<String>) -> types::Message {
    types::Message {
        role: "user".into(),
        content: content.into(),
    }
}

/// Write a line to the run record.
pub fn log(line: impl AsRef<str>) {
    host::log(line.as_ref());
}
