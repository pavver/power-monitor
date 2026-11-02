//! The battery voltage through the resistor divider on GPIO34 (ADC1).

use std::sync::{Arc, Mutex};

use esp_idf_svc::hal::adc::attenuation::DB_12;
use esp_idf_svc::hal::adc::oneshot::config::{AdcChannelConfig, Calibration};
use esp_idf_svc::hal::adc::oneshot::{AdcChannelDriver, AdcDriver};
use esp_idf_svc::hal::adc::{ADC1, ADCCH6, ADCU1};
use esp_idf_svc::hal::gpio::Gpio34;

use crate::settings::Settings;

/// Conversions averaged per reading: single ones wander by tens of millivolts.
const SAMPLES: u32 = 64;

type Channel = AdcChannelDriver<'static, ADCCH6<ADCU1>, AdcDriver<'static, ADCU1>>;

/// Shared by the notifier and the web server; one reading at a time.
#[derive(Clone)]
pub struct Voltmeter {
    channel: Arc<Mutex<Channel>>,
    settings: Arc<Settings>,
}

impl Voltmeter {
    pub fn new(adc: ADC1<'static>, pin: Gpio34<'static>, settings: Arc<Settings>) -> anyhow::Result<Self> {
        // 12 dB reads up to ~2.45 V accurately; the factory calibration
        // in eFuse straightens the line.
        let config = AdcChannelConfig { attenuation: DB_12, calibration: Calibration::Line, ..Default::default() };
        let channel = AdcChannelDriver::new(AdcDriver::new(adc)?, pin, &config)?;
        Ok(Self { channel: Arc::new(Mutex::new(channel)), settings })
    }

    /// The battery voltage, or `None` without a divider set (the pin then
    /// floats) or when the ADC fails.
    pub fn battery_mv(&self) -> Option<u32> {
        let divider = self.settings.divider()?;
        let mut channel = self.channel.lock().unwrap();
        let mut total = 0;
        for _ in 0..SAMPLES {
            match channel.read() {
                Ok(mv) => total += u32::from(mv),
                Err(error) => {
                    log::warn!("battery voltage: {error}");
                    return None;
                }
            }
        }
        Some(divider.battery_mv(total / SAMPLES))
    }
}
