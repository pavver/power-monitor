//! Wi-Fi: station on the configured network, or a setup access point with a
//! captive portal when there is no config or the network cannot be reached.

use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{
    AccessPointConfiguration, AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi,
};
use power_core::config::Config;
use power_core::dns;

use crate::status::Status;

pub const SETUP_SSID: &str = "PowerMonitor-Setup";
const RECONNECT_EVERY: Duration = Duration::from_secs(10);

pub type Wifi = BlockingWifi<EspWifi<'static>>;

pub fn start(
    modem: Modem<'static>,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
    config: Option<&Config>,
    status: &Status,
) -> anyhow::Result<Wifi> {
    let mut wifi = BlockingWifi::wrap(EspWifi::new(modem, sys_loop.clone(), Some(nvs))?, sys_loop)?;
    match config {
        None => start_setup(&mut wifi, Configuration::AccessPoint(setup_ap()), status)?,
        Some(config) => {
            wifi.set_configuration(&Configuration::Client(client(config)?))?;
            wifi.start()?;
            if connect(&mut wifi, status).is_err() {
                let mixed = Configuration::Mixed(client(config)?, setup_ap());
                start_setup(&mut wifi, mixed, status)?;
            }
        }
    }
    Ok(wifi)
}

/// Keeps the station connected; runs forever.
pub fn supervise(mut wifi: Wifi, config: Option<&Config>, status: &Status) -> ! {
    loop {
        thread::sleep(RECONNECT_EVERY);
        if config.is_some() && !wifi.is_connected().unwrap_or(false) {
            status.set_network("Wi-Fi: немає зв'язку, перепідключення…");
            let _ = connect(&mut wifi, status);
        }
    }
}

fn connect(wifi: &mut Wifi, status: &Status) -> anyhow::Result<()> {
    let result = wifi.connect().and_then(|()| wifi.wait_netif_up());
    match &result {
        Ok(()) => {
            let ip = wifi.wifi().sta_netif().get_ip_info()?.ip;
            let ssid = match wifi.get_configuration()? {
                Configuration::Client(c) | Configuration::Mixed(c, _) => c.ssid.to_string(),
                _ => String::new(),
            };
            status.set_network(format!("Wi-Fi: {ssid}, IP {ip}"));
        }
        Err(error) => log::warn!("wifi connect: {error}"),
    }
    Ok(result?)
}

fn start_setup(wifi: &mut Wifi, configuration: Configuration, status: &Status) -> anyhow::Result<()> {
    if wifi.is_started()? {
        wifi.stop()?;
    }
    wifi.set_configuration(&configuration)?;
    wifi.start()?;
    wifi.wait_netif_up()?;
    let ip = wifi.wifi().ap_netif().get_ip_info()?.ip;
    status.set_network(format!(
        "Режим налаштування: мережа «{SETUP_SSID}», сторінка http://{ip}/"
    ));
    spawn_dns(ip.octets())
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

fn client(config: &Config) -> anyhow::Result<ClientConfiguration> {
    let auth_method = if config.wifi_password().is_empty() {
        AuthMethod::None
    } else {
        AuthMethod::WPA2Personal
    };
    Ok(ClientConfiguration {
        ssid: config.ssid().try_into().map_err(|_| anyhow::anyhow!("ssid"))?,
        password: config.wifi_password().try_into().map_err(|_| anyhow::anyhow!("password"))?,
        auth_method,
        ..Default::default()
    })
}
