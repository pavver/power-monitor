//! Automation engine for small devices.
//!
//! Evaluates triggers (startup, sensor changes, steps, threshold crossings,
//! time-of-day, periodic intervals) combined with conditions to fire actions.
//! Memory advances independently of condition evaluations to prevent stale firings.

use std::fmt;

use crate::condition;
use crate::template::TemplateError;

/// Maximum number of automations supported in memory/configuration.
pub const MAX_AUTOMATIONS: usize = 16;

/// Time of day within a local day.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTime {
    /// Day counter incremented at local midnight.
    pub day: u32,
    /// Minutes since local midnight (0..1439).
    pub minute: u16,
}

/// Current time passed into the engine tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Now {
    /// Monotonic millisecond timestamp.
    pub ms: u64,
    /// Local time of day, or `None` if the wall clock is not synchronized yet.
    pub local: Option<LocalTime>,
}

/// Trigger condition for an automation.
#[derive(Debug, Clone, PartialEq)]
pub enum Trigger {
    /// Fires once on the very first tick.
    StartUp,
    /// Fires when the sensor value changes between two known values.
    Change { sensor: String },
    /// Fires when the sensor changes to `value` (numeric comparison if both parse as numbers, else exact text).
    Becomes { sensor: String, value: String },
    /// Fires when the numeric value moves by at least `step` (> 0) from the reference value.
    Step { sensor: String, step: f64 },
    /// Fires when the numeric value falls below `threshold`. Re-arms only above `threshold + hysteresis`.
    Below { sensor: String, threshold: f64, hysteresis: f64 },
    /// Fires when the numeric value rises above `threshold`. Re-arms only below `threshold - hysteresis`.
    Above { sensor: String, threshold: f64, hysteresis: f64 },
    /// Fires at `minute` (0..1439) of the local day once per day. Never before the clock is set.
    At { minute: u16 },
    /// Fires repeatedly every `seconds` while conditions hold.
    Every { seconds: u32 },
}

/// Action performed when an automation fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Send a new message with the rendered text.
    Send,
    /// Refresh the live status message.
    UpdateStatus,
}

/// Automation rule comprising trigger, conditions, action, and message template.
#[derive(Debug, Clone, PartialEq)]
pub struct Automation {
    pub trigger: Trigger,
    pub conditions: Vec<String>,
    pub action: Action,
    pub text: String,
}

/// Validation error for an automation rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutomationError {
    /// Sensor referenced in trigger is unknown.
    UnknownSensor(String),
    /// Step must be greater than zero and finite.
    InvalidStep,
    /// Hysteresis must be non-negative and finite.
    InvalidHysteresis,
    /// Minute must be strictly less than 1440.
    InvalidMinute(u16),
    /// Interval must be greater than zero seconds.
    ZeroInterval,
    /// Condition syntax is invalid.
    BadCondition {
        condition: String,
        error: TemplateError,
    },
    /// A sensor referenced in a condition is unknown.
    UnknownConditionSensor {
        condition: String,
        sensor: String,
    },
}

impl fmt::Display for AutomationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSensor(s) => write!(f, "невідомий сенсор: {s}"),
            Self::InvalidStep => f.write_str("крок має бути більшим за 0"),
            Self::InvalidHysteresis => f.write_str("гістерезис не може бути від'ємним"),
            Self::InvalidMinute(m) => write!(f, "хвилина має бути від 0 до 1439 ({m})"),
            Self::ZeroInterval => f.write_str("інтервал не може бути нульовим"),
            Self::BadCondition { condition, error } => {
                write!(f, "помилка в умові '{condition}': {error}")
            }
            Self::UnknownConditionSensor { condition, sensor } => {
                write!(f, "невідома змінна '{sensor}' в умові '{condition}'")
            }
        }
    }
}

impl std::error::Error for AutomationError {}

