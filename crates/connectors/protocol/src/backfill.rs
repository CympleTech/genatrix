//! Importing history, newest first.
//!
//! Design: `docs/design/05-connectors.md`, "回填".
//!
//! Newest first is a product decision, not a technical one. Five minutes after
//! adding an account there should be something on the timeline worth looking
//! at, and that means this week's mail, not 2015's. It also means the user can
//! stop the backfill whenever they like and keep what matters most.

use serde::{Deserialize, Serialize};

/// How far an account's history has been read.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// Items taken so far.
    pub done: u64,
    /// What the source said there are, when it will say. An estimate: a
    /// number that moves is better than no number, as long as the interface
    /// does not present it as a promise.
    pub total: Option<u64>,
    /// How far back it has reached, for the "currently at March 2024" line.
    pub reached: Option<String>,
    /// Whether there is any more.
    pub complete: bool,
}

impl Progress {
    /// A fraction between 0 and 1, when there is enough to say.
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        if self.complete {
            return Some(1.0);
        }
        let total = self.total?;
        if total == 0 {
            return Some(1.0);
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "a mailbox large enough to lose precision here is larger than any that exists"
        )]
        Some((self.done as f32 / total as f32).min(1.0))
    }

    /// A line for the interface. Vague where the truth is vague.
    #[must_use]
    pub fn describe(&self) -> String {
        if self.complete {
            return format!("{} item(s), all of it", self.done);
        }
        let reached = self
            .reached
            .as_ref()
            .map_or_else(String::new, |r| format!(", back to {r}"));
        match self.total {
            Some(total) => format!("{} of about {total}{reached}", self.done),
            None => format!("{}{reached}", self.done),
        }
    }
}

/// A backfill in flight.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Backfill {
    /// Which account.
    pub account: String,
    /// Which subdivision: a folder, a chat, or the whole account.
    pub scope: String,
    /// How it is going.
    pub progress: Progress,
    /// Whether the user has paused it.
    pub paused: bool,
}

impl Backfill {
    /// Start one.
    pub fn new(account: impl Into<String>, scope: impl Into<String>) -> Self {
        Self {
            account: account.into(),
            scope: scope.into(),
            progress: Progress::default(),
            paused: false,
        }
    }

    /// Whether the connector should fetch more right now.
    #[must_use]
    pub const fn should_continue(&self) -> bool {
        !self.paused && !self.progress.complete
    }

    /// Record a batch.
    pub fn took(&mut self, count: u64, reached: Option<String>) {
        self.progress.done += count;
        if reached.is_some() {
            self.progress.reached = reached;
        }
    }

    /// There is no more history.
    pub const fn finished(&mut self) {
        self.progress.complete = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_backfill_wants_to_run() {
        let backfill = Backfill::new("me@example.com", "INBOX");
        assert!(backfill.should_continue());
        assert_eq!(backfill.progress.describe(), "0");
        assert_eq!(backfill.progress.fraction(), None);
    }

    #[test]
    fn progress_reads_as_a_sentence_and_says_how_far_back_it_got() {
        let mut backfill = Backfill::new("me@example.com", "INBOX");
        backfill.progress.total = Some(48_000);
        backfill.took(3_240, Some("March 2024".into()));
        assert_eq!(
            backfill.progress.describe(),
            "3240 of about 48000, back to March 2024"
        );
        assert!((backfill.progress.fraction().unwrap() - 0.0675).abs() < 0.001);
    }

    #[test]
    fn without_a_total_it_says_what_it_knows_and_no_more() {
        let mut backfill = Backfill::new("me", "chat:7");
        backfill.took(120, Some("last June".into()));
        assert_eq!(backfill.progress.describe(), "120, back to last June");
        assert_eq!(
            backfill.progress.fraction(),
            None,
            "no bar is better than a made-up bar"
        );
    }

    #[test]
    fn pausing_stops_it_and_finishing_stops_it() {
        let mut backfill = Backfill::new("me", "INBOX");
        backfill.paused = true;
        assert!(!backfill.should_continue());
        backfill.paused = false;
        assert!(backfill.should_continue());
        backfill.finished();
        assert!(!backfill.should_continue());
        assert_eq!(backfill.progress.fraction(), Some(1.0));
        assert_eq!(backfill.progress.describe(), "0 item(s), all of it");
    }

    #[test]
    fn a_total_that_turns_out_to_be_wrong_does_not_produce_more_than_a_whole() {
        let mut backfill = Backfill::new("me", "INBOX");
        backfill.progress.total = Some(100);
        backfill.took(150, None);
        assert_eq!(backfill.progress.fraction(), Some(1.0));
    }
}
