//! The manifest: everything an agent may do, and nothing else (design 11).
//!
//! A capability exists only because it is declared here. The schema is
//! closed: an unknown field is an error, not something ignored, so a
//! manifest cannot carry a grant this version of the core does not
//! understand.

use genatrix_model::Level;
use serde::{Deserialize, Serialize};

use crate::HostError;

/// The largest quotas the core grants, whatever a manifest asks for.
pub mod ceiling {
    /// Space database, in megabytes.
    pub const SPACE_MB: u32 = 2048;
    /// Wall time of one run, in seconds.
    pub const SECONDS: u32 = 300;
    /// Linear memory of one run, in megabytes.
    pub const MEMORY_MB: u32 = 1024;
    /// Proposals in one day.
    pub const PROPOSALS_PER_DAY: u32 = 200;
}

/// A manifest, as read from a package's `genatrix:manifest` section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Shown to the user; not an identity.
    pub name: String,
    /// One sentence: what it is for.
    pub purpose: String,
    /// Who published it, as they say. A claim, shown as one.
    pub author: String,
    /// What it may read.
    pub reads: ReadScope,
    /// Whether it may have the text of attachments.
    #[serde(default)]
    pub blobs: BlobAccess,
    /// What it may ask a model to do.
    #[serde(default)]
    pub model: Vec<Purpose>,
    /// What it may propose.
    #[serde(default)]
    pub proposes: Vec<Proposal>,
    /// When it runs.
    #[serde(default)]
    pub triggers: Vec<Trigger>,
    /// Its limits.
    #[serde(default)]
    pub quota: Quota,
}

/// The read scope: a combination of fields, never a free query, so the
/// page can say it in words.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReadScope {
    /// `imap`, `telegram`. Empty means none.
    #[serde(default)]
    pub connectors: Vec<String>,
    /// `mail`, `message`, `event`. Empty means every kind.
    #[serde(default)]
    pub kinds: Vec<String>,
    /// Only items with an attachment.
    #[serde(default)]
    pub with_attachment: bool,
    /// Attachment types, when `with_attachment` is set. Empty means any.
    #[serde(default)]
    pub mime: Vec<String>,
    /// Words that must appear in the sender or subject. Empty means any.
    #[serde(default)]
    pub matching: Vec<String>,
    /// How far back, in days. None means all history.
    #[serde(default)]
    pub days: Option<u32>,
    /// The highest level it may read.
    #[serde(default)]
    pub max_level: Level,
}

/// What of an attachment the agent may have.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlobAccess {
    /// Nothing.
    #[default]
    None,
    /// The text the core extracts.
    Text,
}

/// Why a model may be called: the part of design 02's closed list an agent
/// may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// Put something in one of a few bins.
    Classify,
    /// Pull fields out of text.
    Extract,
    /// Make it shorter.
    Summarize,
    /// Write something for the user to send.
    Draft,
    /// Into another language.
    Translate,
}

/// One kind of effect the agent may propose.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    /// The agent's name for it, e.g. `record_entry`.
    pub kind: String,
    /// What the page calls it, e.g. "Record an entry".
    pub label: String,
    /// What it does when approved.
    #[serde(default)]
    pub effect: Effect,
    /// For an outward effect: whom it may reach.
    #[serde(default)]
    pub targets: Option<Targets>,
}

/// What an approved proposal does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Written to the agent's own space by its `apply`.
    #[default]
    Own,
    /// A mail, sent by the mail connector.
    SendMail,
    /// A chat message, sent by its connector.
    SendMessage,
    /// A calendar event.
    CreateEvent,
}

impl Effect {
    /// Whether it reaches anyone outside the device.
    #[must_use]
    pub fn is_outward(self) -> bool {
        !matches!(self, Self::Own)
    }
}

/// Whom an outward effect may reach (design 11, ruling 7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Targets {
    /// `same_thread` or `known_contacts`.
    Rule(TargetRule),
    /// A fixed list of addresses.
    Addresses(Vec<String>),
}

/// A target rule by name.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetRule {
    /// Only a reply in a thread the agent can read.
    SameThread,
    /// Only people the user has already exchanged messages with.
    KnownContacts,
}

