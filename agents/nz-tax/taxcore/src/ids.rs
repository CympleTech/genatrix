use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, TaxError};

/// UUIDv7 so ids sort by creation time, which keeps SQLite index writes local
/// and makes append-only tables readable without a join.
macro_rules! typed_id {
    ($name:ident) => {
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            pub fn new() -> Self {
                $name(Uuid::now_v7())
            }

            pub fn from_uuid(id: Uuid) -> Self {
                $name(id)
            }

            pub fn as_uuid(&self) -> &Uuid {
                &self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl FromStr for $name {
            type Err = TaxError;
            fn from_str(s: &str) -> Result<Self> {
                Uuid::parse_str(s)
                    .map($name)
                    .map_err(|_| TaxError::BadId(s.to_string()))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}({})", stringify!($name), self.0)
            }
        }
    };
}

typed_id!(DocumentId);
typed_id!(ExtractionId);
typed_id!(EntryId);
typed_id!(PostingId);
typed_id!(BankTxnId);
typed_id!(ReturnRunId);

/// Accounts are keyed by a stable human-chosen code (`5010-rent`), not a uuid:
/// they appear in rule files and reports where a uuid would be unreadable.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AccountCode(String);

impl AccountCode {
    pub fn new(code: impl Into<String>) -> Result<Self> {
        let code = code.into();
        let ok = !code.is_empty()
            && code.len() <= 64
            && code
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
        if !ok {
            return Err(TaxError::BadId(code));
        }
        Ok(AccountCode(code))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for AccountCode {
    type Err = TaxError;
    fn from_str(s: &str) -> Result<Self> {
        AccountCode::new(s)
    }
}

impl fmt::Display for AccountCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for AccountCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AccountCode({})", self.0)
    }
}
