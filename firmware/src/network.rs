//! Wi-Fi. One thread owns the driver; the web server sends it commands.
//!
//! Without saved credentials (or when the saved network is unreachable at
//! start-up) the setup access point runs alongside the station, with a DNS
//! responder for the captive portal, until the next restart.
//!
//! Once the setup access point runs, only the station's configuration is
//! touched: re-applying the access point's restarts it and drops every phone,
//! closing the portal before it can show a failed connection.

use std::ffi::CString;
use std::net::UdpSocket;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::ipv4::{self, Ipv4Addr};
use esp_idf_svc::netif::{EspNetif, NetifConfiguration, NetifStack};
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::sys::{
    esp, esp_netif_dhcp_option_id_t_ESP_NETIF_CAPTIVEPORTAL_URI, esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
    esp_netif_dhcps_option, esp_netif_dhcps_start, esp_netif_dhcps_stop, esp_wifi_get_config, esp_wifi_set_config,
    wifi_auth_mode_t_WIFI_AUTH_OPEN, wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK, wifi_config_t, wifi_interface_t_WIFI_IF_STA,
    ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED,
};
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi, WifiDriver,
};
use power_core::api::Link;
use power_core::dns;
use power_core::message::Notice;
use power_core::portal::{portal_url, SETUP_IP};
use power_core::radio::{self, Seen};
use power_core::settings::WifiCredentials;

use crate::settings::Settings;
use crate::status::Status;

pub const SETUP_SSID: &str = "PowerMonitor-Setup";
const RECONNECT_EVERY: Duration = Duration::from_secs(10);
/// Scans disturb clients of the setup access point, so they are rationed.
const SCAN_AT_MOST_EVERY: Duration = Duration::from_secs(8);

pub enum Command {
    Scan,
    Connect(WifiCredentials),
}

type Wifi = BlockingWifi<EspWifi<'static>>;

struct Radio {
    wifi: Wifi,
    setup_ap: bool,
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
    let mut network = Radio { wifi, setup_ap: false, last_scan: None, seen: Vec::new(), settings, status, notices };
    network.boot()?;
    let (commands, inbox) = mpsc::channel();
    thread::Builder::new().stack_size(8 * 1024).spawn(move || network.run(inbox))?;
    Ok(commands)
}

impl Radio {
    fn boot(&mut self) -> anyhow::Result<()> {
        let saved = self.settings.wifi();
        let station = saved.as_ref().map(client).transpose()?.unwrap_or_default();
        self.wifi.set_configuration(&Configuration::Client(station.clone()))?;
        self.wifi.start()?;
        if let Some(saved) = &saved {
            if self.connect(saved).is_ok() {
                return Ok(());
            }
        }
        self.start_setup_ap(station)
    }

    fn run(mut self, inbox: Receiver<Command>) {
        loop {
            match inbox.recv_timeout(RECONNECT_EVERY) {
                Ok(Command::Scan) => self.scan(),
                Ok(Command::Connect(credentials)) => self.switch_to(credentials),
                Err(RecvTimeoutError::Timeout) => self.keep_connected(),
                Err(RecvTimeoutError::Disconnected) => return,
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
            if let Err(error) = self.settings.save_wifi(credentials) {
                log::error!("saving wifi: {error}");
            }
        }
    }

    /// Reconnects to the saved network after a drop. While phones may be on
    /// the setup network, only when the saved network was heard, so the radio
    /// does not sweep every channel each time.
    fn keep_connected(&mut self) {
        let Some(saved) = self.settings.wifi() else { return };
        if self.wifi.is_connected().unwrap_or(false) {
            return;
        }
        if self.setup_ap && radio::strongest(&self.seen, saved.ssid()).is_none() {
            return;
        }
        if self.aim_station(&saved).is_ok() {
            let _ = self.connect(&saved);
        }
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
        let result = self.wifi.connect().and_then(|()| self.wifi.wait_netif_up());
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
    fn start_setup_ap(&mut self, station: ClientConfiguration) -> anyhow::Result<()> {
        self.scan();
        let preferred = self.settings.wifi();
        let channel = radio::setup_channel(&self.seen, preferred.as_ref().map(|w| w.ssid()));
        self.wifi.stop()?;
        self.setup_ap = true;
        self.wifi.set_configuration(&Configuration::Mixed(station, setup_ap(channel)))?;
        self.wifi.start()?;
        offer_portal_uri(self.wifi.wifi().ap_netif())?;
        log::info!("setup access point {SETUP_SSID} on channel {channel} at {}", portal_url());
        self.status.set_setup_ap(SETUP_SSID);
        spawn_dns(SETUP_IP)
    }
}

/// The setup network's interface. Its DHCP server must name the device itself
/// as DNS server (the library default is 8.8.8.8, unreachable here), otherwise
/// phones never ask our DNS responder and the portal does not open.
fn setup_ap_netif() -> anyhow::Result<EspNetif> {
    let ip = Ipv4Addr::from(SETUP_IP);
    let mut conf = NetifConfiguration::wifi_default_router();
    conf.ip_configuration = Some(ipv4::Configuration::Router(ipv4::RouterConfiguration {
        subnet: ipv4::Subnet { gateway: ip, mask: ipv4::Mask(24) },
        dhcp_enabled: true,
        dns: Some(ip),
        secondary_dns: None,
    }));
    Ok(EspNetif::new_with_conf(&conf)?)
}

/// DHCP option 114 (RFC 8910): recent Android and iOS open this URL as the
/// captive portal directly, without relying on their connectivity probes.
fn offer_portal_uri(netif: &EspNetif) -> anyhow::Result<()> {
    // DHCP keeps the pointer, so the string lives for the whole program.
    static URI: OnceLock<CString> = OnceLock::new();
    let uri = URI.get_or_init(|| CString::new(portal_url()).unwrap());
    let handle = netif.handle();
    // SAFETY: `handle` is the live AP interface; option value and length
    // describe a NUL-terminated string that is never freed.
    unsafe {
        esp!(esp_netif_dhcps_stop(handle)).or_else(|e| {
            // Already stopped is fine.
            if e.code() == ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED { Ok(()) } else { Err(e) }
        })?;
        esp!(esp_netif_dhcps_option(
            handle,
            esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
            esp_netif_dhcp_option_id_t_ESP_NETIF_CAPTIVEPORTAL_URI,
            uri.as_ptr() as *mut _,
            uri.as_bytes().len() as u32,
        ))?;
        esp!(esp_netif_dhcps_start(handle))?;
    }
    Ok(())
}

/// Answers every DNS query with our address so phones open the portal.
fn spawn_dns(ip: [u8; 4]) -> anyhow::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:53")?;
    thread::Builder::new().stack_size(4096).spawn(move || {
        let mut buf = [0u8; 512];
        loop {
            let Ok((len, peer)) = socket.recv_from(&mut buf) else { continue };
            if let Some(reply) = dns::answer(&buf[..len], ip) {
                let _ = socket.send_to(&reply, peer);
            }
        }
    })?;
    Ok(())
}

fn setup_ap(channel: u8) -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: SETUP_SSID.try_into().unwrap(),
        channel,
        auth_method: AuthMethod::None,
        max_connections: 4,
        ..Default::default()
    }
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
