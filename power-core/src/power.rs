//! Mains presence detection from an unfiltered optocoupler.
//!
//! With 220 V present the optocoupler pulls the pin low on every mains half-wave
//! (50 Hz pulses); with no voltage the pin stays high. The firmware samples the
//! pin once per millisecond and hands each window's count of low samples here.

/// Whether mains voltage is on the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerState {
    Present,
    Absent,
}

impl PowerState {
    /// How the state is kept across a restart.
    pub fn to_stored(self) -> u8 {
        match self {
            Self::Present => 1,
            Self::Absent => 0,
        }
    }

    pub fn from_stored(stored: u8) -> Option<Self> {
        match stored {
            1 => Some(Self::Present),
            0 => Some(Self::Absent),
            _ => None,
        }
    }
}

/// Low samples a window needs to count as "pulses seen". A 100 ms window at
/// 50 Hz holds at least five pulses of several ms each, so 3 rejects single
/// spikes of noise while keeping a wide margin.
pub const MIN_LOW_SAMPLES: u32 = 3;

pub fn classify_window(low_samples: u32) -> PowerState {
    if low_samples >= MIN_LOW_SAMPLES {
        PowerState::Present
    } else {
        PowerState::Absent
    }
}

/// A confirmed state, emitted once at start-up and then on every change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerEvent {
    pub state: PowerState,
    /// Monotonic time of the first window that showed the new state.
    pub at_ms: u64,
    pub before: Before,
}

impl PowerEvent {
    /// When the new state began, if the device saw it begin.
    pub fn began_ms(&self) -> Option<u64> {
        (self.before != Before::StartUp).then_some(self.at_ms)
    }
}

/// What came before an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Before {
    /// Nothing: this is the state found at start-up.
    StartUp,
    /// A state whose beginning nobody saw: found at start-up, or after a
    /// planned restart.
    Unmeasured,
    /// A state that began and ended while the device ran, this long.
    Lasted(u64),
}

/// Turns noisy per-window readings into confirmed state changes.
pub struct Detector {
    confirm_windows: u32,
    /// The confirmed state and when it began (`None` if before start-up).
    stable: Option<(PowerState, Option<u64>)>,
    /// A run of windows that disagree with `stable`.
    streak: Option<Streak>,
}

#[derive(Clone, Copy)]
struct Streak {
    state: PowerState,
    started_ms: u64,
    windows: u32,
}

impl Detector {
    /// `confirm_windows` consecutive agreeing windows confirm a state.
    pub fn new(confirm_windows: u32) -> Self {
        Self { confirm_windows, stable: None, streak: None }
    }

    /// Goes on watching from `state`, seen before a planned restart: only a
    /// change is reported, and how long the state before it lasted is unknown.
    pub fn resume(confirm_windows: u32, state: PowerState) -> Self {
        Self { stable: Some((state, None)), ..Self::new(confirm_windows) }
    }

    pub fn observe(&mut self, window: PowerState, now_ms: u64) -> Option<PowerEvent> {
        if self.stable.map(|(state, _)| state) == Some(window) {
            self.streak = None;
            return None;
        }
        let streak = self.extend_streak(window, now_ms);
        if streak.windows < self.confirm_windows {
            return None;
        }
        let at_ms = streak.started_ms;
        let before = match self.stable {
            None => Before::StartUp,
            Some((_, None)) => Before::Unmeasured,
            Some((_, Some(since))) => Before::Lasted(at_ms - since),
        };
        let event = PowerEvent { state: window, at_ms, before };
        self.stable = Some((window, event.began_ms()));
        self.streak = None;
        Some(event)
    }