impl Trigger {
    /// The sensor the trigger watches, if it watches one.
    fn sensor(&self) -> Option<&str> {
        match self {
            Trigger::Change { sensor }
            | Trigger::Becomes { sensor, .. }
            | Trigger::Step { sensor, .. }
            | Trigger::Below { sensor, .. }
            | Trigger::Above { sensor, .. } => Some(sensor),
            Trigger::StartUp | Trigger::At { .. } | Trigger::Every { .. } => None,
        }
    }

    /// What is wrong with the trigger's own numbers.
    fn fault(&self) -> Option<AutomationError> {
        match *self {
            Trigger::Step { step, .. } if !step.is_finite() || step <= 0.0 => Some(AutomationError::InvalidStep),
            Trigger::Below { hysteresis, .. } | Trigger::Above { hysteresis, .. }
                if !hysteresis.is_finite() || hysteresis < 0.0 =>
            {
                Some(AutomationError::InvalidHysteresis)
            }
            Trigger::At { minute } if minute >= 1440 => Some(AutomationError::InvalidMinute(minute)),
            Trigger::Every { seconds: 0 } => Some(AutomationError::ZeroInterval),
            _ => None,
        }
    }
}

/// Validates an automation before saving: the trigger's numbers are in
/// range and every sensor and condition names a value `known_value` knows.
///
/// The text is validated separately by the caller with [`crate::template::validate`].
pub fn validate(a: &Automation, known_value: &dyn Fn(&str) -> bool) -> Result<(), AutomationError> {
    if let Some(fault) = a.trigger.fault() {
        return Err(fault);
    }
    if let Some(sensor) = a.trigger.sensor().filter(|sensor| !known_value(sensor)) {
        return Err(AutomationError::UnknownSensor(sensor.to_string()));
    }
    for text in &a.conditions {
        let bad = |error| AutomationError::BadCondition { condition: text.clone(), error };
        let name = condition::parse(text).map_err(bad)?.name;
        if !known_value(name) {
            return Err(AutomationError::UnknownConditionSensor { condition: text.clone(), sensor: name.to_string() });
        }
    }
    Ok(())
}

type Values<'a, 'v> = &'a [(&'a str, Option<&'v str>)];

/// What one automation remembers between ticks; each trigger uses its part.
#[derive(Debug, Clone, Default)]
struct Memory {
    /// StartUp: it has fired.
    started: bool,
    /// Change, Becomes: the last known value.
    last: Option<String>,
    /// Step: the value at the last firing (or the first known one).
    reference: Option<f64>,
    /// Below, Above: whether crossing the threshold fires.
    armed: Option<bool>,
    /// At: the local day it fired, or was already past, on.
    day: Option<u32>,
    /// Every: when it fires next, while its conditions hold.
    due_ms: Option<u64>,
}

impl Memory {
    /// Whether `trigger` fires now; `holds` says whether the conditions do.
    fn fires(&mut self, trigger: &Trigger, now: Now, values: Values, holds: bool) -> bool {
        let number = |sensor: &str| value(values, sensor).and_then(condition::parse_finite);
        match trigger {
            Trigger::StartUp => !std::mem::replace(&mut self.started, true),
            Trigger::Change { sensor } => self.changed(value(values, sensor)).is_some(),
            Trigger::Becomes { sensor, value: to } => self
                .changed(value(values, sensor))
                .is_some_and(|(before, now)| condition::eq_values(now, to) && !condition::eq_values(&before, to)),
            Trigger::Step { sensor, step } => self.stepped(number(sensor), *step),
            Trigger::Below { sensor, threshold, hysteresis } => self.fell(number(sensor), *threshold, *hysteresis),
            // Rising over a threshold is falling under it, upside down.
            Trigger::Above { sensor, threshold, hysteresis } => self.fell(number(sensor).map(|v| -v), -threshold, *hysteresis),
            Trigger::At { minute } => self.at(now.local, *minute),
            Trigger::Every { seconds } => self.every(now.ms, u64::from(*seconds) * 1000, holds),
        }
    }

