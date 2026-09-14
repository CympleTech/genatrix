//! Redaction: the reduction applied to personal content before it may be
//! sent to a cloud model.
//!
//! Design: `docs/design/02-trust-boundary.md`, "脱敏".
//!
//! What this is not: privacy. Redaction removes names, addresses and the
//! spans the pattern engine recognises. It cannot remove meaning, and "I am
//! having surgery next week" says what it says with every name gone. The
//! design is explicit about this and so is the interface. The only real
//! privacy switch is whether the cloud is enabled at all.
//!
//! Pseudonyms are stable within one request and nowhere else. The map exists
//! to turn the model's reply back into real names on this machine; it is
//! never written down (design 02, invariant 6).

use std::collections::BTreeMap;

use crate::patterns;

/// One identity to hide: a person, and every string that names them.
#[derive(Clone, Debug)]
pub struct Identity {
    /// Stable key for this person, usually a `PersonId` rendered as text.
    pub key: String,
    /// Every string that refers to them: display name, email, phone, handle.
    /// Longer strings are replaced first so an address is not half-replaced
    /// by the name inside it.
    pub names: Vec<String>,
}

/// The result of redacting one request.
#[derive(Clone, Debug)]
pub struct Redacted {
    /// The text to send.
    pub text: String,
    /// Pseudonym to identity key, for restoring the reply. Lives in memory
    /// for the length of the request and is never persisted.
    pub pseudonyms: BTreeMap<String, String>,
}

impl Redacted {
    /// Turn pseudonyms in a model's reply back into the real names.
    ///
    /// `resolve` maps an identity key to the name to show. Restoring happens
    /// on this machine, after the reply comes back.
    #[must_use]
    pub fn restore(&self, reply: &str, resolve: impl Fn(&str) -> Option<String>) -> String {
        let mut out = reply.to_owned();
        for (pseudonym, key) in &self.pseudonyms {
            if let Some(name) = resolve(key) {
                out = out.replace(pseudonym, &name);
            }
        }
        out
    }
}