    fn extend_streak(&mut self, window: PowerState, now_ms: u64) -> Streak {
        let streak = self
            .streak
            .take()
            .filter(|s| s.state == window)
            .unwrap_or(Streak { state: window, started_ms: now_ms, windows: 0 });
        *self.streak.insert(Streak { windows: streak.windows + 1, ..streak })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use PowerState::*;

    /// Feeds `count` windows of `state`, 100 ms apart starting at `start_ms`,
    /// and returns every event emitted.
    fn feed(d: &mut Detector, state: PowerState, start_ms: u64, count: u64) -> Vec<PowerEvent> {
        (0..count)
            .filter_map(|i| d.observe(state, start_ms + i * 100))
            .collect()
    }

    #[test]
    fn window_with_enough_low_samples_means_present() {
        assert_eq!(classify_window(MIN_LOW_SAMPLES), Present);
        assert_eq!(classify_window(40), Present);
    }

    #[test]
    fn window_with_too_few_low_samples_means_absent() {
        assert_eq!(classify_window(0), Absent);
        assert_eq!(classify_window(MIN_LOW_SAMPLES - 1), Absent);
    }

    #[test]
    fn first_state_is_reported_once_confirmed() {
        let mut d = Detector::new(3);
        assert_eq!(feed(&mut d, Present, 0, 2), vec![]);
        assert_eq!(
            d.observe(Present, 200),
            Some(PowerEvent { state: Present, at_ms: 0, before: Before::StartUp })
        );
    }

    #[test]
    fn steady_state_is_reported_only_once() {
        let mut d = Detector::new(3);
        assert_eq!(feed(&mut d, Absent, 0, 50).len(), 1);
    }

    #[test]
    fn change_reports_how_long_the_previous_state_lasted() {
        let mut d = Detector::new(3);
        feed(&mut d, Present, 0, 10);
        feed(&mut d, Absent, 1_000, 3);
        let events = feed(&mut d, Present, 5_000, 3);
        assert_eq!(events, vec![PowerEvent { state: Present, at_ms: 5_000, before: Before::Lasted(4_000) }]);
    }

    #[test]
    fn state_found_at_start_up_has_no_known_length() {
        // Power may have been off long before the device was switched on.
        let mut d = Detector::new(3);
        feed(&mut d, Absent, 0, 10);
        let events = feed(&mut d, Present, 1_000, 3);
        assert_eq!(events, vec![PowerEvent { state: Present, at_ms: 1_000, before: Before::Unmeasured }]);
    }

    #[test]
    fn resumed_state_that_still_holds_is_not_reported_again() {
        // A planned restart is a pause in watching, not a new start.
        let mut d = Detector::resume(3, Present);
        assert_eq!(feed(&mut d, Present, 0, 50), vec![]);
    }

    #[test]
    fn change_during_a_planned_restart_is_reported_without_a_length() {
        let mut d = Detector::resume(3, Present);
        let events = feed(&mut d, Absent, 0, 3);
        assert_eq!(events, vec![PowerEvent { state: Absent, at_ms: 0, before: Before::Unmeasured }]);
    }

    #[test]
    fn state_survives_storage() {
        for state in [Present, Absent] {
            assert_eq!(PowerState::from_stored(state.to_stored()), Some(state));
        }
        assert_eq!(PowerState::from_stored(7), None);
    }

    #[test]
    fn short_blip_is_ignored() {
        let mut d = Detector::new(3);
        feed(&mut d, Present, 0, 5);
        assert_eq!(feed(&mut d, Absent, 500, 2), vec![]);
        assert_eq!(feed(&mut d, Present, 700, 5), vec![]);
    }

    #[test]
    fn interrupted_streak_starts_counting_again() {
        let mut d = Detector::new(3);
        feed(&mut d, Present, 0, 5);
        feed(&mut d, Absent, 500, 2);
        feed(&mut d, Present, 700, 1);
        assert_eq!(feed(&mut d, Absent, 800, 2), vec![]);
        assert_eq!(
            d.observe(Absent, 1_000),
            Some(PowerEvent { state: Absent, at_ms: 800, before: Before::Unmeasured })
        );
    }
}
