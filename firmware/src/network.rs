//! Wi-Fi. One thread owns the driver; the web server sends it commands.
//!
//! While the station has no link to the home network, or a phone is still on
//! it, the setup access point runs alongside the station, with a DNS
//! responder for the captive portal. It closes once the station is linked and
//! nobody is on it (or when the page asks), and opens again when the link is
//! lost and a reconnect fails.
//!
//! Once the setup access point runs, only the station's configuration is
//! touched: re-applying the access point's restarts it and drops every phone,
//! closing the portal before it can show a failed connection.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::netif::{EspNetif, NetifStack};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    esp, esp_wifi_ap_get_sta_list, esp_wifi_get_config, esp_wifi_set_config, esp_wifi_set_mode,
    wifi_auth_mode_t_WIFI_AUTH_OPEN, wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK, wifi_config_t, wifi_interface_t_WIFI_IF_STA,
    wifi_mode_t_WIFI_MODE_STA, wifi_sta_list_t, EspError,
};
use esp_idf_svc::wifi::{
    AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi, WifiDriver,
};
use power_core::api::{Link, SetupAp};
use power_core::message::Notice;
use power_core::portal::{portal_url, SETUP_IP};
use power_core::radio::{self, Seen, SetupApStep};
use power_core::settings::WifiCredentials;

use crate::settings::Settings;
use crate::status::Status;

mod setup_ap;
use setup_ap::{announce_channel_switches, offer_portal_uri, setup_ap, setup_ap_netif, spawn_dns};

pub const SETUP_SSID: &str = "PowerMonitor-Setup";
const RECONNECT_EVERY: Duration = Duration::from_secs(10);
/// Scans disturb clients of the setup access point, so they are rationed.
const SCAN_AT_MOST_EVERY: Duration = Duration::from_secs(8);
/// The driver makes one association attempt per connect and does not retry;
/// the first often fails while the access point moves to the router's
/// channel. Attempts × wait plus the address wait stay within the page's 60 s.
const JOIN_ATTEMPTS: u32 = 3;
const JOIN_WAIT: Duration = Duration::from_secs(10);
/// After a join from the page the setup network waits this long for the
/// phone, which the channel change may have dropped, to come back.
const HOLD_AFTER_JOIN: Duration = Duration::from_secs(180);

pub enum Command {
    Scan,
    Connect(WifiCredentials),
    /// The user is moving to the home network, or setup is complete.
    CloseSetup,
}

type Wifi = BlockingWifi<EspWifi<'static>>;

struct Radio {
    wifi: Wifi,
    setup_ap: bool,
    /// The captive portal's DNS responder; it outlives the access point.
    dns_running: bool,
    /// Until when the setup network stays up for a phone to come back.
    hold_until: Option<Instant>,
    last_scan: Option<Instant>,
    /// Access points heard in the last scan.
    seen: Vec<Seen>,
    settings: Arc<Settings>,
    status: Arc<Status>,
    notices: Sender<Notice>,
}

pub fn start(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    settings: Arc<Settings>,
    status: Arc<Status>,
    notices: Sender<Notice>,
) -> anyhow::Result<Sender<Command>> {
    let driver = WifiDriver::new(modem, sys_loop.clone(), Some(nvs))?;
    let esp_wifi = EspWifi::wrap_all(driver, EspNetif::new(NetifStack::Sta)?, setup_ap_netif()?)?;
    let wifi = BlockingWifi::wrap(esp_wifi, sys_loop)?;
    let mut network = Radio { wifi, setup_ap: false, dns_running: false, hold_until: None, last_scan: None, seen: Vec::new(), settings, status, notices };
    network.boot()?;
    let (commands, inbox) = mpsc::channel();
    thread::Builder::new().stack_size(8 * 1024).spawn(move || network.run(inbox))?;
    Ok(commands)
}

impl Radio {
    fn boot(&mut self) -> anyhow::Result<()> {
        let saved = self.settings.wifi();
        self.wifi.set_configuration(&Configuration::Client(station(saved.as_ref())?))?;
        self.wifi.start()?;
        if let Some(saved) = &saved {
            if self.connect(saved).is_ok() {
                return Ok(());
            }
        }
        // The access point comes first, on the saved network's channel, so
        // joining that network afterwards does not move it.
        self.start_setup_ap()
    }

