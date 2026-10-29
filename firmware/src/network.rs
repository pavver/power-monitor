//! Wi-Fi. One thread owns the driver; the web server sends it commands.
//!
//! Without saved credentials (or when the saved network is unreachable at
//! start-up) the setup access point runs alongside the station, with a DNS
//! responder for the captive portal, until the next restart.

use std::net::UdpSocket;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use power_core::api::{tidy_networks, Link, Network};
use power_core::dns;
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
}

pub fn start(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    settings: Arc<Settings>,
    status: Arc<Status>,
) -> anyhow::Result<Sender<Command>> {
    let wifi = BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;
    let mut network = Radio { wifi, setup_ap: false, last_scan: None, settings, status };
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
        self.status.set_link(Link::Connected { ssid, ip });
        Ok(())
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
        let ip = self.wifi.wifi().ap_netif().get_ip_info()?.ip;
        log::info!("setup access point {SETUP_SSID} at {ip}");
        self.status.set_setup_ap(SETUP_SSID);
        spawn_dns(ip.octets())
    }
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
