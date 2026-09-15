//! Turning a person identifier back into a name, on this machine.

use std::sync::Arc;

use genatrix_model::PersonId;
use genatrix_store::Store;

use crate::caller::Names;

/// Names read from the store.
#[derive(Clone)]
pub struct StoreNames {
    store: Arc<Store>,
}

impl StoreNames {
    /// Read names from this store.
    #[must_use]
    pub const fn new(store: Arc<Store>) -> Self {
        Self { store }
    }
}

impl std::fmt::Debug for StoreNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("StoreNames")
    }
}

impl Names for StoreNames {
    fn display_name(&self, person: PersonId) -> Option<String> {
        match self.store.get_person(person) {
            Ok(found) => found.map(|p| p.display_name),
            Err(e) => {
                tracing::warn!(error = %e, %person, "could not read a name back");
                None
            }
        }
    }
}