/// Redact text: replace every known identity with a stable pseudonym, then
/// mask every secret span the pattern engine finds.
///
/// The pseudonym is language-neutral (`[Person 1]`) on purpose. A Chinese
/// placeholder in an English mail, or the reverse, steers the model's output
/// language and makes the redaction itself a hint.
#[must_use]
pub fn redact(text: &str, identities: &[Identity]) -> Redacted {
    let mut work = text.to_owned();
    let mut pseudonyms = BTreeMap::new();

    // Longest names first, so "Neo Sun <neo@example.com>" is replaced as a
    // whole before "Neo" would carve it up.
    let mut targets: Vec<(&Identity, &str)> = identities
        .iter()
        .flat_map(|id| id.names.iter().map(move |n| (id, n.as_str())))
        .filter(|(_, n)| !n.trim().is_empty())
        .collect();
    targets.sort_by_key(|(_, n)| std::cmp::Reverse(n.len()));

    let mut assigned: BTreeMap<&str, String> = BTreeMap::new();
    let mut next = 1usize;
    for (identity, name) in targets {
        if !contains_ignore_ascii_case(&work, name) {
            continue;
        }
        let pseudonym = assigned
            .entry(identity.key.as_str())
            .or_insert_with(|| {
                let p = format!("[Person {next}]");
                next += 1;
                p
            })
            .clone();
        work = replace_ignore_ascii_case(&work, name, &pseudonym);
        pseudonyms.insert(pseudonym, identity.key.clone());
    }

    // Mask secret spans last: replacing names can shift offsets, so the scan
    // has to run on the text as it will be sent.
    let mut out = String::with_capacity(work.len());
    let mut cursor = 0;
    for m in patterns::scan(&work) {
        out.push_str(&work[cursor..m.start]);
        out.push_str(m.kind.placeholder());
        cursor = m.end;
    }
    out.push_str(&work[cursor..]);

    Redacted {
        text: out,
        pseudonyms,
    }
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Case-insensitive replace that keeps the rest of the string intact. Works
/// on the lowercased copy only to find positions; slices come from the
/// original, so non-ASCII text is untouched.
fn replace_ignore_ascii_case(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_owned();
    }
    let lower_hay = haystack.to_lowercase();
    let lower_needle = needle.to_lowercase();
    // Lowercasing can change byte lengths for some scripts; fall back to an
    // exact replace when that happens rather than slicing at bad offsets.
    if lower_hay.len() != haystack.len() || lower_needle.len() != needle.len() {
        return haystack.replace(needle, replacement);
    }
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(found) = lower_hay[cursor..].find(&lower_needle) {
        let start = cursor + found;
        let end = start + lower_needle.len();
        if !haystack.is_char_boundary(start) || !haystack.is_char_boundary(end) {
            break;
        }
        out.push_str(&haystack[cursor..start]);
        out.push_str(replacement);
        cursor = end;
    }
    out.push_str(&haystack[cursor..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(key: &str, names: &[&str]) -> Identity {
        Identity {
            key: key.to_owned(),
            names: names.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn one_person_gets_one_pseudonym_everywhere() {
        let ids = [identity("p1", &["Neo Sun", "neo@example.com", "Neo"])];
        let r = redact(
            "Neo Sun asked about the contract. Reply to neo@example.com, or ping Neo.",
            &ids,
        );
        assert!(!r.text.contains("Neo"), "{}", r.text);
        assert!(!r.text.contains("example.com"), "{}", r.text);
        assert_eq!(r.text.matches("[Person 1]").count(), 3);
        assert_eq!(r.pseudonyms.len(), 1);
    }

    #[test]
    fn different_people_get_different_pseudonyms() {
        let ids = [identity("p1", &["Alice"]), identity("p2", &["Bob"])];
        let r = redact("Alice told Bob that Alice would be late.", &ids);
        assert!(!r.text.contains("Alice") && !r.text.contains("Bob"));
        assert_eq!(r.pseudonyms.len(), 2);
        let first = r.text.split_whitespace().next().unwrap();
        assert!(r.text.matches(first).count() >= 2, "{}", r.text);
    }

    #[test]
    fn secrets_are_masked_even_inside_personal_text() {
        let r = redact(
            "Alice, the verification code is 482913 and the card is 4111 1111 1111 1111.",
            &[identity("p1", &["Alice"])],
        );
        assert!(!r.text.contains("482913"), "{}", r.text);
        assert!(!r.text.contains("4111"), "{}", r.text);
        assert!(r.text.contains("[code]") && r.text.contains("[card number]"));
    }

    #[test]
    fn longer_names_win_so_an_address_is_not_carved_up() {
        let ids = [identity("p1", &["Neo", "neo@example.com"])];
        let r = redact("write to neo@example.com today", &ids);
        assert_eq!(r.text, "write to [Person 1] today");
    }

    #[test]
    fn matching_ignores_case_and_leaves_the_rest_alone() {
        let ids = [identity("p1", &["neo@example.com"])];
        let r = redact("Mail NEO@Example.COM about it", &ids);
        assert_eq!(r.text, "Mail [Person 1] about it");
    }

    #[test]
    fn chinese_names_are_replaced_without_slicing_errors() {
        let ids = [identity("p1", &["张律师", "zhang@example.com"])];
        let r = redact("张律师回复：对方同意调解，请发到 zhang@example.com。", &ids);
        assert!(!r.text.contains("张律师"), "{}", r.text);
        assert!(!r.text.contains("zhang@"), "{}", r.text);
        assert!(r.text.contains("对方同意调解"), "surrounding text survives");
    }

    #[test]
    fn a_reply_is_restored_on_this_machine() {
        let ids = [identity("p1", &["Alice"])];
        let r = redact("Draft a reply to Alice about Friday.", &ids);
        let model_reply = "Sure, I will tell [Person 1] that Friday works.";
        let restored = r.restore(model_reply, |key| (key == "p1").then(|| "Alice".to_owned()));
        assert_eq!(restored, "Sure, I will tell Alice that Friday works.");
    }

    #[test]
    fn a_person_not_mentioned_gets_no_pseudonym() {
        let ids = [identity("p1", &["Alice"]), identity("p2", &["Bob"])];
        let r = redact("Alice is coming.", &ids);
        assert_eq!(r.pseudonyms.len(), 1);
    }

    #[test]
    fn text_without_identities_still_has_its_secrets_masked() {
        let r = redact("password is hunter22", &[]);
        assert!(!r.text.contains("hunter22"));
        assert!(r.pseudonyms.is_empty());
    }
}
