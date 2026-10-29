//! The config in NVS, stored in its urlencoded form.

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use power_core::config::Config;

const NAMESPACE: &str = "monitor";
const KEY: &str = "config";

pub struct Storage(EspNvs<NvsDefault>);

impl Storage {
    pub fn new(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        Ok(Self(EspNvs::new(partition, NAMESPACE, true)?))
    }

    /// The stored config; `None` when missing or no longer valid.
    pub fn load(&self) -> Option<Config> {
        let mut buf = [0u8; 1024];
        let stored = self.0.get_str(KEY, &mut buf).ok().flatten()?;
        Config::from_form(stored, None).ok()
    }

    pub fn save(&mut self, config: &Config) -> anyhow::Result<()> {
        self.0.set_str(KEY, &config.to_form())?;
        Ok(())
    }
}
