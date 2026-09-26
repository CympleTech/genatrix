use thiserror::Error;

pub type Result<T> = std::result::Result<T, TaxError>;

#[derive(Debug, Error)]
pub enum TaxError {
    #[error("not a 3-letter currency code: {0}")]
    BadCurrency(String),

    #[error("cannot combine {left} with {right}")]
    CurrencyMismatch { left: String, right: String },

    #[error("amount overflowed 64-bit minor units")]
    AmountOverflow,

    #[error("divide by zero")]
    DivideByZero,

    #[error("journal entry does not balance: postings sum to {residual}")]
    Unbalanced { residual: String },

    #[error("journal entry needs at least two postings, got {0}")]
    TooFewPostings(usize),

    #[error("posting amounts must be in the ledger's base currency {base}, got {found}")]
    NotBaseCurrency { base: String, found: String },

    #[error("{0} is not a valid id")]
    BadId(String),

    #[error("date {0} falls outside tax year {1}")]
    DateOutsideTaxYear(String, String),

    #[error("invalid tax year: {0}")]
    BadTaxYear(String),
}
