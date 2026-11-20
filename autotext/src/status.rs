//! Live status message policy for chat updates.

/// Configuration for status message updates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusConfig {
    pub every_ms: u64,
    pub important: Vec<String>,
    pub keep_at_bottom: bool,
}

impl Default for StatusConfig {
    fn default() -> Self {
        Self {
            every_ms: 300_000,
            important: vec!["mains".to_string()],
            keep_at_bottom: true,
        }
    }
}

/// Snapshot and state of the current live status message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Live {
    pub message_id: i64,
    pub snapshot: Vec<Option<String>>,
    pub updated_ms: u64,
    pub at_bottom: bool,
}

/// Action to take on the next tick/cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Send,
    Edit(i64),
    Replace(i64),
    Wait,
}

/// Status message policy tracker.
#[derive(Debug, Clone, Default)]
pub struct Status {
    live: Option<Live>,
    wanted: bool,
    /// An important sensor changed since the last message, perhaps back
    /// again: the snapshot alone would miss an outage seen while offline.
    changed: bool,
}

impl Status {
    /// Creates a new `Status` tracker, optionally restoring previous `Live` state.
    pub fn new(live: Option<Live>) -> Self {
        Self {
            live,
            wanted: false,
            changed: false,
        }
    }

    /// Returns a reference to the active `Live` state, if any (for storage).
    pub fn live(&self) -> Option<&Live> {
        self.live.as_ref()
    }

    /// Decides the next step according to live status policy.
    pub fn decide(&self, config: &StatusConfig, now_ms: u64, values: &[(&str, Option<&str>)]) -> Step {
        // 1. no live message -> Send
        let live = match &self.live {
            Some(l) if !self.changed => l,
            _ => return Step::Send,
        };

        // 2. an important value differs from the snapshot -> Send
        if live.snapshot.len() != config.important.len() {
            return Step::Send;
        }
        let important_differs = config.important.iter().enumerate().any(|(i, key)| {
            let curr = values.iter().find(|(k, _)| *k == key).and_then(|(_, v)| *v);
            let snap = live.snapshot[i].as_deref();
            curr != snap
        });
        if important_differs {
            return Step::Send;
        }

        // 3. not due (now - updated_ms < every_ms and no want_update) -> Wait
        // Monotonic ms may restart from 0 after a reboot while Live came from storage:
        // treat updated_ms > now_ms as due (saturating).
        let is_due = self.wanted
            || (live.updated_ms > now_ms)
            || (now_ms - live.updated_ms >= config.every_ms);

        if !is_due {
            return Step::Wait;
        }

        // 4. due and !at_bottom and keep_at_bottom -> Replace(id)
        if !live.at_bottom && config.keep_at_bottom {
            Step::Replace(live.message_id)
        } else {
            // 5. due -> Edit(id)
            Step::Edit(live.message_id)
        }
    }

    /// An important sensor changed: the next status is a new message.
    pub fn important_changed(&mut self) {
        self.changed = true;
    }

    /// Asks for an immediate update (e.g. from Action::UpdateStatus).
    pub fn want_update(&mut self) {
        self.wanted = true;
    }

    /// Recorded when a new status message went out (after Send or Replace).
    pub fn sent(
        &mut self,
        message_id: i64,
        now_ms: u64,
        config: &StatusConfig,
        values: &[(&str, Option<&str>)],
    ) {
        let snapshot = config
            .important
            .iter()
            .map(|key| {
                values
                    .iter()
                    .find(|(k, _)| *k == key)
                    .and_then(|(_, v)| *v)
                    .map(ToString::to_string)
            })
            .collect();

        self.live = Some(Live {
            message_id,
            snapshot,
            updated_ms: now_ms,
            at_bottom: true,
        });
        self.wanted = false;
        self.changed = false;
    }

    /// Recorded when an edit went through (or Telegram reported "not modified").
    pub fn edited(&mut self, now_ms: u64) {
        if let Some(live) = &mut self.live {
            live.updated_ms = now_ms;
        }
        self.wanted = false;
    }

    /// Recorded when some other message went out in the same chat.
    pub fn other_sent(&mut self) {
        if let Some(live) = &mut self.live {
            live.at_bottom = false;
        }
    }