    /// The value before and now, when a known value differs from the last
    /// known one; missing values are skipped and the first one is a baseline.
    fn changed<'v>(&mut self, now: Option<&'v str>) -> Option<(String, &'v str)> {
        let now = now?;
        if self.last.as_deref().is_some_and(|before| condition::eq_values(before, now)) {
            return None;
        }
        Some((self.last.replace(now.to_string())?, now))
    }

    fn stepped(&mut self, now: Option<f64>, step: f64) -> bool {
        let Some(now) = now else { return false };
        let moved = self.reference.is_some_and(|reference| (now - reference).abs() >= step);
        if moved || self.reference.is_none() {
            self.reference = Some(now);
        }
        moved
    }

    /// Fell under `threshold`; armed again only over `threshold + hysteresis`,
    /// and not armed at all when the first value is already under it.
    fn fell(&mut self, now: Option<f64>, threshold: f64, hysteresis: f64) -> bool {
        let Some(now) = now else { return false };
        match self.armed {
            None => self.armed = Some(now >= threshold),
            Some(true) if now < threshold => {
                self.armed = Some(false);
                return true;
            }
            Some(false) if now > threshold + hysteresis => self.armed = Some(true),
            Some(_) => {}
        }
        false
    }

    fn at(&mut self, local: Option<LocalTime>, minute: u16) -> bool {
        let Some(local) = local else { return false };
        // Set already past the minute: today's is missed, not late.
        if self.day.is_none() && local.minute > minute {
            self.day = Some(local.day);
        }
        let fires = local.minute == minute && self.day != Some(local.day);
        if fires {
            self.day = Some(local.day);
        }
        fires
    }

    /// Counts from the tick the conditions began to hold; stops when they don't.
    fn every(&mut self, now_ms: u64, interval_ms: u64, holds: bool) -> bool {
        if !holds {
            self.due_ms = None;
            return false;
        }
        let next = Some(now_ms.saturating_add(interval_ms));
        match self.due_ms {
            None => {
                self.due_ms = next;
                false
            }
            Some(due) if now_ms >= due => {
                self.due_ms = next;
                true
            }
            Some(_) => false,
        }
    }
}

/// Tracks the automations' memory across ticks.
#[derive(Debug, Clone, Default)]
pub struct Engine {
    memory: Vec<Memory>,
}

impl Engine {
    /// An engine for `count` automations. When the user edits the list the
    /// caller makes a new one; `tick` follows a changed length without panicking.
    pub fn new(count: usize) -> Self {
        Self { memory: vec![Memory::default(); count] }
    }

    /// Indices of the automations that fire now, in order: the trigger fires
    /// and every condition holds. A trigger's memory advances either way, so
    /// a change while the conditions failed is not reported later as stale.
    pub fn tick(&mut self, automations: &[Automation], now: Now, values: Values) -> Vec<usize> {
        self.memory.resize_with(automations.len(), Memory::default);
        let memory = self.memory.iter_mut();
        let fired = memory.zip(automations).map(|(memory, a)| {
            let holds = conditions_hold(&a.conditions, values);
            memory.fires(&a.trigger, now, values, holds) && holds
        });
        fired.enumerate().filter_map(|(index, fired)| fired.then_some(index)).collect()
    }
}

fn value<'v>(values: Values<'_, 'v>, name: &str) -> Option<&'v str> {
    values.iter().find(|(key, _)| *key == name).and_then(|(_, value)| *value)
}

