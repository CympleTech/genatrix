//! Built-in patterns for content that is secret wherever it appears.
//!
//! Design: `docs/design/02-trust-boundary.md`, the content rules.
//!
//! These serve twice: they raise an item's level during classification, and
//! they mark the spans that redaction removes before anything leaves the
//! device. A pattern that only matched during classification would let the
//! same digits through inside an otherwise personal message.
//!
//! Where a format carries a checksum, it is verified. A sixteen-digit number
//! that fails Luhn is not a card number, and treating it as one would mark
//! half the world's order numbers secret.

use std::sync::LazyLock;

use regex::Regex;

/// What kind of secret a span holds. The name appears in the reason attached
/// to a judgement, so it is part of what the user sees.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Secret {
    /// A one-time verification code.
    VerificationCode,
    /// A payment card number.
    CardNumber,
    /// A bank account or IBAN.
    BankAccount,
    /// A national identity number.
    NationalId,
    /// A password or passphrase stated in the text.
    Password,
    /// An API key or access token.
    ApiKey,
    /// A private key block.
    PrivateKey,
}

impl Secret {
    /// Stable name, used in reasons and in the ledger.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VerificationCode => "verification_code",
            Self::CardNumber => "card_number",
            Self::BankAccount => "bank_account",
            Self::NationalId => "national_id",
            Self::Password => "password",
            Self::ApiKey => "api_key",
            Self::PrivateKey => "private_key",
        }
    }

    /// The placeholder redaction leaves behind.
    #[must_use]
    pub const fn placeholder(self) -> &'static str {
        match self {
            Self::VerificationCode => "[code]",
            Self::CardNumber => "[card number]",
            Self::BankAccount => "[account number]",
            Self::NationalId => "[id number]",
            Self::Password => "[password]",
            Self::ApiKey => "[api key]",
            Self::PrivateKey => "[private key]",
        }
    }
}

/// One matched span of secret content.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Match {
    /// Which kind.
    pub kind: Secret,
    /// Byte range in the text.
    pub start: usize,
    /// End of the byte range.
    pub end: usize,
}

struct Pattern {
    kind: Secret,
    re: Regex,
    /// Which capture group holds the sensitive span. 0 means the whole match.
    group: usize,
    /// Extra check on the captured text; `None` accepts every match.
    check: Option<fn(&str) -> bool>,
}

fn digits(s: &str) -> Vec<u32> {
    s.chars().filter_map(|c| c.to_digit(10)).collect()
}

