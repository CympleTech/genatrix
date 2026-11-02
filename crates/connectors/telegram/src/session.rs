//! The Telegram session as a value.
//!
//! The client library keeps its session behind a trait; its own storages
//! are memory and a plain `SQLite` file. Neither suits us: the session
//! holds the authorization key, which is the login itself, and design 05
//! wants it protected like a password. So the session lives in memory while
//! the connector runs and is handed to the core as JSON, which keeps it in
//! the encrypted store beside the sync cursors. (Design 05 named the
//! keychain; the store is protected by the keychain's master key and can
//! hold the peer cache too, which the keychain is the wrong place for.)

use std::sync::Mutex;

use futures::future::BoxFuture;
use grammers_session::types::{DcOption, PeerId, PeerInfo, UpdateState, UpdatesState};
use grammers_session::{Session, SessionData};
use serde::{Deserialize, Serialize};

/// Everything a session is, in a form that serializes.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// The datacenter the account lives in.
    pub home_dc: i32,
    /// Known datacenters, with authorization keys where a connection was made.
    pub dc_options: Vec<DcOption>,
    /// Cached peers, which the update stream needs to fill gaps.
    pub peers: Vec<PeerInfo>,
    /// The update sequence position.
    pub updates: UpdatesState,
}

impl std::fmt::Debug for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Snapshot")
            .field("home_dc", &self.home_dc)
            .field("dc_options", &self.dc_options.len())
            .field("peers", &self.peers.len())
            .finish_non_exhaustive()
    }
}

impl Snapshot {
    /// Whether an authorization key exists for the home datacenter: whether
    /// this session has ever signed in.
    #[must_use]
    pub fn signed_in(&self) -> bool {
        self.dc_options
            .iter()
            .any(|d| d.id == self.home_dc && d.auth_key.is_some())
    }
}

/// A session held in memory, exportable as a [`Snapshot`].
#[derive(Default)]
pub struct JsonSession(Mutex<SessionData>);

/// The one way this can fail: a lock poisoned by a panic elsewhere.
#[derive(Debug)]
pub struct Poisoned;

impl std::fmt::Display for Poisoned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("session lock is poisoned")
    }
}

impl std::error::Error for Poisoned {}

impl JsonSession {
    /// A session from a snapshot; a default one when there is none yet.
    #[must_use]
    pub fn from_snapshot(snapshot: Option<Snapshot>) -> Self {
        let mut data = SessionData::default();
        if let Some(s) = snapshot {
            data.home_dc = s.home_dc;
            data.dc_options = s.dc_options.into_iter().map(|d| (d.id, d)).collect();
            data.peer_infos = s.peers.into_iter().map(|p| (p.id(), p)).collect();
            data.updates_state = s.updates;
        }
        Self(Mutex::new(data))
    }

    /// The session as it is now.
    pub fn snapshot(&self) -> Result<Snapshot, Poisoned> {
        let data = self.0.lock().map_err(|_| Poisoned)?;
        Ok(Snapshot {
            home_dc: data.home_dc,
            dc_options: data.dc_options.values().cloned().collect(),
            peers: data.peer_infos.values().cloned().collect(),
            updates: data.updates_state.clone(),
        })
    }

    fn data(&self) -> Result<std::sync::MutexGuard<'_, SessionData>, Poisoned> {
        self.0.lock().map_err(|_| Poisoned)
    }
}

impl Session for JsonSession {
    type Error = Poisoned;

    fn home_dc_id(&self) -> Result<i32, Poisoned> {
        Ok(self.data()?.home_dc)
    }

    fn set_home_dc_id(&self, dc_id: i32) -> BoxFuture<'_, Result<(), Poisoned>> {
        Box::pin(async move {
            self.data()?.home_dc = dc_id;
            Ok(())
        })
    }

    fn dc_option(&self, dc_id: i32) -> Result<Option<DcOption>, Poisoned> {
        Ok(self.data()?.dc_options.get(&dc_id).cloned())
    }

    fn set_dc_option(&self, dc_option: &DcOption) -> BoxFuture<'_, Result<(), Poisoned>> {
        let dc_option = dc_option.clone();
        Box::pin(async move {
            self.data()?.dc_options.insert(dc_option.id, dc_option);
            Ok(())
        })
    }

    fn peer(&self, peer: PeerId) -> BoxFuture<'_, Result<Option<PeerInfo>, Poisoned>> {
        Box::pin(async move { Ok(self.data()?.peer_infos.get(&peer).cloned()) })
    }

    fn cache_peer(&self, peer: &PeerInfo) -> BoxFuture<'_, Result<(), Poisoned>> {
        let peer = peer.clone();
        Box::pin(async move {
            self.data()?
                .peer_infos
                .entry(peer.id())
                .or_insert_with(|| peer.clone())
                .extend_info(&peer);
            Ok(())
        })
    }

    fn updates_state(&self) -> BoxFuture<'_, Result<UpdatesState, Poisoned>> {
        Box::pin(async move { Ok(self.data()?.updates_state.clone()) })
    }

    fn set_update_state(&self, update: UpdateState) -> BoxFuture<'_, Result<(), Poisoned>> {
        Box::pin(async move {
            let mut data = self.data()?;
            match update {
                UpdateState::All(state) => data.updates_state = state,
                UpdateState::Primary { pts, date, seq } => {
                    data.updates_state.pts = pts;
                    data.updates_state.date = date;
                    data.updates_state.seq = seq;
                }
                UpdateState::Secondary { qts } => data.updates_state.qts = qts,
                UpdateState::Channel { id, pts } => {
                    data.updates_state.channels.retain(|c| c.id != id);
                    data.updates_state
                        .channels
                        .push(grammers_session::types::ChannelState { id, pts });
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_session_has_not_signed_in_and_survives_json() {
        let session = JsonSession::default();
        let snapshot = session.snapshot().unwrap();
        assert!(!snapshot.signed_in());
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back.home_dc, snapshot.home_dc);
        let again = JsonSession::from_snapshot(Some(back));
        assert_eq!(again.home_dc_id().unwrap(), snapshot.home_dc);
    }
}