    /// Recorded when the message can no longer be edited or deleted.
    pub fn gone(&mut self) {
        self.live = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_to_send() {
        let config = StatusConfig::default();
        let status = Status::new(None);
        assert_eq!(status.live(), None);
        assert_eq!(
            status.decide(&config, 1000, &[("mains", Some("1"))]),
            Step::Send
        );
    }

    #[test]
    fn test_sent_wait_until_every_ms_then_edit() {
        let config = StatusConfig::default(); // every_ms: 300_000
        let mut status = Status::new(None);
        assert_eq!(
            status.decide(&config, 1000, &[("mains", Some("1"))]),
            Step::Send
        );

        status.sent(42, 1000, &config, &[("mains", Some("1"))]);
        assert!(status.live().is_some());
        assert_eq!(status.live().unwrap().message_id, 42);
        assert_eq!(status.live().unwrap().at_bottom, true);

        // Before interval expires -> Wait
        assert_eq!(
            status.decide(&config, 1000, &[("mains", Some("1"))]),
            Step::Wait
        );
        assert_eq!(
            status.decide(&config, 300_999, &[("mains", Some("1"))]),
            Step::Wait
        );

        // At interval expiration -> Edit
        assert_eq!(
            status.decide(&config, 301_000, &[("mains", Some("1"))]),
            Step::Edit(42)
        );

        // Edit performed
        status.edited(301_000);
        assert_eq!(
            status.decide(&config, 301_000, &[("mains", Some("1"))]),
            Step::Wait
        );
        assert_eq!(
            status.decide(&config, 600_999, &[("mains", Some("1"))]),
            Step::Wait
        );
        assert_eq!(
            status.decide(&config, 601_000, &[("mains", Some("1"))]),
            Step::Edit(42)
        );
    }

    #[test]
    fn test_mains_changes_sends_even_before_due_and_takes_new_snapshot() {
        let config = StatusConfig::default();
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);

        // Sensor value changes before due time
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("off"))]),
            Step::Send
        );

        // New status message sent
        status.sent(43, 2000, &config, &[("mains", Some("off"))]);
        assert_eq!(
            status.live().unwrap().snapshot,
            vec![Some("off".to_string())]
        );
        assert_eq!(status.live().unwrap().message_id, 43);

        // Now with "off", it should wait
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("off"))]),
            Step::Wait
        );
    }

    #[test]
    fn test_other_sent_then_due_replace_or_edit() {
        let mut config = StatusConfig::default(); // keep_at_bottom: true
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);

        status.other_sent();
        assert_eq!(status.live().unwrap().at_bottom, false);

        // Not due yet -> Wait
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Wait
        );

        // Due -> Replace because keep_at_bottom is true
        assert_eq!(
            status.decide(&config, 301_000, &[("mains", Some("on"))]),
            Step::Replace(42)
        );

        // When keep_at_bottom is false -> Edit instead
        config.keep_at_bottom = false;
        assert_eq!(
            status.decide(&config, 301_000, &[("mains", Some("on"))]),
            Step::Edit(42)
        );

        // When replaced, sent() is called for the replacement message
        config.keep_at_bottom = true;
        status.sent(44, 301_000, &config, &[("mains", Some("on"))]);
        assert_eq!(status.live().unwrap().at_bottom, true);
        assert_eq!(
            status.decide(&config, 301_000, &[("mains", Some("on"))]),
            Step::Wait
        );
    }

    #[test]
    fn test_want_update_due_at_once() {
        let config = StatusConfig::default();
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);

        // Not due yet
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Wait
        );

        status.want_update();
        // Immediately due for edit
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Edit(42)
        );

        // After edit, wanted is cleared
        status.edited(2000);
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Wait
        );

        // If not at bottom, want_update causes immediate Replace
        status.other_sent();
        status.want_update();
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Replace(42)
        );
    }

    #[test]
    fn test_gone_to_send() {
        let config = StatusConfig::default();
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);

        status.gone();
        assert_eq!(status.live(), None);
        assert_eq!(
            status.decide(&config, 2000, &[("mains", Some("on"))]),
            Step::Send
        );
    }

    #[test]
    fn a_change_and_back_while_offline_still_sends_a_new_message() {
        let config = StatusConfig::default();
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);
        // Off and on again before the device could say anything.
        status.important_changed();
        assert_eq!(status.decide(&config, 2000, &[("mains", Some("on"))]), Step::Send);
        status.sent(43, 3000, &config, &[("mains", Some("on"))]);
        assert_eq!(status.decide(&config, 4000, &[("mains", Some("on"))]), Step::Wait);
    }

    #[test]
    fn test_restored_live_with_updated_ms_in_future_due() {
        let config = StatusConfig::default();
        // Live state restored from storage with updated_ms from before reboot
        let live = Live {
            message_id: 101,
            snapshot: vec![Some("on".to_string())],
            updated_ms: 500_000,
            at_bottom: true,
        };
        let status = Status::new(Some(live));
        // updated_ms (500_000) > now_ms (100) -> due immediately
        assert_eq!(
            status.decide(&config, 100, &[("mains", Some("on"))]),
            Step::Edit(101)
        );
    }

    #[test]
    fn test_important_value_going_missing_sends() {
        let config = StatusConfig::default();
        let mut status = Status::new(None);
        status.sent(42, 1000, &config, &[("mains", Some("on"))]);

        // Missing value explicitly as None
        assert_eq!(
            status.decide(&config, 2000, &[("mains", None)]),
            Step::Send
        );

        // Missing value by absence from values list
        assert_eq!(status.decide(&config, 2000, &[]), Step::Send);

        // Message sent when value was missing
        status.sent(43, 2000, &config, &[("mains", None)]);
        assert_eq!(status.live().unwrap().snapshot, vec![None]);

        // Still missing -> Wait
        assert_eq!(status.decide(&config, 3000, &[]), Step::Wait);
        assert_eq!(
            status.decide(&config, 3000, &[("mains", None)]),
            Step::Wait
        );

        // Reappears -> Send
        assert_eq!(
            status.decide(&config, 3000, &[("mains", Some("on"))]),
            Step::Send
        );
    }

    #[test]
    fn test_multiple_important_values_order() {
        let config = StatusConfig {
            every_ms: 10_000,
            important: vec!["mains".to_string(), "temp".to_string()],
            keep_at_bottom: true,
        };
        let mut status = Status::new(None);
        let values = [("temp", Some("25")), ("mains", Some("1"))];

        status.sent(1, 0, &config, &values);
        assert_eq!(
            status.live().unwrap().snapshot,
            vec![Some("1".to_string()), Some("25".to_string())]
        );

        // Same values, different order in values slice -> still matches
        let values_reordered = [("mains", Some("1")), ("temp", Some("25"))];
        assert_eq!(status.decide(&config, 1000, &values_reordered), Step::Wait);

        // One changes -> Send
        let values_changed = [("mains", Some("1")), ("temp", Some("26"))];
        assert_eq!(
            status.decide(&config, 1000, &values_changed),
            Step::Send
        );
    }
}