/// Luhn check, used by payment cards.
fn luhn(s: &str) -> bool {
    let d = digits(s);
    if d.len() < 13 || d.len() > 19 {
        return false;
    }
    let sum: u32 = d
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &n)| {
            if i % 2 == 1 {
                let x = n * 2;
                if x > 9 { x - 9 } else { x }
            } else {
                n
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

/// Checksum of a mainland China resident identity number.
fn cn_id_checked(s: &str) -> bool {
    const WEIGHTS: [u32; 17] = [7, 9, 10, 5, 8, 4, 2, 1, 6, 3, 7, 9, 10, 5, 8, 4, 2];
    const CHECK: [char; 11] = ['1', '0', 'X', '9', '8', '7', '6', '5', '4', '3', '2'];

    fn inner(s: &str) -> Option<bool> {
        let chars: Vec<char> = s.chars().filter(|c| !c.is_whitespace()).collect();
        if chars.len() != 18 {
            return Some(false);
        }
        let mut sum = 0;
        for (i, c) in chars[..17].iter().enumerate() {
            sum += c.to_digit(10)? * WEIGHTS[i];
        }
        Some(chars[17].to_ascii_uppercase() == CHECK[(sum % 11) as usize])
    }
    inner(s).unwrap_or(false)
}

static PATTERNS: LazyLock<Vec<Pattern>> = LazyLock::new(|| {
    let p = |kind, re: &str, group, check| Pattern {
        kind,
        re: Regex::new(re).expect("built-in pattern compiles"),
        group,
        check,
    };
    vec![
        // "-----BEGIN ... PRIVATE KEY-----" through the matching end line.
        p(
            Secret::PrivateKey,
            r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            0,
            None,
        ),
        // Provider-shaped keys and tokens.
        p(
            Secret::ApiKey,
            r"\b(?:sk-[A-Za-z0-9_-]{16,}|AKIA[0-9A-Z]{16}|gh[pousr]_[A-Za-z0-9]{20,}|xox[baprs]-[A-Za-z0-9-]{10,})\b",
            0,
            None,
        ),
        // A code announced as one. Requiring the word avoids matching every
        // six-digit number, which would swallow years, prices and counts.
        p(
            Secret::VerificationCode,
            r"(?i)(?:verification|security|auth(?:entication)?|one[- ]time|login|access)\s*code\D{0,20}?(\d[\d\s-]{3,9}\d)",
            1,
            None,
        ),
        p(
            Secret::VerificationCode,
            r"(?:验证码|校验码|动态密码|短信码)\D{0,10}?(\d{4,8})",
            1,
            None,
        ),
        p(
            Secret::VerificationCode,
            r"(?i)\bcode:\s*(\d[\d\s-]{3,9}\d)",
            1,
            None,
        ),
        // Payment cards: 13 to 19 digits in groups, checked with Luhn.
        p(
            Secret::CardNumber,
            r"\b(\d{4}[ -]?\d{4}[ -]?\d{4}[ -]?\d{1,7}|\d{13,19})\b",
            1,
            Some(luhn),
        ),
        // IBAN.
        p(
            Secret::BankAccount,
            r"\b[A-Z]{2}\d{2}[ ]?(?:[A-Z0-9]{4}[ ]?){2,7}[A-Z0-9]{1,4}\b",
            0,
            None,
        ),
        // An account number announced as one, including a masked tail.
        p(
            Secret::BankAccount,
            r"(?i)account(?:\s+number)?\s+(?:ending(?:\s+in)?\s+)?[#:]?\s*((?:\*{2,}|x{4,})?\d{4,})",
            1,
            None,
        ),
        p(
            Secret::BankAccount,
            r"(?:银行卡号|账号|帐号|卡号)\D{0,6}?([\d ]{10,25})",
            1,
            None,
        ),
        // Mainland China resident identity number, checksum verified.
        p(
            Secret::NationalId,
            r"\b(\d{17}[\dXx])\b",
            1,
            Some(cn_id_checked),
        ),
        p(
            Secret::NationalId,
            r"(?:身份证号?|证件号)\D{0,6}?(\d{17}[\dXx])",
            1,
            Some(cn_id_checked),
        ),
        // A password stated in the text.
        p(
            Secret::Password,
            r"(?i)\b(?:password|passphrase|passcode|pwd)\s*(?:is|:|=)\s*(\S{4,})",
            1,
            None,
        ),
        p(
            Secret::Password,
            r"(?:密码|口令)\s*(?:是|为|:|：)\s*(\S{4,})",
            1,
            None,
        ),
    ]
});

/// Every secret span in the text, sorted by position, without overlaps.
///
/// When two patterns cover the same bytes the earlier start wins, and on a
/// tie the longer span wins, so a card number inside an "account number ..."
/// phrase is redacted once rather than twice.
#[must_use]
pub fn scan(text: &str) -> Vec<Match> {
    let mut found: Vec<Match> = Vec::new();
    for pattern in PATTERNS.iter() {
        for caps in pattern.re.captures_iter(text) {
            let Some(m) = caps.get(pattern.group) else {
                continue;
            };
            if let Some(check) = pattern.check
                && !check(m.as_str())
            {
                continue;
            }
            found.push(Match {
                kind: pattern.kind,
                start: m.start(),
                end: m.end(),
            });
        }
    }
    found.sort_by_key(|m| (m.start, std::cmp::Reverse(m.end)));
    let mut out: Vec<Match> = Vec::new();
    for m in found {
        if out.last().is_some_and(|prev| m.start < prev.end) {
            continue;
        }
        out.push(m);
    }
    out
}

/// Whether the text holds anything secret. Cheaper to read than `scan`, and
/// the reason a call site cares is usually just this.
#[must_use]
pub fn has_secret(text: &str) -> bool {
    !scan(text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<Secret> {
        let mut k: Vec<Secret> = scan(text).into_iter().map(|m| m.kind).collect();
        k.sort_unstable();
        k.dedup();
        k
    }

    #[test]
    fn verification_codes_in_both_languages() {
        assert_eq!(
            kinds("Your verification code is 482913. It expires in 10 minutes."),
            [Secret::VerificationCode]
        );
        assert_eq!(
            kinds("【某银行】您的验证码是 728301，请勿告诉他人。"),
            [Secret::VerificationCode]
        );
        assert_eq!(kinds("Code: 771 902"), [Secret::VerificationCode]);
    }

    #[test]
    fn a_bare_six_digit_number_is_not_a_code() {
        assert!(scan("We shipped 482913 units in 2024.").is_empty());
        assert!(scan("The meeting is at 100200 in room 3.").is_empty());
    }

    #[test]
    fn cards_are_checked_with_luhn() {
        // A valid test number.
        assert_eq!(
            kinds("card 4111 1111 1111 1111 on file"),
            [Secret::CardNumber]
        );
        // Same shape, wrong checksum: an order number, not a card.
        assert!(
            !kinds("order 4111 1111 1111 1112 shipped").contains(&Secret::CardNumber),
            "a number failing Luhn must not be treated as a card"
        );
    }

    #[test]
    fn national_ids_are_checked_and_bare_digits_are_not() {
        // A checksum-valid sample identity number.
        assert!(kinds("身份证号 110101199003071233 请核对").contains(&Secret::NationalId));
        assert!(
            !kinds("流水号 110101199003071234 已入库").contains(&Secret::NationalId),
            "an 18-digit number with a wrong check character is not an id"
        );
    }

    #[test]
    fn keys_passwords_and_accounts() {
        assert_eq!(
            kinds("AWS key AKIAIOSFODNN7EXAMPLE was rotated"),
            [Secret::ApiKey]
        );
        assert_eq!(
            kinds("The password is Tr0ub4dor&3, change it after login."),
            [Secret::Password]
        );
        assert_eq!(kinds("wifi 密码是 blueheron2024"), [Secret::Password]);
        assert!(kinds("Refund to account ending 9032").contains(&Secret::BankAccount));
        assert_eq!(
            kinds("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"),
            [Secret::PrivateKey]
        );
    }

    #[test]
    fn ordinary_messages_match_nothing() {
        for text in [
            "Hey, are we still on for dinner Thursday?",
            "周四能不能改到下午三点？上午我要去接孩子。",
            "The build finished in 42 seconds and all 107 tests passed.",
            "Invoice 2026-0041 is attached; it covers September.",
        ] {
            assert!(scan(text).is_empty(), "false positive in: {text}");
        }
    }

    #[test]
    fn spans_do_not_overlap_and_are_ordered() {
        let text = "login code 482913, card 4111 1111 1111 1111, password is hunter22";
        let m = scan(text);
        assert_eq!(m.len(), 3);
        for w in m.windows(2) {
            assert!(w[0].end <= w[1].start, "spans overlap: {m:?}");
        }
        assert!(has_secret(text));
    }
}
