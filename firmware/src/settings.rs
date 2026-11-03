//! Saved settings: NVS plus an in-memory copy. Each part has one writer —
//! Wi-Fi and last address: the network thread; bot, messages and the page
//! password, BMS, divider, remote support: the web server; the remote
//! identity: the remote thread, once.

use std::sync::{Mutex, RwLock};

use esp_idf_svc::nvs::{EspDefaultNvsPartition, EspNvs, NvsDefault};
use power_core::api::{self, Stage};
use power_core::auth::PasswordHash;
use power_core::remote::{Identity, RemoteSettings};
use power_core::settings::{Battery, Bot, Divider, Messages, WifiCredentials};

const NAMESPACE: &str = "monitor";
const WIFI: &str = "wifi";
const BOT: &str = "bot";
const MESSAGES: &str = "messages";
const PASSWORD: &str = "password";
const LAST_IP: &str = "last_ip";
const QUICK_BOOTS: &str = "quick_boots";
const BLUETOOTH_ONCE: &str = "bt_once";
const BATTERY: &str = "bms";
const DIVIDER: &str = "divider";
const REMOTE: &str = "remote";
const IDENTITY: &str = "remote_id";
/// `REMOTE_SERVER=https://… cargo build` makes new devices call that
/// server for remote support; without it support starts switched off.
const BUILT_IN_SERVER: Option<&str> = option_env!("REMOTE_SERVER");
/// `DIVIDER_TOP_OHM=… DIVIDER_BOTTOM_OHM=… cargo build` gives new devices
/// that divider instead of the one on sheet 1; build.rs has checked the values.
const BUILT_IN_DIVIDER: [Option<&str>; 2] = [option_env!("DIVIDER_TOP_OHM"), option_env!("DIVIDER_BOTTOM_OHM")];

pub struct Settings {
    nvs: Mutex<EspNvs<NvsDefault>>,
    wifi: RwLock<Option<WifiCredentials>>,
    bot: RwLock<Option<Bot>>,
    messages: RwLock<Messages>,
    password: RwLock<Option<PasswordHash>>,
    last_ip: RwLock<Option<String>>,
    battery: RwLock<Option<Battery>>,
    divider: RwLock<Option<Divider>>,
    remote: RwLock<RemoteSettings>,
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
        let battery = read(&nvs, BATTERY, Battery::from_form);
        let stored = read(&nvs, DIVIDER, |s| Ok::<_, ()>(s.to_string()));
        let divider = Divider::from_stored(stored.as_deref(), built_in_divider());
        let remote = read(&nvs, REMOTE, RemoteSettings::from_form).unwrap_or_else(built_in_remote);
        Ok(Self {
            nvs: Mutex::new(nvs),
            wifi: RwLock::new(wifi),
            bot: RwLock::new(bot),
            messages: RwLock::new(messages),
            password: RwLock::new(password),
            last_ip: RwLock::new(last_ip),
            battery: RwLock::new(battery),
            divider: RwLock::new(divider),
            remote: RwLock::new(remote),
        })
    }

    pub fn wifi(&self) -> Option<WifiCredentials> {
        self.wifi.read().unwrap().clone()
    }

    pub fn bot(&self) -> Option<Bot> {
        self.bot.read().unwrap().clone()
    }

    /// How far the setup has got.
    pub fn stage(&self) -> Stage {
        api::stage(self.wifi().is_some(), self.bot().is_some())
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

    pub fn battery(&self) -> Option<Battery> {
        self.battery.read().unwrap().clone()
    }

    /// `None` stops reading any BMS.
    pub fn save_battery(&self, battery: Option<Battery>) -> anyhow::Result<()> {
        match &battery {
            Some(battery) => self.write(BATTERY, &battery.to_form())?,
            None => {
                self.nvs.lock().unwrap().remove(BATTERY)?;
            }
        }
        *self.battery.write().unwrap() = battery;
        Ok(())
    }

    pub fn divider(&self) -> Option<Divider> {
        *self.divider.read().unwrap()
    }

    /// `None`: no divider fitted, the battery voltage is not read.
    pub fn save_divider(&self, divider: Option<Divider>) -> anyhow::Result<()> {
        self.write(DIVIDER, &Divider::to_stored(divider))?;
        *self.divider.write().unwrap() = divider;
        Ok(())
    }

    pub fn remote(&self) -> RemoteSettings {
        self.remote.read().unwrap().clone()
    }

    pub fn save_remote(&self, remote: RemoteSettings) -> anyhow::Result<()> {
        self.write(REMOTE, &remote.to_form())?;
        *self.remote.write().unwrap() = remote;
        Ok(())
    }

    /// Who this device is to the remote server: made on first use and kept,
    /// so the server keeps recognising it (a factory reset makes a new one).
    pub fn identity(&self) -> anyhow::Result<Identity> {
        let stored = read(&self.nvs.lock().unwrap(), IDENTITY, |s| Identity::from_form(s).ok_or(()));
        if let Some(identity) = stored {
            return Ok(identity);
        }
        let identity = Identity::from_random(&crate::random());
        self.write(IDENTITY, &identity.to_form())?;
        Ok(identity)
    }

    /// Quick start-ups in a row before this one (see power_core::reset).
    pub fn quick_boots(&self) -> u32 {
        self.nvs.lock().unwrap().get_u32(QUICK_BOOTS).ok().flatten().unwrap_or(0)
    }

    pub fn set_quick_boots(&self, count: u32) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().set_u32(QUICK_BOOTS, count)?;
        Ok(())
    }

    /// Keep Bluetooth at the next start even with no BMS saved, to look for one.
    pub fn ask_bluetooth_once(&self) -> anyhow::Result<()> {
        self.nvs.lock().unwrap().set_u8(BLUETOOTH_ONCE, 1)?;
        Ok(())
    }

    /// Whether this start was asked to keep Bluetooth; the request is used up.
    pub fn take_bluetooth_once(&self) -> anyhow::Result<bool> {
        let nvs = self.nvs.lock().unwrap();
        let asked = nvs.get_u8(BLUETOOTH_ONCE)?.is_some();
        if asked {
            nvs.remove(BLUETOOTH_ONCE)?;
        }
        Ok(asked)
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

fn built_in_remote() -> RemoteSettings {
    RemoteSettings::factory(BUILT_IN_SERVER)
}

fn built_in_divider() -> Divider {
    let [top, bottom] = BUILT_IN_DIVIDER;
    Divider::built_in(top, bottom).unwrap_or(Divider::SHEET_1)
}