/// When the agent is woken. The model never adds one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "on", rename_all = "snake_case", deny_unknown_fields)]
pub enum Trigger {
    /// New items in the read scope.
    Items,
    /// The user wrote in the agent's conversation.
    Message,
    /// A fixed schedule.
    Schedule {
        /// Passed to `on-schedule`.
        name: String,
        /// `daily`, `weekly:mon` … `weekly:sun`, or `monthly:1` … `monthly:28`.
        every: String,
        /// Local time, `HH:MM`.
        at: String,
    },
}

/// Limits. The core's ceilings apply over whatever is asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Quota {
    /// Space database, in megabytes.
    pub space_mb: u32,
    /// Wall time of one run, in seconds.
    pub seconds: u32,
    /// Linear memory of one run, in megabytes.
    pub memory_mb: u32,
    /// Proposals in one day.
    pub proposals_per_day: u32,
}

impl Default for Quota {
    fn default() -> Self {
        Self {
            space_mb: 50,
            seconds: 30,
            memory_mb: 256,
            proposals_per_day: 20,
        }
    }
}

impl Manifest {
    /// Parse and check a manifest.
    pub fn parse(text: &str) -> Result<Self, HostError> {
        let manifest: Self =
            toml::from_str(text).map_err(|e| HostError::Manifest(e.message().to_owned()))?;
        manifest.check()?;
        Ok(manifest)
    }

    /// The proposal of this kind, if declared.
    #[must_use]
    pub fn proposal(&self, kind: &str) -> Option<&Proposal> {
        self.proposes.iter().find(|p| p.kind == kind)
    }

    /// Whether this purpose is declared.
    #[must_use]
    pub fn may_call(&self, purpose: Purpose) -> bool {
        self.model.contains(&purpose)
    }

    fn check(&self) -> Result<(), HostError> {
        let bad = |why: String| Err(HostError::Manifest(why));
        if self.name.trim().is_empty() || self.name.chars().count() > 60 {
            return bad("name must be 1 to 60 characters".into());
        }
        if self.purpose.trim().is_empty() {
            return bad("purpose must say what it is for".into());
        }
        for c in &self.reads.connectors {
            if !matches!(c.as_str(), "imap" | "telegram") {
                return bad(format!("unknown connector `{c}`"));
            }
        }
        for k in &self.reads.kinds {
            if !matches!(k.as_str(), "mail" | "message" | "event") {
                return bad(format!("unknown item kind `{k}`"));
            }
        }
        if !self.reads.mime.is_empty() && !self.reads.with_attachment {
            return bad("`mime` needs `with_attachment = true`".into());
        }
        if self.blobs == BlobAccess::Text && self.reads.connectors.is_empty() {
            return bad("attachment text without anything to read".into());
        }
        let mut kinds = std::collections::HashSet::new();
        for p in &self.proposes {
            if p.kind.is_empty()
                || !p
                    .kind
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            {
                return bad(format!(
                    "kind `{}` must be lowercase letters, digits, `_`",
                    p.kind
                ));
            }
            if !kinds.insert(p.kind.as_str()) {
                return bad(format!("kind `{}` declared twice", p.kind));
            }
            match (p.effect.is_outward(), &p.targets) {
                (true, None) => {
                    return bad(format!(
                        "`{}` reaches outside and must name its targets",
                        p.kind
                    ));
                }
                (false, Some(_)) => {
                    return bad(format!(
                        "`{}` stays in the space and has no targets",
                        p.kind
                    ));
                }
                (true, Some(Targets::Addresses(a))) if a.is_empty() => {
                    return bad(format!("`{}` has an empty address list", p.kind));
                }
                _ => {}
            }
        }
        if self.triggers.is_empty() {
            return bad("at least one trigger".into());
        }
        let mut schedules = std::collections::HashSet::new();
        for t in &self.triggers {
            if let Trigger::Schedule { name, every, at } = t {
                if !schedules.insert(name.as_str()) {
                    return bad(format!("schedule `{name}` declared twice"));
                }
                check_every(every).map_err(HostError::Manifest)?;
                check_at(at).map_err(HostError::Manifest)?;
            }
        }
        let q = &self.quota;
        if q.space_mb == 0 || q.space_mb > ceiling::SPACE_MB {
            return bad(format!("space_mb must be 1 to {}", ceiling::SPACE_MB));
        }
        if q.seconds == 0 || q.seconds > ceiling::SECONDS {
            return bad(format!("seconds must be 1 to {}", ceiling::SECONDS));
        }
        if q.memory_mb < 16 || q.memory_mb > ceiling::MEMORY_MB {
            return bad(format!("memory_mb must be 16 to {}", ceiling::MEMORY_MB));
        }
        if q.proposals_per_day > ceiling::PROPOSALS_PER_DAY {
            return bad(format!(
                "proposals_per_day at most {}",
                ceiling::PROPOSALS_PER_DAY
            ));
        }
        Ok(())
    }
}

