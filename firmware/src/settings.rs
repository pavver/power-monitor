//! Saved settings: NVS plus an in-memory copy. Each part has one writer —
//! Wi-Fi and last address: the network thread; bot, messages and the page
//! password: the web server.

use std::sync::{Mutex, RwLock};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use power_core::auth::PasswordHash;
use power_core::settings::{Bot, Messages, WifiCredentials};

const NAMESPACE: &str = "monitor";
const WIFI: &str = "wifi";
const BOT: &str = "bot";
const MESSAGES: &str = "messages";
const PASSWORD: &str = "password";
const LAST_IP: &str = "last_ip";
const QUICK_BOOTS: &str = "quick_boots";

pub struct Settings {
    nvs: Mutex<EspNvs<NvsDefault>>,
    wifi: RwLock<Option<WifiCredentials>>,
    bot: RwLock<Option<Bot>>,
    messages: RwLock<Messages>,
    password: RwLock<Option<PasswordHash>>,
    last_ip: RwLock<Option<String>>,
}

impl Settings {
    /// Loads what is stored; a part that is missing or no longer valid counts as unset.
    pub fn load(partition: EspDefaultNvsPartition) -> anyhow::Result<Self> {
        let nvs = EspNvs::new(partition, NAMESPACE, true)?;
        let wifi = read(&nvs, WIFI, WifiCredentials::from_form);
        let bot = read(&nvs, BOT, Bot::from_form);
        let messages = read(&nvs, MESSAGES, Messages::from_form).unwrap_or_default();
        let password = read(&nvs, PASSWORD, |s| PasswordHash::from_stored(s).ok_or(()));
        let last_ip = read(&nvs, LAST_IP, |s| Ok::<_, ()>(s.to_string()));
        Ok(Self {
            nvs: Mutex::new(nvs),
            wifi: RwLock::new(wifi),
            bot: RwLock::new(bot),
            messages: RwLock::new(messages),
            password: RwLock::new(password),
            last_ip: RwLock::new(last_ip),
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

    pub fn password(&self) -> Option<PasswordHash> {
        self.password.read().unwrap().clone()
    }

    /// `None` opens the page to everyone again.
    pub fn save_password(&self, password: Option<PasswordHash>) -> anyhow::Result<()> {
        match &password {
            Some(hash) => self.write(PASSWORD, &hash.to_stored())?,
            None => {
                self.nvs.lock().unwrap().remove(PASSWORD)?;
            }
        }
        *self.password.write().unwrap() = password;
        Ok(())
    }

    pub fn last_ip(&self) -> Option<String> {
        self.last_ip.read().unwrap().clone()
    }

    pub fn save_last_ip(&self, ip: &str) -> anyhow::Result<()> {
        self.write(LAST_IP, ip)?;
        *self.last_ip.write().unwrap() = Some(ip.to_string());
        Ok(())
    }

    /// Quick start-ups in a row before this one (see power_core::reset).
    pub fn quick_boots(&self) -> u32 {
        self.nvs.lock().unwrap().get_u32(QUICK_BOOTS).ok().flatten().unwrap_or(0)
    }

    pub fn set_quick_boots(&self, count: u32) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().set_u32(QUICK_BOOTS, count)?;
        Ok(())
    }

    /// Wipes everything this device stored; takes effect after a restart.
    pub fn factory_reset(&self) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().erase_all()?;
        Ok(())
    }

    fn write(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().set_str(key, value)?;
        Ok(())
    }
}

fn read<T, E>(nvs: &EspNvs<NvsDefault>, key: &str, parse: fn(&str) -> Result<T, E>) -> Option<T> {
    let mut buf = vec![0u8; 4096];
    let stored = nvs.get_str(key, &mut buf).ok().flatten()?;
    parse(stored).ok()
}
