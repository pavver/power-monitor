//! Wi-Fi. One thread owns the driver; the web server sends it commands.
//!
//! Without saved credentials (or when the saved network is unreachable at
//! start-up) the setup access point runs alongside the station, with a DNS
//! responder for the captive portal, until the next restart.

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
    esp_netif_dhcps_option, esp_netif_dhcps_start, esp_netif_dhcps_stop, ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED,
};
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi, WifiDriver,
};
use power_core::api::{tidy_networks, Link, Network};
use power_core::dns;
use power_core::portal::{portal_url, SETUP_IP};
use power_core::message::Notice;
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
    let mut network = Radio { wifi, setup_ap: false, last_scan: None, settings, status, notices };
    network.boot()?;
    let (commands, inbox) = mpsc::channel();
    thread::Builder::new().stack_size(8 * 1024).spawn(move || network.run(inbox))?;
    Ok(commands)
}

impl Radio {
    fn boot(&mut self) -> anyhow::Result<()> {
        let Some(saved) = self.settings.wifi() else {
            return self.start_setup_ap(ClientConfiguration::default());
        };
        self.wifi.set_configuration(&Configuration::Client(client(&saved)?))?;
        self.wifi.start()?;
        if self.connect(&saved).is_err() {
            self.wifi.stop()?;
            self.start_setup_ap(client(&saved)?)?;
        }
        Ok(())
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
            Ok(found) => self.status.set_networks(tidy_networks(
                found
                    .into_iter()
                    .map(|ap| Network {
                        ssid: ap.ssid.to_string(),
                        rssi: ap.signal_strength,
                        secure: !matches!(ap.auth_method, None | Some(AuthMethod::None)),
                    })
                    .collect(),
            )),
            Err(error) => log::warn!("wifi scan: {error}"),
        }
    }

    /// Joins a network chosen on the page; it is saved only once it works.
    fn switch_to(&mut self, credentials: WifiCredentials) {
        let _ = self.wifi.disconnect();
        let applied = client(&credentials).and_then(|c| Ok(self.wifi.set_configuration(&self.configuration(c))?));
        if let Err(error) = applied {
            self.fail(&credentials, &error.to_string());
            return;
        }
        if self.connect(&credentials).is_ok() {
            if let Err(error) = self.settings.save_wifi(credentials) {
                log::error!("saving wifi: {error}");
            }
        }
    }

    /// Reconnects to the saved network after a drop.
    fn keep_connected(&mut self) {
        let Some(saved) = self.settings.wifi() else { return };
        if self.wifi.is_connected().unwrap_or(false) {
            return;
        }
        let applied = client(&saved).and_then(|c| Ok(self.wifi.set_configuration(&self.configuration(c))?));
        if applied.is_ok() {
            let _ = self.connect(&saved);
        }
    }

    fn connect(&mut self, credentials: &WifiCredentials) -> anyhow::Result<()> {
        let ssid = credentials.ssid().to_string();
        self.status.set_link(Link::Connecting { ssid: ssid.clone() });
        let result = self.wifi.connect().and_then(|()| self.wifi.wait_netif_up());
        if let Err(error) = result {
            log::warn!("wifi connect {ssid}: {error}");
            self.fail(credentials, "не вдалося підключитися — перевірте пароль і сигнал");
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

    fn configuration(&self, client: ClientConfiguration) -> Configuration {
        if self.setup_ap {
            Configuration::Mixed(client, setup_ap())
        } else {
            Configuration::Client(client)
        }
    }

    fn start_setup_ap(&mut self, client: ClientConfiguration) -> anyhow::Result<()> {
        self.setup_ap = true;
        self.wifi.set_configuration(&Configuration::Mixed(client, setup_ap()))?;
        self.wifi.start()?;
        offer_portal_uri(self.wifi.wifi().ap_netif())?;
        log::info!("setup access point {SETUP_SSID} at {}", portal_url());
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

fn setup_ap() -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: SETUP_SSID.try_into().unwrap(),
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
