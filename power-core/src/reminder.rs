//! Repeats "still no power" while it stays off.

use crate::power::{PowerEvent, PowerState};

/// A reminder that the power is still off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StillOff {
    /// Monotonic time the reminder was due.
    pub at_ms: u64,
    /// When the power went off, if the device saw it go.
    pub since_ms: Option<u64>,
}

/// Knows whether the power is off and when it was last said so.
#[derive(Debug, Default)]
pub struct Reminder {
    last: Option<StillOff>,
}

impl Reminder {
    /// Goes on from `state`, kept over a planned restart at `now_ms`.
    pub fn resume(state: Option<PowerState>, now_ms: u64) -> Self {
        let off = state == Some(PowerState::Absent);
        Self { last: off.then_some(StillOff { at_ms: now_ms, since_ms: None }) }
    }

    /// A reported change: power off starts the count, power back stops it.
    pub fn changed(&mut self, event: &PowerEvent) {
        let off = event.state == PowerState::Absent;
        self.last = off.then_some(StillOff { at_ms: event.at_ms, since_ms: event.began_ms() });
    }

    /// The reminder due at `now_ms`, `every_ms` after the last word on it.
    pub fn due(&mut self, now_ms: u64, every_ms: u64) -> Option<StillOff> {
        let last = self.last.as_mut()?;
        if now_ms.saturating_sub(last.at_ms) < every_ms {
            return None;
        }
        last.at_ms = now_ms;
        Some(*last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::power::Before;
    use PowerState::*;

    const MIN: u64 = 60_000;

    fn change(state: PowerState, at_ms: u64, before: Before) -> PowerEvent {
        PowerEvent { state, at_ms, before }
    }

    #[test]
    fn reminds_every_interval_while_the_power_is_off() {
        let mut r = Reminder::default();
        r.changed(&change(Absent, 10 * MIN, Before::Lasted(MIN)));
        assert_eq!(r.due(70 * MIN - 1, 60 * MIN), None);
        assert_eq!(r.due(70 * MIN, 60 * MIN), Some(StillOff { at_ms: 70 * MIN, since_ms: Some(10 * MIN) }));
        assert_eq!(r.due(129 * MIN, 60 * MIN), None);
        assert_eq!(r.due(130 * MIN, 60 * MIN), Some(StillOff { at_ms: 130 * MIN, since_ms: Some(10 * MIN) }));
    }

    #[test]
    fn power_back_stops_the_reminders() {
        let mut r = Reminder::default();
        r.changed(&change(Absent, 0, Before::Lasted(MIN)));
        r.changed(&change(Present, MIN, Before::Lasted(MIN)));
        assert_eq!(r.due(10 * 60 * MIN, 60 * MIN), None);
    }

    #[test]
    fn power_found_off_has_no_known_start() {
        let mut r = Reminder::default();
        assert_eq!(r.due(10 * 60 * MIN, 60 * MIN), None);
        r.changed(&change(Absent, 5_000, Before::StartUp));
        assert_eq!(r.due(5_000 + 60 * MIN, 60 * MIN), Some(StillOff { at_ms: 5_000 + 60 * MIN, since_ms: None }));
    }

    #[test]
    fn resumes_counting_after_a_planned_restart() {
        assert_eq!(Reminder::resume(Some(Present), 0).due(60 * MIN, 60 * MIN), None);
        assert_eq!(Reminder::resume(None, 0).due(60 * MIN, 60 * MIN), None);
        let mut r = Reminder::resume(Some(Absent), 1_000);
        assert_eq!(r.due(1_000 + 60 * MIN, 60 * MIN), Some(StillOff { at_ms: 1_000 + 60 * MIN, since_ms: None }));
    }
}