/// A condition that does not parse never holds (they are checked on saving).
fn conditions_hold(conditions: &[String], values: Values) -> bool {
    conditions.iter().all(|text| condition::parse(text).is_ok_and(|c| c.holds(value(values, c.name))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_now(ms: u64) -> Now {
        Now { ms, local: None }
    }

    fn timed_now(ms: u64, day: u32, minute: u16) -> Now {
        Now {
            ms,
            local: Some(LocalTime { day, minute }),
        }
    }

    #[test]
    fn baseline_is_not_a_change() {
        let autos = [Automation {
            trigger: Trigger::Change {
                sensor: "temp".into(),
            },
            conditions: vec![],
            action: Action::Send,
            text: "changed".into(),
        }];
        let mut engine = Engine::new(1);

        // First known value is baseline, not a change.
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("temp", Some("20.0"))]),
            Vec::<usize>::new()
        );
        // Same value: no change.
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("temp", Some("20.0"))]),
            Vec::<usize>::new()
        );
        // Changed value: fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("temp", Some("21.0"))]),
            vec![0]
        );
    }

    #[test]
    fn change_ignores_missing() {
        let autos = [Automation {
            trigger: Trigger::Change {
                sensor: "temp".into(),
            },
            conditions: vec![],
            action: Action::Send,
            text: "changed".into(),
        }];
        let mut engine = Engine::new(1);

        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("temp", Some("20.0"))]),
            Vec::<usize>::new()
        );
        // Sensor disappears: not a change.
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("temp", None)]),
            Vec::<usize>::new()
        );
        // Sensor returns with same value: not a change.
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("temp", Some("20.0"))]),
            Vec::<usize>::new()
        );
        // Sensor disappears again: not a change.
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("temp", None)]),
            Vec::<usize>::new()
        );
        // Sensor returns with new value: changed!
        assert_eq!(
            engine.tick(&autos, dummy_now(500), &[("temp", Some("25.0"))]),
            vec![0]
        );
    }

    #[test]
    fn becomes_fires_only_on_transition() {
        let autos = [Automation {
            trigger: Trigger::Becomes {
                sensor: "mains".into(),
                value: "on".into(),
            },
            conditions: vec![],
            action: Action::Send,
            text: "mains on".into(),
        }];
        let mut engine = Engine::new(1);

        // Baseline "on" at start: does not fire!
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("mains", Some("on"))]),
            Vec::<usize>::new()
        );
        // Changes to "off": does not fire.
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("mains", Some("off"))]),
            Vec::<usize>::new()
        );
        // Transitions to "on": fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("mains", Some("on"))]),
            vec![0]
        );
        // Stays "on": does not fire.
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("mains", Some("on"))]),
            Vec::<usize>::new()
        );
        // Missing: does not fire.
        assert_eq!(
            engine.tick(&autos, dummy_now(500), &[("mains", None)]),
            Vec::<usize>::new()
        );
        // Comes back as "on": was already "on", so not a transition to "on".
        assert_eq!(
            engine.tick(&autos, dummy_now(600), &[("mains", Some("on"))]),
            Vec::<usize>::new()
        );
        // Goes to "off": does not fire.
        assert_eq!(
            engine.tick(&autos, dummy_now(700), &[("mains", Some("off"))]),
            Vec::<usize>::new()
        );
        // Transitions to "on": fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(800), &[("mains", Some("on"))]),
            vec![0]
        );
    }

    #[test]
    fn becomes_numeric_equality() {
        let autos = [Automation {
            trigger: Trigger::Becomes {
                sensor: "soc".into(),
                value: "100".into(),
            },
            conditions: vec![],
            action: Action::Send,
            text: "full".into(),
        }];
        let mut engine = Engine::new(1);

        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("soc", Some("99.0"))]),
            Vec::<usize>::new()
        );
        // 100.0 numerically equals "100"
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("soc", Some("100.0"))]),
            vec![0]
        );
        // 100.0 to 100 is not a transition
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("soc", Some("100"))]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn step_trigger_and_non_numbers_ignored() {
        let autos = [Automation {
            trigger: Trigger::Step {
                sensor: "soc".into(),
                step: 10.0,
            },
            conditions: vec![],
            action: Action::Send,
            text: "step".into(),
        }];
        let mut engine = Engine::new(1);

        // 47: first known value (baseline reference = 47)
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("soc", Some("47"))]),
            Vec::<usize>::new()
        );
        // 47 -> 52: diff 5 < 10: no
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("soc", Some("52"))]),
            Vec::<usize>::new()
        );
        // Non-number ignored
        assert_eq!(
            engine.tick(&autos, dummy_now(250), &[("soc", Some("invalid"))]),
            Vec::<usize>::new()
        );
        // 52 -> 57: diff |57 - 47| = 10 >= 10: yes! (ref becomes 57)
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("soc", Some("57"))]),
            vec![0]
        );
        // 57 -> 55: diff |55 - 57| = 2 < 10: no
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("soc", Some("55"))]),
            Vec::<usize>::new()
        );
        // 55 -> 47: diff |47 - 57| = 10 >= 10: yes! (ref becomes 47)
        assert_eq!(
            engine.tick(&autos, dummy_now(500), &[("soc", Some("47"))]),
            vec![0]
        );
    }

    #[test]
    fn below_with_hysteresis_does_not_chatter() {
        let autos = [Automation {
            trigger: Trigger::Below {
                sensor: "temp".into(),
                threshold: 20.0,
                hysteresis: 1.0,
            },
            conditions: vec![],
            action: Action::Send,
            text: "cold".into(),
        }];
        let mut engine = Engine::new(1);

        // Safe side >= 20.0 arms it
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("temp", Some("25.0"))]),
            Vec::<usize>::new()
        );
        // 19.9 falls below 20.0: fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("temp", Some("19.9"))]),
            vec![0]
        );
        // 20.1 is not > 20 + 1: does not re-arm, no firing
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("temp", Some("20.1"))]),
            Vec::<usize>::new()
        );
        // 19.9 still disarmed: no chatter!
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("temp", Some("19.9"))]),
            Vec::<usize>::new()
        );
        // 21.5 rises above threshold + hysteresis (21.0): re-arms!
        assert_eq!(
            engine.tick(&autos, dummy_now(500), &[("temp", Some("21.5"))]),
            Vec::<usize>::new()
        );
        // 19.9 falls below threshold: fires again!
        assert_eq!(
            engine.tick(&autos, dummy_now(600), &[("temp", Some("19.9"))]),
            vec![0]
        );
    }

    #[test]
    fn below_already_below_at_start_does_not_fire() {
        let autos = [Automation {
            trigger: Trigger::Below {
                sensor: "temp".into(),
                threshold: 20.0,
                hysteresis: 1.0,
            },
            conditions: vec![],
            action: Action::Send,
            text: "cold".into(),
        }];
        let mut engine = Engine::new(1);

        // Starts below 20.0: disarmed initial state, does not fire!
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("temp", Some("19.9"))]),
            Vec::<usize>::new()
        );
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("temp", Some("19.5"))]),
            Vec::<usize>::new()
        );
        // Rises above 21.0 to re-arm
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("temp", Some("21.2"))]),
            Vec::<usize>::new()
        );
        // Now falling below fires
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("temp", Some("19.8"))]),
            vec![0]
        );
    }

    #[test]
    fn above_with_hysteresis_and_mirror_behavior() {
        let autos = [Automation {
            trigger: Trigger::Above {
                sensor: "temp".into(),
                threshold: 30.0,
                hysteresis: 2.0,
            },
            conditions: vec![],
            action: Action::Send,
            text: "hot".into(),
        }];
        let mut engine = Engine::new(1);

        // Starts already above threshold: does not fire
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("temp", Some("35.0"))]),
            Vec::<usize>::new()
        );
        // Drops below threshold - hysteresis (< 28.0): re-arms
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("temp", Some("27.5"))]),
            Vec::<usize>::new()
        );
        // Rises above 30.0: fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(300), &[("temp", Some("30.5"))]),
            vec![0]
        );
        // Drops to 29.0: not below 28.0, so does not re-arm
        assert_eq!(
            engine.tick(&autos, dummy_now(400), &[("temp", Some("29.0"))]),
            Vec::<usize>::new()
        );
        // Rises to 30.5 again: still disarmed
        assert_eq!(
            engine.tick(&autos, dummy_now(500), &[("temp", Some("30.5"))]),
            Vec::<usize>::new()
        );
        // Drops below 28.0: re-arms
        assert_eq!(
            engine.tick(&autos, dummy_now(600), &[("temp", Some("27.0"))]),
            Vec::<usize>::new()
        );
        // Rises above 30.0: fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(700), &[("temp", Some("30.1"))]),
            vec![0]
        );
    }

    #[test]
    fn at_fires_once_that_day() {
        let autos = [Automation {
            trigger: Trigger::At { minute: 600 },
            conditions: vec![],
            action: Action::Send,
            text: "at 10am".into(),
        }];
        let mut engine = Engine::new(1);

        // Clock not set: never fires
        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[]),
            Vec::<usize>::new()
        );

        // Clock set at minute 599 on day 1
        assert_eq!(
            engine.tick(&autos, timed_now(200, 1, 599), &[]),
            Vec::<usize>::new()
        );
        // Minute 600 arrives on day 1: fires!
        assert_eq!(
            engine.tick(&autos, timed_now(300, 1, 600), &[]),
            vec![0]
        );
        // Repeated tick at minute 600 on day 1: does not fire again
        assert_eq!(
            engine.tick(&autos, timed_now(400, 1, 600), &[]),
            Vec::<usize>::new()
        );
        // Later that day: no firing
        assert_eq!(
            engine.tick(&autos, timed_now(500, 1, 601), &[]),
            Vec::<usize>::new()
        );

        // Next day at minute 600: fires again!
        assert_eq!(
            engine.tick(&autos, timed_now(600, 2, 600), &[]),
            vec![0]
        );
        // Repeated tick: does not fire
        assert_eq!(
            engine.tick(&autos, timed_now(700, 2, 600), &[]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn at_booting_after_minute_does_not_fire_today() {
        let autos = [Automation {
            trigger: Trigger::At { minute: 600 },
            conditions: vec![],
            action: Action::Send,
            text: "at 10am".into(),
        }];
        let mut engine = Engine::new(1);

        // Boot at minute 700 (> 600) on day 1: does NOT fire today
        assert_eq!(
            engine.tick(&autos, timed_now(100, 1, 700), &[]),
            Vec::<usize>::new()
        );

        // Next day minute 600: fires!
        assert_eq!(
            engine.tick(&autos, timed_now(200, 2, 600), &[]),
            vec![0]
        );
    }

    #[test]
    fn every_fires_after_n_seconds_and_resets() {
        let autos = [Automation {
            trigger: Trigger::Every { seconds: 10 },
            conditions: vec!["run=yes".into()],
            action: Action::Send,
            text: "periodic".into(),
        }];
        let mut engine = Engine::new(1);

        // Condition does not hold: no firing, timer does not start
        assert_eq!(
            engine.tick(&autos, dummy_now(0), &[("run", Some("no"))]),
            Vec::<usize>::new()
        );
        // Condition becomes true at ms = 1000: timer starts (due at 11000)
        assert_eq!(
            engine.tick(&autos, dummy_now(1000), &[("run", Some("yes"))]),
            Vec::<usize>::new()
        );
        // 5 seconds later: not yet
        assert_eq!(
            engine.tick(&autos, dummy_now(6000), &[("run", Some("yes"))]),
            Vec::<usize>::new()
        );
        // 10 seconds after start (ms = 11000): fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(11000), &[("run", Some("yes"))]),
            vec![0]
        );
        // Another 10 seconds (ms = 21000): fires again!
        assert_eq!(
            engine.tick(&autos, dummy_now(21000), &[("run", Some("yes"))]),
            vec![0]
        );
        // Condition stops holding at ms = 25000: count resets!
        assert_eq!(
            engine.tick(&autos, dummy_now(25000), &[("run", Some("no"))]),
            Vec::<usize>::new()
        );
        // Condition holds again at ms = 27000: new timer starts (due at 37000)
        assert_eq!(
            engine.tick(&autos, dummy_now(27000), &[("run", Some("yes"))]),
            Vec::<usize>::new()
        );
        // At ms = 31000: not yet
        assert_eq!(
            engine.tick(&autos, dummy_now(31000), &[("run", Some("yes"))]),
            Vec::<usize>::new()
        );
        // At ms = 37000: fires!
        assert_eq!(
            engine.tick(&autos, dummy_now(37000), &[("run", Some("yes"))]),
            vec![0]
        );
    }

    #[test]
    fn every_without_conditions() {
        let autos = [Automation {
            trigger: Trigger::Every { seconds: 5 },
            conditions: vec![],
            action: Action::Send,
            text: "ping".into(),
        }];
        let mut engine = Engine::new(1);

        // Starts timer at ms = 0, first firing due at 5000
        assert_eq!(engine.tick(&autos, dummy_now(0), &[]), Vec::<usize>::new());
        assert_eq!(
            engine.tick(&autos, dummy_now(5000), &[]),
            vec![0]
        );
        assert_eq!(
            engine.tick(&autos, dummy_now(10000), &[]),
            vec![0]
        );
    }

    #[test]
    fn startup_only_on_first_tick() {
        // Condition holds on first tick
        let autos = [Automation {
            trigger: Trigger::StartUp,
            conditions: vec!["enabled=yes".into()],
            action: Action::Send,
            text: "started".into(),
        }];
        let mut engine = Engine::new(1);

        assert_eq!(
            engine.tick(&autos, dummy_now(100), &[("enabled", Some("yes"))]),
            vec![0]
        );
        // Second tick: never fires
        assert_eq!(
            engine.tick(&autos, dummy_now(200), &[("enabled", Some("yes"))]),
            Vec::<usize>::new()
        );

        // Condition does not hold on first tick
        let mut engine2 = Engine::new(1);
        assert_eq!(
            engine2.tick(&autos, dummy_now(100), &[("enabled", Some("no"))]),
            Vec::<usize>::new()
        );
        // Second tick condition becomes true: still does NOT fire!
        assert_eq!(
            engine2.tick(&autos, dummy_now(200), &[("enabled", Some("yes"))]),
            Vec::<usize>::new()
        );
    }

    #[test]
    fn conditions_gate_firing_but_memory_advances() {
        let autos = [Automation {
            trigger: Trigger::Change {
                sensor: "temp".into(),
            },
            conditions: vec!["enabled=yes".into()],
            action: Action::Send,
            text: "temp changed".into(),
        }];
        let mut engine = Engine::new(1);

        // Baseline temp = 20
        assert_eq!(
            engine.tick(
                &autos,
                dummy_now(100),
                &[("temp", Some("20")), ("enabled", Some("no"))]
            ),
            Vec::<usize>::new()
        );

        // Temp changes 20 -> 25 while condition is false: trigger memory advances to 25, but no firing
        assert_eq!(
            engine.tick(
                &autos,
                dummy_now(200),
                &[("temp", Some("25")), ("enabled", Some("no"))]
            ),
            Vec::<usize>::new()
        );

        // Condition becomes true, but temp did not change (still 25): no stale firing!
        assert_eq!(
            engine.tick(
                &autos,
                dummy_now(300),
                &[("temp", Some("25")), ("enabled", Some("yes"))]
            ),
            Vec::<usize>::new()
        );

        // Temp changes 25 -> 30 while condition is true: fires!
        assert_eq!(
            engine.tick(
                &autos,
                dummy_now(400),
                &[("temp", Some("30")), ("enabled", Some("yes"))]
            ),
            vec![0]
        );
    }

    #[test]
    fn engine_resizes_robustly() {
        let mut engine = Engine::new(1);
        let autos = [
            Automation {
                trigger: Trigger::StartUp,
                conditions: vec![],
                action: Action::Send,
                text: "a".into(),
            },
            Automation {
                trigger: Trigger::StartUp,
                conditions: vec![],
                action: Action::Send,
                text: "b".into(),
            },
        ];
        // Sized for 1, passed 2: resizes without panicking
        let fired = engine.tick(&autos, dummy_now(100), &[]);
        assert_eq!(fired, vec![0, 1]);
    }

    #[test]
    fn validation_errors() {
        let known = |s: &str| s == "temp" || s == "mains";

        // Unknown sensor in trigger
        let a = Automation {
            trigger: Trigger::Change {
                sensor: "unknown_sensor".into(),
            },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(
            validate(&a, &known),
            Err(AutomationError::UnknownSensor("unknown_sensor".into()))
        );

        // Step <= 0 or not finite
        let a = Automation {
            trigger: Trigger::Step {
                sensor: "temp".into(),
                step: 0.0,
            },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(validate(&a, &known), Err(AutomationError::InvalidStep));

        let a = Automation {
            trigger: Trigger::Step {
                sensor: "temp".into(),
                step: f64::NAN,
            },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(validate(&a, &known), Err(AutomationError::InvalidStep));

        // Hysteresis < 0 or not finite
        let a = Automation {
            trigger: Trigger::Below {
                sensor: "temp".into(),
                threshold: 20.0,
                hysteresis: -0.1,
            },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(validate(&a, &known), Err(AutomationError::InvalidHysteresis));

        // Minute >= 1440
        let a = Automation {
            trigger: Trigger::At { minute: 1440 },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(validate(&a, &known), Err(AutomationError::InvalidMinute(1440)));

        // Seconds == 0
        let a = Automation {
            trigger: Trigger::Every { seconds: 0 },
            conditions: vec![],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(validate(&a, &known), Err(AutomationError::ZeroInterval));

        // Bad condition (syntax error)
        let a = Automation {
            trigger: Trigger::StartUp,
            conditions: vec!["temp<nan".into()],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(
            validate(&a, &known),
            Err(AutomationError::BadCondition {
                condition: "temp<nan".into(),
                error: TemplateError::NotANumber,
            })
        );

        // Unknown name in condition
        let a = Automation {
            trigger: Trigger::StartUp,
            conditions: vec!["unknown_var=1".into()],
            action: Action::Send,
            text: "t".into(),
        };
        assert_eq!(
            validate(&a, &known),
            Err(AutomationError::UnknownConditionSensor {
                condition: "unknown_var=1".into(),
                sensor: "unknown_var".into(),
            })
        );

        // Valid automation
        let a = Automation {
            trigger: Trigger::Below {
                sensor: "temp".into(),
                threshold: 20.0,
                hysteresis: 0.0,
            },
            conditions: vec!["mains=on".into()],
            action: Action::Send,
            text: "valid".into(),
        };
        assert!(validate(&a, &known).is_ok());

        // Error display Ukrainian checks
        assert!(!AutomationError::InvalidStep.to_string().is_empty());
        assert!(!AutomationError::InvalidHysteresis.to_string().is_empty());
        assert!(!AutomationError::ZeroInterval.to_string().is_empty());
        assert!(!AutomationError::InvalidMinute(1500).to_string().is_empty());
        assert!(!AutomationError::UnknownSensor("x".into()).to_string().is_empty());
        assert!(!AutomationError::UnknownConditionSensor {
            condition: "x=1".into(),
            sensor: "x".into(),
        }
        .to_string()
        .is_empty());
        assert!(!AutomationError::BadCondition {
            condition: "x".into(),
            error: TemplateError::Unknown,
        }
        .to_string()
        .is_empty());
    }
}
