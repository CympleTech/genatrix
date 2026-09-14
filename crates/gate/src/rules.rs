//! The rule file: the deterministic part of sensitivity judgement.
//!
//! Design: `docs/design/02-trust-boundary.md`, "规则". Rules are data, not
//! code, in a file the user can read and change. They establish a floor; the
//! local model may raise it; only the user may lower it.
//!
//! Three rule kinds are data-shaped and live in the file: where an item came
//! from, what a mail header says, and who sent it. The fourth kind, content
//! patterns, is code: it needs checksums (Luhn, identity numbers) that no
//! table can express, and a user-editable regex that silently stops matching
//! is a bad way to lose a guarantee. The file can still switch each pattern
//! kind off, so nothing is hidden.

use std::collections::BTreeSet;
use std::path::Path;

use genatrix_model::{Connector, Level, ThreadKind};
use serde::{Deserialize, Serialize};

use crate::patterns::{self, Secret};

/// What is being judged. The pipeline fills this in from an item; the rules
/// never reach into storage themselves.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    /// Which connector produced it.
    pub connector: Connector,
    /// The kind of container it belongs to.
    pub thread_kind: ThreadKind,
    /// Normalized plain text.
    pub text: &'a str,
    /// Mail headers, lowercased names. Empty for other kinds.
    pub headers: &'a [(String, String)],
    /// Domain of the sender's address, lowercased, if there is one.
    pub sender_domain: Option<&'a str>,
}

/// The rules' verdict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Judgement {
    /// The floor this item sits at.
    pub level: Level,
    /// Why, in the order the rules fired. Shown to the user on hover.
    pub reasons: Vec<String>,
}

impl Judgement {
    /// A one-line reason for the annotation and the interface.
    #[must_use]
    pub fn reason(&self) -> String {
        if self.reasons.is_empty() {
            "default".to_owned()
        } else {
            self.reasons.join("; ")
        }
    }
}

/// Where an item came from.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceRule {
    /// Connector name, or absent for any.
    #[serde(default)]
    pub connector: Option<Connector>,
    /// Thread kind, or absent for any.
    #[serde(default)]
    pub thread_kind: Option<ThreadKind>,
    /// The baseline level for matching items.
    pub level: Level,
    /// Human-readable reason.
    pub reason: String,
}

/// What a mail header says.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HeaderRule {
    /// Header name, lowercase.
    pub name: String,
    /// Match when the header is present at all.
    #[serde(default)]
    pub present: bool,
    /// Match when the header's value equals this, case-insensitively.
    #[serde(default)]
    pub equals: Option<String>,
    /// Match when the header's value contains this, case-insensitively.
    #[serde(default)]
    pub contains: Option<String>,
    /// The level to apply.
    pub level: Level,
    /// Human-readable reason.
    pub reason: String,
}

/// Who sent it.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SenderRule {
    /// Any of these domain suffixes, lowercase, matched on label boundaries.
    pub domain_suffix: Vec<String>,
    /// The level to apply.
    pub level: Level,
    /// Human-readable reason.
    pub reason: String,
}

/// Which built-in content patterns are active.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PatternConfig {
    /// Pattern kinds switched off. Empty by default; switching one off is a
    /// deliberate act and shows up in the file.
    #[serde(default)]
    pub disabled: Vec<String>,
}

/// A complete rule file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RuleSet {
    /// Version of this rule file, recorded with every judgement so an old
    /// annotation can be told from a new one.
    pub version: String,
    /// Source rules; the first match classifies, so put specific ones first.
    #[serde(default)]
    pub source: Vec<SourceRule>,
    /// Header rules; every match applies, in file order.
    #[serde(default)]
    pub header: Vec<HeaderRule>,
    /// Sender rules; every match applies, in file order.
    #[serde(default)]
    pub sender: Vec<SenderRule>,
    /// Content pattern switches.
    #[serde(default)]
    pub patterns: PatternConfig,
}

/// Problems with a rule file.
#[derive(Debug, thiserror::Error)]
pub enum RuleError {
    /// The file could not be read.
    #[error("reading {path}: {source}")]
    Read {
        /// Path.
        path: String,
        /// Cause.
        source: std::io::Error,
    },
    /// The file is not valid TOML or does not match the schema.
    #[error("parsing {path}: {source}")]
    Parse {
        /// Path.
        path: String,
        /// Cause.
        source: toml::de::Error,
    },
    /// A pattern kind named in `disabled` does not exist. Refused rather
    /// than ignored: a typo would silently leave a pattern switched on that
    /// the user believes is off, or the reverse.
    #[error("unknown content pattern `{0}`")]
    UnknownPattern(String),
}

/// The rules shipped with Genatrix.
pub const DEFAULT_RULES: &str = include_str!("../rules/default.toml");

impl Default for RuleSet {
    fn default() -> Self {
        Self::builtin()
    }
}