    /// Commands as they come; the link is checked on a fixed beat, however
    /// often the page asks for scans.
    fn run(mut self, inbox: Receiver<Command>) {
        let mut next_check = Instant::now() + RECONNECT_EVERY;
        loop {
            match inbox.recv_timeout(next_check.saturating_duration_since(Instant::now())) {
                Ok(Command::Scan) => self.scan(),
                Ok(Command::Connect(credentials)) => self.switch_to(credentials),
                Ok(Command::CloseSetup) => self.close_setup(),
                Err(RecvTimeoutError::Timeout) => {
                    self.check_link();
                    next_check = Instant::now() + RECONNECT_EVERY;
                }
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Reconnects if needed, then opens or closes the setup access point.
    fn check_link(&mut self) {
        self.keep_connected();
        let linked = self.wifi.is_connected().unwrap_or(false);
        let held = self.hold_until.is_some_and(|until| Instant::now() < until);
        match radio::setup_ap_step(self.setup_ap, linked, self.phones(), held) {
            SetupApStep::Open => {
                log::warn!("no link to the home network: opening the setup access point");
                if let Err(error) = self.start_setup_ap() {
                    log::error!("opening the setup access point: {error}");
                }
            }
            SetupApStep::Close => {
                log::info!("linked and nobody on the setup access point");
                self.close_setup_ap();
            }
            SetupApStep::Keep => {}
        }
    }

    /// Phones on the setup access point.
    fn phones(&self) -> usize {
        if !self.setup_ap {
            return 0;
        }
        let mut list = wifi_sta_list_t::default();
        // SAFETY: the driver runs with the access point up; the list is ours.
        match esp!(unsafe { esp_wifi_ap_get_sta_list(&mut list) }) {
            Ok(()) => list.num.max(0) as usize,
            Err(error) => {
                log::warn!("listing setup access point clients: {error}");
                // Unknown counts as someone there: never cut a phone off.
                1
            }
        }
    }

    fn scan(&mut self) {
        if self.last_scan.is_some_and(|at| at.elapsed() < SCAN_AT_MOST_EVERY) {
            return;
        }
        self.last_scan = Some(Instant::now());
        match self.wifi.scan() {
            Ok(found) => {
                self.seen = found
                    .into_iter()
                    .map(|ap| Seen {
                        ssid: ap.ssid.to_string(),
                        rssi: ap.signal_strength,
                        channel: ap.channel,
                        bssid: ap.bssid,
                        secure: !matches!(ap.auth_method, None | Some(AuthMethod::None)),
                    })
                    .collect();
                self.status.set_networks(radio::networks(&self.seen));
            }
            Err(error) => log::warn!("wifi scan: {error}"),
        }
    }

    /// Joins a network chosen on the page; it is saved only once it works.
    fn switch_to(&mut self, credentials: WifiCredentials) {
        let _ = self.wifi.disconnect();
        if let Err(error) = self.aim_station(&credentials) {
            self.fail(&credentials, &error.to_string());
            return;
        }
        if self.connect(&credentials).is_ok() {
            self.hold_until = Some(Instant::now() + HOLD_AFTER_JOIN);
            if let Err(error) = self.settings.save_wifi(credentials) {
                log::error!("saving wifi: {error}");
            }
        }
    }

    /// The page asks: the user moves to the home network. Refused while the
    /// station is not connected: the setup network would be the only way back in.
    fn close_setup(&mut self) {
        if !self.setup_ap {
            return;
        }
        if !self.wifi.is_connected().unwrap_or(false) {
            log::warn!("not closing the setup access point: wifi is not connected");
            return;
        }
        // Lets the page's reply reach the phone before its network goes away.
        thread::sleep(Duration::from_secs(1));
        self.close_setup_ap();
    }

    /// Leaves only the station, which keeps its link.
    fn close_setup_ap(&mut self) {
        self.hold_until = None;
        // SAFETY: a mode change on the started driver; the station keeps its link.
        if let Err(error) = esp!(unsafe { esp_wifi_set_mode(wifi_mode_t_WIFI_MODE_STA) }) {
            log::warn!("closing the setup access point: {error}");
            return;
        }
        self.setup_ap = false;
        self.status.set_setup_ap(None);
        log::info!("setup access point closed");
    }

    /// Reconnects to the saved network after a drop. While phones may be on
    /// the setup network, only when the saved network was heard, so the radio
    /// does not sweep every channel each time; now and then it listens again.
    fn keep_connected(&mut self) {
        let Some(saved) = self.settings.wifi() else { return };
        if self.wifi.is_connected().unwrap_or(false) {
            return;
        }
        if self.setup_ap && !self.heard(saved.ssid()) {
            return;
        }
        if self.aim_station(&saved).is_ok() {
            let _ = self.connect(&saved);
        }
    }

    /// Whether `ssid` is on the air, scanning again when the last scan that
    /// missed it is old.
    fn heard(&mut self, ssid: &str) -> bool {
        if radio::strongest(&self.seen, ssid).is_none() && radio::rescan_due(self.last_scan.map(|at| at.elapsed())) {
            self.scan();
        }
        radio::strongest(&self.seen, ssid).is_some()
    }

    /// Points the station at `credentials`, on the channel and access point
    /// heard in the last scan when there is one. Leaves the access point alone.
    fn aim_station(&self, credentials: &WifiCredentials) -> anyhow::Result<()> {
        let target = radio::strongest(&self.seen, credentials.ssid());
        let mut conf = wifi_config_t::default();
        // SAFETY: the driver is initialised; the station view of the union is
        // the one read and written for WIFI_IF_STA.
        unsafe {
            esp!(esp_wifi_get_config(wifi_interface_t_WIFI_IF_STA, &mut conf))?;
            let sta = &mut conf.sta;
            sta.ssid = padded(credentials.ssid()).ok_or_else(|| anyhow::anyhow!("ssid too long"))?;
            sta.password = padded(credentials.password()).ok_or_else(|| anyhow::anyhow!("password too long"))?;
            sta.threshold.authmode = if credentials.password().is_empty() {
                wifi_auth_mode_t_WIFI_AUTH_OPEN
            } else {
                wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK
            };
            sta.channel = target.map_or(0, |t| t.channel);
            sta.bssid_set = target.is_some();
            sta.bssid = target.map_or([0; 6], |t| t.bssid);
            esp!(esp_wifi_set_config(wifi_interface_t_WIFI_IF_STA, &mut conf))?;
        }
        Ok(())
    }

    fn connect(&mut self, credentials: &WifiCredentials) -> anyhow::Result<()> {
        let ssid = credentials.ssid().to_string();
        self.status.set_link(Link::Connecting { ssid: ssid.clone() });
        let result = self.join().and_then(|()| self.wifi.wait_netif_up());
        if let Err(error) = result {
            log::warn!("wifi connect {ssid}: {error}");
            let reason = if radio::strongest(&self.seen, &ssid).is_some() || self.seen.is_empty() {
                "не вдалося підключитися — неправильний пароль або мережа відхилила підключення"
            } else {
                "мережу не знайдено поруч"
            };
            self.fail(credentials, reason);
            anyhow::bail!(error);
        }
        let ip = self.wifi.wifi().sta_netif().get_ip_info()?.ip.to_string();
        log::info!("wifi connected to {ssid}, ip {ip}");
        self.report_if_new(&ip);
        self.status.set_link(Link::Connected { ssid, ip });
        Ok(())
    }

    /// Associates with the network the station is aimed at, retrying a
    /// dropped attempt.
    fn join(&mut self) -> Result<(), EspError> {
        let mut result = Ok(());
        for attempt in 1..=JOIN_ATTEMPTS {
            self.wifi.wifi_mut().connect()?;
            result = self.wifi.wifi_wait_while(|| self.wifi.is_connected().map(|c| !c), Some(JOIN_WAIT));
            if result.is_ok() {
                return result;
            }
            log::warn!("wifi join attempt {attempt} of {JOIN_ATTEMPTS} failed");
            // Ends the attempt still in progress, so the next starts clean.
            let _ = self.wifi.wifi_mut().disconnect();
        }
        result
    }

    /// Remembers the address and tells the chat when it differs from the last one.
    fn report_if_new(&self, ip: &str) {
        if self.settings.last_ip().as_deref() == Some(ip) {
            return;
        }
        if let Err(error) = self.settings.save_last_ip(ip) {
            log::error!("saving address: {error}");
        }
        let _ = self.notices.send(Notice::Address(ip.to_string()));
    }

    fn fail(&self, credentials: &WifiCredentials, reason: &str) {
        self.status.set_link(Link::Failed {
            ssid: credentials.ssid().to_string(),
            reason: reason.to_string(),
        });
    }

    /// Starts the setup network on the channel the station is most likely to
    /// join, so that joining it later does not move the phones' channel.
    /// Expects the station to be started (it scans first).
    fn start_setup_ap(&mut self) -> anyhow::Result<()> {
        self.scan();
        let preferred = self.settings.wifi();
        let channel = radio::setup_channel(&self.seen, preferred.as_ref().map(|w| w.ssid()));
        self.wifi.stop()?;
        self.setup_ap = true;
        self.wifi.set_configuration(&Configuration::Mixed(station(preferred.as_ref())?, setup_ap(channel)))?;
        announce_channel_switches()?;
        self.wifi.start()?;
        offer_portal_uri(self.wifi.wifi().ap_netif())?;
        log::info!("setup access point {SETUP_SSID} on channel {channel} at {}", portal_url());
        self.status.set_setup_ap(Some(SetupAp::new(SETUP_SSID)));
        if !self.dns_running {
            spawn_dns(SETUP_IP)?;
            self.dns_running = true;
        }
        Ok(())
    }
}

/// The station's configuration: the saved network, or none. Without saved
/// credentials the station only scans; an open threshold keeps the driver
/// from warning about the empty password.
fn station(saved: Option<&WifiCredentials>) -> anyhow::Result<ClientConfiguration> {
    let idle = || ClientConfiguration { auth_method: AuthMethod::None, ..Default::default() };
    Ok(saved.map(client).transpose()?.unwrap_or_else(idle))
}

fn client(credentials: &WifiCredentials) -> anyhow::Result<ClientConfiguration> {
    let auth_method = if credentials.password().is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    Ok(ClientConfiguration {
        ssid: credentials.ssid().try_into().map_err(|_| anyhow::anyhow!("ssid too long"))?,
        password: credentials.password().try_into().map_err(|_| anyhow::anyhow!("password too long"))?,
        auth_method,
        ..Default::default()
    })
}

/// `text` in a zero-padded driver field, or `None` when it does not fit.
fn padded<const N: usize>(text: &str) -> Option<[u8; N]> {
    let mut field = [0u8; N];
    field.get_mut(..text.len())?.copy_from_slice(text.as_bytes());
    Some(field)
}
