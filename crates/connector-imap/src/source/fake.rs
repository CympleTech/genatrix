//! A mail server that exists only inside a test.
//!
//! It can do the things a real one does at the worst possible moment:
//! renumber a folder, drop a connection in the middle of a fetch, or receive
//! new mail while being read. Those are the cases that go wrong in the field
//! and cannot be arranged on demand against a real server.

#![expect(
    clippy::missing_panics_doc,
    reason = "a test fixture: a poisoned lock here means a failing test, which is the point"
)]

use std::sync::Mutex;

use genatrix_connector::Fault;

use super::{Fetched, Folder, MailSource};

/// A message sitting in a fake folder.
#[derive(Clone, Debug)]
pub struct Message {
    /// Its UID.
    pub uid: u32,
    /// The bytes.
    pub raw: Vec<u8>,
}

/// A folder in the fake server.
#[derive(Clone, Debug)]
pub struct FakeFolder {
    /// Name.
    pub name: String,
    /// Validity marker.
    pub uidvalidity: u32,
    /// Whether the engine should read it.
    pub wanted: bool,
    /// Its messages.
    pub messages: Vec<Message>,
}

/// A mail server in memory.
#[derive(Debug)]
pub struct FakeServer {
    state: Mutex<State>,
}

#[derive(Debug, Default)]
struct State {
    folders: Vec<FakeFolder>,
    /// Fail the next this many fetches, as a dropped connection would.
    fail_fetches: u32,
    /// How many fetch calls have been made, so a test can prove batching.
    fetch_calls: u32,
}

impl FakeServer {
    /// A server with one wanted folder holding these messages.
    #[must_use]
    pub fn with_messages(folder: &str, messages: &[(u32, &str)]) -> Self {
        Self {
            state: Mutex::new(State {
                folders: vec![FakeFolder {
                    name: folder.to_owned(),
                    uidvalidity: 1,
                    wanted: true,
                    messages: messages
                        .iter()
                        .map(|(uid, body)| Message {
                            uid: *uid,
                            raw: message_bytes(*uid, body),
                        })
                        .collect(),
                }],
                ..State::default()
            }),
        }
    }

    /// Add folders.
    pub fn add_folder(&self, folder: FakeFolder) {
        self.state.lock().unwrap().folders.push(folder);
    }

    /// New mail arrives while the engine is working.
    pub fn deliver(&self, folder: &str, uid: u32, body: &str) {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folders.iter_mut().find(|f| f.name == folder) {
            f.messages.push(Message {
                uid,
                raw: message_bytes(uid, body),
            });
        }
    }

    /// The server renumbers a folder, which invalidates every UID in it.
    pub fn renumber(&self, folder: &str, uidvalidity: u32) {
        let mut state = self.state.lock().unwrap();
        if let Some(f) = state.folders.iter_mut().find(|f| f.name == folder) {
            f.uidvalidity = uidvalidity;
            for (index, message) in f.messages.iter_mut().enumerate() {
                message.uid = u32::try_from(index).unwrap_or(0) + 1;
            }
        }
    }

    /// The next `count` fetches fail, as a dropped connection would.
    pub fn break_next_fetches(&self, count: u32) {
        self.state.lock().unwrap().fail_fetches = count;
    }

    /// How many times the engine asked for messages.
    #[must_use]
    pub fn fetch_calls(&self) -> u32 {
        self.state.lock().unwrap().fetch_calls
    }
}

impl MailSource for FakeServer {
    async fn folders(&self) -> Result<Vec<Folder>, Fault> {
        let state = self.state.lock().unwrap();
        Ok(state
            .folders
            .iter()
            .map(|f| Folder {
                name: f.name.clone(),
                uidvalidity: f.uidvalidity,
                count: u32::try_from(f.messages.len()).unwrap_or(u32::MAX),
                wanted: f.wanted,
            })
            .collect())
    }

    async fn uids(&self, folder: &str, above: Option<u32>) -> Result<Vec<u32>, Fault> {
        let state = self.state.lock().unwrap();
        let Some(f) = state.folders.iter().find(|f| f.name == folder) else {
            return Err(Fault::permanent("fake", format!("no folder {folder}")));
        };
        let mut uids: Vec<u32> = f
            .messages
            .iter()
            .map(|m| m.uid)
            .filter(|uid| above.is_none_or(|a| *uid > a))
            .collect();
        uids.sort_unstable();
        Ok(uids)
    }

    async fn fetch(&self, folder: &str, uids: &[u32]) -> Result<Vec<Fetched>, Fault> {
        let mut state = self.state.lock().unwrap();
        state.fetch_calls += 1;
        if state.fail_fetches > 0 {
            state.fail_fetches -= 1;
            return Err(Fault::transient("fake", "the connection dropped"));
        }
        let Some(f) = state.folders.iter().find(|f| f.name == folder) else {
            return Err(Fault::permanent("fake", format!("no folder {folder}")));
        };
        Ok(f.messages
            .iter()
            .filter(|m| uids.contains(&m.uid))
            .map(|m| Fetched {
                uid: m.uid,
                gmail_message_id: None,
                gmail_thread_id: None,
                raw: m.raw.clone(),
            })
            .collect())
    }
}

/// A plausible message, so normalization has something real to chew on.
fn message_bytes(uid: u32, body: &str) -> Vec<u8> {
    format!(
        "From: Someone <someone@example.com>\r\n\
         To: me@example.com\r\n\
         Subject: message {uid}\r\n\
         Date: Mon, 1 Sep 2026 10:00:00 +0800\r\n\
         Message-ID: <{uid}@example.com>\r\n\
         Content-Type: text/plain\r\n\
         \r\n\
         {body}\r\n"
    )
    .into_bytes()
}
