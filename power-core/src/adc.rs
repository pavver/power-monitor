//! A burst of ADC conversions on the battery pin, and whether it is steady
//! enough to be a voltage: a pin left floating (no divider fitted) wanders.

/// The most the pin may wander, between the 10th and the 90th percentile of
/// a burst, and still count as a voltage. The divider's capacitor holds it
/// within a few tens of millivolts; a floating pin swings by hundreds.
pub const MAX_SPREAD_MV: u16 = 100;

/// What the pin showed over one burst of conversions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pin {
    pub mean_mv: u32,
    /// The 10th to 90th percentile range: a rare spike is left out.
    pub spread_mv: u16,
}

impl Pin {
    pub fn of(samples: &[u16]) -> Option<Self> {
        let mut sorted = samples.to_vec();
        sorted.sort_unstable();
        let tail = sorted.len() / 10;
        let spread_mv = sorted.get(sorted.len().checked_sub(1 + tail)?)? - sorted[tail];
        let total: u32 = samples.iter().map(|&mv| u32::from(mv)).sum();
        let mean_mv = total / u32::try_from(samples.len()).ok()?;
        Some(Self { mean_mv, spread_mv })
    }

    /// The pin voltage, or `None` when it wandered too much to be one.
    pub fn steady_mv(&self) -> Option<u32> {
        (self.spread_mv <= MAX_SPREAD_MV).then_some(self.mean_mv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_pin_gives_the_mean() {
        let samples = [1_900, 1_910, 1_905, 1_895, 1_900, 1_890, 1_910, 1_900];
        let pin = Pin::of(&samples).unwrap();
        assert_eq!(pin.mean_mv, 1_901);
        assert_eq!(pin.steady_mv(), Some(1_901));
    }

    #[test]
    fn a_wandering_pin_is_noise() {
        let samples = [120, 900, 40, 1_600, 300, 2_100, 0, 700];
        assert_eq!(Pin::of(&samples).unwrap().steady_mv(), None);
    }

    #[test]
    fn a_rare_spike_is_not_noise() {
        // Wi-Fi bursts knock out a conversion now and then.
        let mut samples = [1_000; 64];
        samples[10] = 2_500;
        samples[40] = 0;
        assert_eq!(Pin::of(&samples).unwrap().spread_mv, 0);
        assert!(Pin::of(&samples).unwrap().steady_mv().is_some());
    }

    #[test]
    fn spread_just_at_the_limit_is_still_steady() {
        let low = [1_000; 32];
        let at_limit = [1_000 + MAX_SPREAD_MV; 32];
        let samples: Vec<u16> = low.into_iter().chain(at_limit).collect();
        assert_eq!(Pin::of(&samples).unwrap().spread_mv, MAX_SPREAD_MV);
        assert!(Pin::of(&samples).unwrap().steady_mv().is_some());
        let over: Vec<u16> = low.into_iter().chain([1_001 + MAX_SPREAD_MV; 32]).collect();
        assert_eq!(Pin::of(&over).unwrap().steady_mv(), None);
    }

    #[test]
    fn no_samples_is_no_voltage() {
        assert_eq!(Pin::of(&[]), None);
    }
}
