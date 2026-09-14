//! Annotation: everything derived from an item by a rule or a model.
//!
//! Annotations sit beside items, never inside them. They can be recomputed,
//! deleted, and coexist in several versions. A model change means new
//! annotations, not touched data.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{AnnotationId, ItemId, Level};

/// Who or what produced an annotation, precisely enough to reproduce it.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Producer {
    /// A model, identified with the prompt version used.
    Model {
        /// Model name from the registry (design 04).
        model: String,
        /// Version of the prompt or task definition.
        prompt_version: String,
    },
    /// A deterministic rule.
    Rule {
        /// Rule name.
        rule: String,
        /// Rule set version.
        version: String,
    },
    /// The user, by hand. Highest precedence.
    User,
}

/// The payload of an annotation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnnotationKind {
    /// A summary with the source set it was drawn from.
    Summary {
        /// Summary text.
        text: String,
        /// Items the summary cites. Empty means "no basis" and is shown as such.
        sources: Vec<ItemId>,
    },
    /// An embedding of one chunk of the item's text.
    Embedding {
        /// Chunk index within the item.
        chunk: u32,
        /// Byte range of the chunk in `Item::text`.
        range: (u32, u32),
        /// The vector.
        vector: Vec<f32>,
    },
    /// A sensitivity judgement. The item's `sensitivity` field caches the
    /// effective result of these.
    Sensitivity {
        /// The judged level.
        level: Level,
        /// Short reason, e.g. the rule name or the model's category.
        reason: String,
    },
    /// A free-form label.
    Label {
        /// Label text.
        label: String,
    },
    /// A suggestion for the user to act on, e.g. "merge these two persons".
    Suggestion {
        /// What is suggested.
        text: String,
    },
}

/// A machine-derived fact about an item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// Identifier.
    pub id: AnnotationId,
    /// The item it annotates.
    pub item_id: ItemId,
    /// Who produced it.
    pub producer: Producer,
    /// When.
    pub created_at: DateTime<Utc>,
    /// The content.
    pub kind: AnnotationKind,
    /// A newer annotation of the same kind that replaces this one.
    pub superseded_by: Option<AnnotationId>,
}

impl Annotation {
    /// Create an annotation now.
    #[must_use]
    pub fn new(item_id: ItemId, producer: Producer, kind: AnnotationKind) -> Self {
        Self {
            id: AnnotationId::new(),
            item_id,
            producer,
            created_at: Utc::now(),
            kind,
            superseded_by: None,
        }
    }
}

/// Resolve the effective sensitivity from all judgements about one item.
///
/// A user judgement wins outright, the most recent one if several. Otherwise
/// the result is the maximum of rule and model judgements: machines only
/// escalate. With no judgement at all the default applies.
#[must_use]
pub fn effective_level<'a, I>(judgements: I) -> Level
where
    I: IntoIterator<Item = &'a Annotation>,
{
    let mut user: Option<(DateTime<Utc>, Level)> = None;
    let mut machine = Level::Public;
    let mut any_machine = false;
    for a in judgements {
        let AnnotationKind::Sensitivity { level, .. } = &a.kind else {
            continue;
        };
        if a.superseded_by.is_some() {
            continue;
        }
        match &a.producer {
            Producer::User => {
                if user.is_none_or(|(t, _)| a.created_at >= t) {
                    user = Some((a.created_at, *level));
                }
            }
            Producer::Model { .. } | Producer::Rule { .. } => {
                any_machine = true;
                machine = machine.escalate(*level);
            }
        }
    }
    match (user, any_machine) {
        (Some((_, level)), _) => level,
        (None, true) => machine,
        (None, false) => Level::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn judge(producer: Producer, level: Level) -> Annotation {
        Annotation::new(
            ItemId::new(),
            producer,
            AnnotationKind::Sensitivity {
                level,
                reason: "test".into(),
            },
        )
    }

    fn rule(level: Level) -> Annotation {
        judge(
            Producer::Rule {
                rule: "r".into(),
                version: "1".into(),
            },
            level,
        )
    }

    fn model(level: Level) -> Annotation {
        judge(
            Producer::Model {
                model: "m".into(),
                prompt_version: "1".into(),
            },
            level,
        )
    }

    #[test]
    fn no_judgement_means_default() {
        assert_eq!(effective_level([]), Level::Personal);
    }

    #[test]
    fn machines_only_escalate() {
        assert_eq!(
            effective_level([&rule(Level::Personal), &model(Level::Public)]),
            Level::Personal
        );
        assert_eq!(
            effective_level([&rule(Level::Public), &model(Level::Secret)]),
            Level::Secret
        );
        assert_eq!(effective_level([&rule(Level::Public)]), Level::Public);
    }

    #[test]
    fn user_overrides_everything_including_lowering() {
        let user = judge(Producer::User, Level::Public);
        assert_eq!(
            effective_level([&rule(Level::Secret), &model(Level::Secret), &user]),
            Level::Public
        );
    }

    #[test]
    fn superseded_judgements_are_ignored() {
        let mut old = model(Level::Secret);
        old.superseded_by = Some(AnnotationId::new());
        assert_eq!(
            effective_level([&old, &model(Level::Public)]),
            Level::Public
        );
    }
}