fn check_every(every: &str) -> Result<(), String> {
    const DAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];
    let ok = match every.split_once(':') {
        None => every == "daily",
        Some(("weekly", d)) => DAYS.contains(&d),
        Some(("monthly", d)) => d.parse::<u8>().is_ok_and(|d| (1..=28).contains(&d)),
        Some(_) => false,
    };
    if ok {
        Ok(())
    } else {
        Err(format!(
            "every `{every}`: daily, weekly:mon…sun, or monthly:1…28"
        ))
    }
}

fn check_at(at: &str) -> Result<(), String> {
    let ok = at.len() == 5
        && at.split_once(':').is_some_and(|(h, m)| {
            h.parse::<u8>().is_ok_and(|h| h < 24) && m.parse::<u8>().is_ok_and(|m| m < 60)
        });
    if ok {
        Ok(())
    } else {
        Err(format!("at `{at}`: HH:MM"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAX: &str = r#"
        name = "NZ tax"
        purpose = "Sort invoices and keep the books the way IRD asks"
        author = "Genatrix"
        blobs = "text"
        model = ["extract", "classify"]

        [reads]
        connectors = ["imap"]
        kinds = ["mail"]
        with_attachment = true
        mime = ["application/pdf"]
        max_level = "secret"

        [[proposes]]
        kind = "record_entry"
        label = "Record an entry"

        [[triggers]]
        on = "items"

        [[triggers]]
        on = "schedule"
        name = "gst_period"
        every = "monthly:1"
        at = "09:00"
    "#;

    #[test]
    fn a_full_manifest_reads() {
        let m = Manifest::parse(TAX).unwrap();
        assert_eq!(m.reads.max_level, Level::Secret);
        assert!(m.may_call(Purpose::Extract));
        assert!(!m.may_call(Purpose::Draft));
        assert_eq!(m.proposal("record_entry").unwrap().effect, Effect::Own);
        assert_eq!(m.quota, Quota::default());
    }

    #[test]
    fn unknown_fields_are_refused_not_ignored() {
        let extra = format!("network = true\n{TAX}");
        assert!(Manifest::parse(&extra).is_err());
        let nested = TAX.replace("[reads]", "[reads]\nhosts = [\"example.com\"]");
        assert!(Manifest::parse(&nested).is_err());
    }

    #[test]
    fn outward_effects_must_name_targets() {
        let send = format!(
            "{TAX}\n[[proposes]]\nkind = \"reply\"\nlabel = \"Reply\"\neffect = \"send_mail\"\n"
        );
        assert!(Manifest::parse(&send).is_err());
        let bounded = format!("{send}targets = \"same_thread\"\n");
        let m = Manifest::parse(&bounded).unwrap();
        assert_eq!(
            m.proposal("reply").unwrap().targets,
            Some(Targets::Rule(TargetRule::SameThread))
        );
        let listed = format!("{send}targets = [\"accountant@example.nz\"]\n");
        assert!(Manifest::parse(&listed).is_ok());
    }

    #[test]
    fn quotas_have_a_ceiling() {
        let greedy = format!("{TAX}\n[quota]\nseconds = 100000\n");
        assert!(Manifest::parse(&greedy).is_err());
    }

    #[test]
    fn schedules_are_checked() {
        for (every, at) in [
            ("hourly", "09:00"),
            ("monthly:31", "09:00"),
            ("daily", "25:00"),
        ] {
            let t = TAX
                .replace("monthly:1", every)
                .replace("\"09:00\"", &format!("\"{at}\""));
            assert!(Manifest::parse(&t).is_err(), "{every} {at}");
        }
    }
}
