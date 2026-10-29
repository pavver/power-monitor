//! Factory reset without buttons: power the device off and on several times
//! in a row, each time within a few seconds of start-up.

/// Quick start-ups in a row that wipe the settings.
pub const QUICK_BOOTS_FOR_RESET: u32 = 5;
/// A start-up counts as quick until the device has run this long.
pub const QUICK_BOOT_MS: u64 = 10_000;

/// Why the chip started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Start {
    /// Power was switched on: the only start a person makes on purpose.
    PowerOn,
    /// Brownout, crash, watchdog, or a restart the firmware asked for.
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootAction {
    /// Store this count; it is cleared once the device has run long enough.
    Count(u32),
    /// Leave the count as it is.
    Keep,
    FactoryReset,
}

/// Decides at start-up, given how many quick start-ups came right before.
pub fn on_boot(quick_boots_before: u32, start: Start) -> BootAction {
    if start != Start::PowerOn {
        return BootAction::Keep;
    }
    match quick_boots_before.saturating_add(1) {
        count if count >= QUICK_BOOTS_FOR_RESET => BootAction::FactoryReset,
        count => BootAction::Count(count),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_quick_start_ups() {
        assert_eq!(on_boot(0, Start::PowerOn), BootAction::Count(1));
        assert_eq!(on_boot(3, Start::PowerOn), BootAction::Count(4));
    }

    #[test]
    fn the_fifth_quick_start_up_resets() {
        assert_eq!(on_boot(QUICK_BOOTS_FOR_RESET - 1, Start::PowerOn), BootAction::FactoryReset);
    }

    #[test]
    fn a_stale_high_count_still_resets() {
        assert_eq!(on_boot(u32::MAX, Start::PowerOn), BootAction::FactoryReset);
    }

    #[test]
    fn restarts_without_a_power_cut_do_not_count() {
        // A weak supply resets the chip when the radio starts (brownout); a
        // loop of those must not wipe the settings. Neither do crashes or
        // restarts the firmware asks for.
        assert_eq!(on_boot(QUICK_BOOTS_FOR_RESET - 1, Start::Other), BootAction::Keep);
        assert_eq!(on_boot(0, Start::Other), BootAction::Keep);
    }
}
