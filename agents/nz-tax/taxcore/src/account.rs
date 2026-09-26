use serde::{Deserialize, Serialize};

use crate::gst::GstTreatment;
use crate::ids::AccountCode;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    Asset,
    Liability,
    Equity,
    Income,
    Expense,
}

impl AccountKind {
    /// Whether a positive (debit) balance is the natural state. Used only for
    /// presentation — the ledger itself is sign-agnostic.
    pub fn debit_normal(&self) -> bool {
        matches!(self, AccountKind::Asset | AccountKind::Expense)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Account {
    pub code: AccountCode,
    pub name: String,
    pub kind: AccountKind,
    pub parent: Option<AccountCode>,
    /// Applied when nothing more specific is known about a transaction. A
    /// default is a starting point for review, never a silent decision.
    pub default_gst_treatment: Option<GstTreatment>,
    pub active: bool,
}

impl Account {
    pub fn new(code: AccountCode, name: impl Into<String>, kind: AccountKind) -> Self {
        Account {
            code,
            name: name.into(),
            kind,
            parent: None,
            default_gst_treatment: None,
            active: true,
        }
    }

    pub fn with_gst(mut self, treatment: GstTreatment) -> Self {
        self.default_gst_treatment = Some(treatment);
        self
    }

    pub fn under(mut self, parent: AccountCode) -> Self {
        self.parent = Some(parent);
        self
    }
}
