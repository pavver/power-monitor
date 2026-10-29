//! Saved settings: NVS plus an in-memory copy. Each part has one writer —
//! Wi-Fi: the network thread (after a successful connect), bot and
//! messages: the web server.

use std::sync::{Mutex, RwLock};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use power_core::settings::{Bot, Messages, SettingsError, WifiCredentials};

const NAMESPACE: &str = "monitor";
const WIFI: &str = "wifi";
const BOT: &str = "bot";
const MESSAGES: &str = "messages";

pub struct Settings {
    nvs: Mutex<EspNvs<NvsDefault>>,
    wifi: RwLock<Option<WifiCredentials>>,
    bot: RwLock<Option<Bot>>,
    messages: RwLock<Messages>,
}

impl Settings {
    /// Loads what is stored; a part that is missing or no longer valid counts as unset.
    pub fn load(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        let wifi = read(&nvs, WIFI, WifiCredentials::from_form);
        let bot = read(&nvs, BOT, Bot::from_form);
        let messages = read(&nvs, MESSAGES, Messages::from_form).unwrap_or_default();
        Ok(Self {
            nvs: Mutex::new(nvs),
            wifi: RwLock::new(wifi),
            bot: RwLock::new(bot),
            messages: RwLock::new(messages),
        })
    }

    pub fn wifi(&self) -> Option<WifiCredentials> {
        self.wifi.read().unwrap().clone()
    }

    pub fn bot(&self) -> Option<Bot> {
        self.bot.read().unwrap().clone()
    }

    pub fn messages(&self) -> Messages {
        self.messages.read().unwrap().clone()
    }

    pub fn save_wifi(&self, wifi: WifiCredentials) -> anyhow::Result<()> {
        self.write(WIFI, &wifi.to_form())?;
        *self.wifi.write().unwrap() = Some(wifi);
        Ok(())
    }

    pub fn save_bot(&self, bot: Bot) -> anyhow::Result<()> {
        self.write(BOT, &bot.to_form())?;
        *self.bot.write().unwrap() = Some(bot);
        Ok(())
    }

    pub fn save_messages(&self, messages: Messages) -> anyhow::Result<()> {
        self.write(MESSAGES, &messages.to_form())?;
        *self.messages.write().unwrap() = messages;
        Ok(())
    }

    fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().set_str(key, value)?;
        Ok(())
    }
}

fn read<T>(
    nvs: &EspNvs<NvsDefault>,
    key: &str,
    parse: fn(&str) -> Result<T, SettingsError>,
) -> Option<T> {
    let mut buf = vec![0u8; 4096];
    let stored = nvs.get_str(key, &mut buf).ok().flatten()?;
    parse(stored).ok()
}
