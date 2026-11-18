//! The setup access point: its interface, the captive portal hints and the
//! DNS responder that sends every name to the device.

use std::ffi::CString;
use std::net::UdpSocket;
use std::sync::OnceLock;

use esp_idf_svc::handle::RawHandle;
use esp_idf_svc::ipv4::{self, Ipv4Addr};
use esp_idf_svc::netif::{EspNetif, NetifConfiguration};
use esp_idf_svc::sys::{
    esp, esp_netif_dhcp_option_id_t_ESP_NETIF_CAPTIVEPORTAL_URI, esp_netif_dhcp_option_mode_t_ESP_NETIF_OP_SET,
    esp_netif_dhcps_option, esp_netif_dhcps_start, esp_netif_dhcps_stop, esp_wifi_get_config, esp_wifi_set_config,
    wifi_config_t, wifi_interface_t_WIFI_IF_AP, ESP_ERR_ESP_NETIF_DHCP_ALREADY_STOPPED,
};
use esp_idf_svc::wifi::{AccessPointConfiguration, AuthMethod};
use power_core::dns;
use power_core::portal::{portal_url, SETUP_IP};

use super::SETUP_SSID;

/// When the station joins a network on another channel, the access point has
/// to follow it. With a Channel Switch Announcement phones move along instead
/// of dropping off (and closing the portal). esp-idf-svc leaves the count at
/// zero, so the IDF defaults are restored before the access point starts.
pub(super) fn announce_channel_switches() -> anyhow::Result<()> {
    let mut conf = wifi_config_t::default();
    // SAFETY: the driver is initialised and stopped; the access point view of
    // the union is the one read and written for WIFI_IF_AP.
    unsafe {
        esp!(esp_wifi_get_config(wifi_interface_t_WIFI_IF_AP, &mut conf))?;
        conf.ap.csa_count = 3;
        conf.ap.dtim_period = 1;
        esp!(esp_wifi_set_config(wifi_interface_t_WIFI_IF_AP, &mut conf))?;
    }
    Ok(())
}

/// The setup network's interface. Its DHCP server must name the device itself
/// as DNS server (the library default is 8.8.8.8, unreachable here), otherwise
/// phones never ask our DNS responder and the portal does not open.
pub(super) fn setup_ap_netif() -> anyhow::Result<EspNetif> {
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
pub(super) fn offer_portal_uri(netif: &EspNetif) -> anyhow::Result<()> {
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
pub(super) fn spawn_dns(ip: [u8; 4]) -> anyhow::Result<()> {
    let socket = UdpSocket::bind("0.0.0.0:53")?;
    crate::tasks::spawn("dns", 4096, move || {
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

pub(super) fn setup_ap(channel: u8) -> AccessPointConfiguration {
    AccessPointConfiguration {
        ssid: SETUP_SSID.try_into().unwrap(),
        channel,
        auth_method: AuthMethod::None,
        max_connections: 4,
        ..Default::default()
    }
}