impl RuleSet {
    /// The shipped defaults.
    ///
    /// # Panics
    ///
    /// If the shipped file is invalid, which a test prevents.
    #[must_use]
    pub fn builtin() -> Self {
        toml::from_str(DEFAULT_RULES).expect("shipped rules parse")
    }

    /// Read a rule file.
    pub fn load(path: &Path) -> Result<Self, RuleError> {
        let text = std::fs::read_to_string(path).map_err(|source| RuleError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let rules: Self = toml::from_str(&text).map_err(|source| RuleError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        rules.validate()?;
        Ok(rules)
    }

    /// Check the parts that parsing cannot.
    pub fn validate(&self) -> Result<(), RuleError> {
        let known: BTreeSet<&str> = ALL_SECRETS.iter().map(|s| s.as_str()).collect();
        for name in &self.patterns.disabled {
            if !known.contains(name.as_str()) {
                return Err(RuleError::UnknownPattern(name.clone()));
            }
        }
        Ok(())
    }

    /// Whether a built-in content pattern is switched on.
    #[must_use]
    pub fn pattern_is_enabled(&self, kind: Secret) -> bool {
        !self.patterns.disabled.iter().any(|d| d == kind.as_str())
    }

    /// Judge one item.
    ///
    /// Two kinds of statement live in the file, told apart by the level they
    /// carry rather than by a flag:
    ///
    /// - A rule that says `secret` **escalates**. Once anything has called an
    ///   item secret, nothing in the file can take that back.
    /// - A rule that says `public` or `personal` **classifies**: it answers
    ///   "what sort of thing is this". Later matches in the file override
    ///   earlier ones, so a header saying "this is a mailing list" overrides
    ///   a source rule saying "mail is personal". File order is the priority
    ///   order, and it is visible.
    ///
    /// With no matching rule an item is `Personal`: something nobody has a
    /// rule for is private until something says otherwise. Content patterns
    /// run last and can only escalate.
    #[must_use]
    pub fn judge(&self, c: &Candidate<'_>) -> Judgement {
        let mut reasons = Vec::new();
        let mut baseline = Level::default();
        let mut secret = false;

        let mut apply = |level: Level, reason: &str, reasons: &mut Vec<String>| {
            if level == Level::Secret {
                secret = true;
            } else {
                baseline = level;
            }
            reasons.push(reason.to_owned());
        };

        for rule in &self.source {
            if rule.connector.is_none_or(|x| x == c.connector)
                && rule.thread_kind.is_none_or(|x| x == c.thread_kind)
            {
                apply(rule.level, &rule.reason, &mut reasons);
                break;
            }
        }

        for rule in &self.header {
            let Some((_, value)) = c
                .headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(&rule.name))
            else {
                continue;
            };
            let hit = if let Some(expected) = &rule.equals {
                value.trim().eq_ignore_ascii_case(expected)
            } else if let Some(needle) = &rule.contains {
                value.to_lowercase().contains(&needle.to_lowercase())
            } else {
                rule.present
            };
            if hit {
                apply(rule.level, &rule.reason, &mut reasons);
            }
        }

        if let Some(domain) = c.sender_domain {
            let domain = domain.trim_end_matches('.').to_lowercase();
            for rule in &self.sender {
                if rule
                    .domain_suffix
                    .iter()
                    .any(|s| domain_matches(&domain, &s.to_lowercase()))
                {
                    apply(rule.level, &rule.reason, &mut reasons);
                }
            }
        }

        let mut level = if secret { Level::Secret } else { baseline };

        let mut kinds: Vec<Secret> = patterns::scan(c.text)
            .into_iter()
            .map(|m| m.kind)
            .filter(|k| self.pattern_is_enabled(*k))
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        if !kinds.is_empty() {
            level = level.escalate(Level::Secret);
            reasons.push(format!(
                "content: {}",
                kinds
                    .iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        Judgement { level, reasons }
    }
}

/// Every pattern kind, for validation and for the interface.
pub const ALL_SECRETS: &[Secret] = &[
    Secret::VerificationCode,
    Secret::CardNumber,
    Secret::BankAccount,
    Secret::NationalId,
    Secret::Password,
    Secret::ApiKey,
    Secret::PrivateKey,
];

/// Whether `domain` is `suffix` or a subdomain of it. Suffix matching on raw
/// strings would let `notmybank.com` match a rule for `mybank.com`.
fn domain_matches(domain: &str, suffix: &str) -> bool {
    domain == suffix
        || domain
            .strip_suffix(suffix)
            .is_some_and(|rest| rest.ends_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate<'a>(
        connector: Connector,
        thread_kind: ThreadKind,
        text: &'a str,
        headers: &'a [(String, String)],
        sender_domain: Option<&'a str>,
    ) -> Candidate<'a> {
        Candidate {
            connector,
            thread_kind,
            text,
            headers,
            sender_domain,
        }
    }

    fn header(name: &str, value: &str) -> (String, String) {
        (name.to_owned(), value.to_owned())
    }

    #[test]
    fn the_shipped_rules_are_valid() {
        let rules = RuleSet::builtin();
        rules.validate().unwrap();
        assert!(!rules.version.is_empty());
        assert!(!rules.source.is_empty());
    }

    #[test]
    fn an_item_nobody_has_a_rule_for_is_personal() {
        let rules = RuleSet::builtin();
        let j = rules.judge(&candidate(
            Connector::Telegram,
            ThreadKind::DirectChat,
            "are we still on for dinner?",
            &[],
            None,
        ));
        assert_eq!(j.level, Level::Personal);
    }

    #[test]
    fn a_public_channel_is_public_and_bulk_mail_is_public() {
        let rules = RuleSet::builtin();
        let channel = rules.judge(&candidate(
            Connector::Telegram,
            ThreadKind::Channel,
            "本周四晚八点直播分享 Rust 异步编程实践",
            &[],
            None,
        ));
        assert_eq!(channel.level, Level::Public);

        let headers = [header("List-Unsubscribe", "<https://example.com/u>")];
        let bulk = rules.judge(&candidate(
            Connector::Imap,
            ThreadKind::MailThread,
            "Our weekly digest of stories you might like.",
            &headers,
            Some("news.example.com"),
        ));
        assert_eq!(
            bulk.level,
            Level::Public,
            "a header rule classifies more specifically than a source rule"
        );
        assert!(bulk.reason().contains("bulk"), "{}", bulk.reason());
    }

    #[test]
    fn a_bank_newsletter_stays_secret() {
        // Bulk headers say "public", the sender says "secret". Secret wins,
        // whichever comes first in the file.
        let rules = RuleSet::builtin();
        let headers = [header("List-Unsubscribe", "<https://example.com/u>")];
        let j = rules.judge(&candidate(
            Connector::Imap,
            ThreadKind::MailThread,
            "Your monthly statement is ready to view.",
            &headers,
            Some("mail.chase.com"),
        ));
        assert_eq!(j.level, Level::Secret);
    }

    #[test]
    fn content_raises_a_public_newsletter_to_secret() {
        let rules = RuleSet::builtin();
        let headers = [header("List-Unsubscribe", "<https://example.com/u>")];
        let j = rules.judge(&candidate(
            Connector::Imap,
            ThreadKind::MailThread,
            "Your order shipped. Your verification code is 482913.",
            &headers,
            Some("shop.example.com"),
        ));
        assert_eq!(
            j.level,
            Level::Secret,
            "a rule floor never keeps content from escalating"
        );
        assert!(j.reason().contains("verification_code"), "{}", j.reason());
    }

    #[test]
    fn nothing_in_the_file_can_take_back_secret() {
        // A later rule saying "public" cannot undo an earlier rule saying
        // "secret", whatever the order in the file.
        let rules: RuleSet = toml::from_str(
            r#"
version = "test"
[[source]]
connector = "imap"
level = "public"
reason = "everything is public here"
[[header]]
name = "x-kind"
equals = "medical"
level = "secret"
reason = "medical"
[[header]]
name = "x-kind2"
present = true
level = "public"
reason = "would lower"
"#,
        )
        .unwrap();
        let headers = [header("x-kind", "medical"), header("x-kind2", "yes")];
        let j = rules.judge(&candidate(
            Connector::Imap,
            ThreadKind::MailThread,
            "nothing special",
            &headers,
            None,
        ));
        assert_eq!(j.level, Level::Secret);
    }

    #[test]
    fn sender_rules_match_on_label_boundaries() {
        let rules: RuleSet = toml::from_str(
            r#"
version = "test"
[[sender]]
domain_suffix = ["mybank.com"]
level = "secret"
reason = "bank"
"#,
        )
        .unwrap();
        let judge = |d: &str| {
            rules
                .judge(&candidate(
                    Connector::Imap,
                    ThreadKind::MailThread,
                    "hello",
                    &[],
                    Some(d),
                ))
                .level
        };
        assert_eq!(judge("mybank.com"), Level::Secret);
        assert_eq!(judge("mail.mybank.com"), Level::Secret);
        assert_eq!(judge("MyBank.com."), Level::Secret, "case and trailing dot");
        assert_eq!(
            judge("notmybank.com"),
            Level::Personal,
            "a suffix must not match across a label boundary"
        );
    }

    #[test]
    fn a_disabled_pattern_stops_escalating_and_a_typo_is_refused() {
        let mut rules = RuleSet::builtin();
        rules.patterns.disabled = vec!["verification_code".into()];
        rules.validate().unwrap();
        let j = rules.judge(&candidate(
            Connector::Telegram,
            ThreadKind::DirectChat,
            "your verification code is 482913",
            &[],
            None,
        ));
        assert_eq!(j.level, Level::Personal);

        rules.patterns.disabled = vec!["verifcation_code".into()];
        assert!(matches!(
            rules.validate(),
            Err(RuleError::UnknownPattern(_))
        ));
    }
}
