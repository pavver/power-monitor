//! Captive portal: the setup access point's address and where stray
//! requests are sent so the phone's connectivity check lands on the page.

/// Address of the setup access point; also its DHCP-advertised DNS server.
pub const SETUP_IP: [u8; 4] = [192, 168, 71, 1];

/// The page on the setup access point.
pub fn portal_url() -> String {
    let [a, b, c, d] = SETUP_IP;
    format!("http://{a}.{b}.{c}.{d}/")
}

/// `Location` for a request to an unknown path. Phones probe foreign hosts
/// (connectivitycheck.gstatic.com, captive.apple.com) that our DNS pointed
/// here; those get the absolute portal URL so the portal opens under our
/// address. Requests addressed to us by IP keep their host.
pub fn redirect_target(host: Option<&str>) -> String {
    let name = host.map(|h| h.split(':').next().unwrap_or(h));
    match name {
        Some(name) if name.parse::<std::net::Ipv4Addr>().is_err() => portal_url(),
        _ => "/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portal_url_uses_the_setup_address() {
        assert_eq!(portal_url(), "http://192.168.71.1/");
    }

    #[test]
    fn connectivity_checks_go_to_the_portal() {
        for host in ["connectivitycheck.gstatic.com", "captive.apple.com", "www.msftconnecttest.com:80"] {
            assert_eq!(redirect_target(Some(host)), "http://192.168.71.1/", "{host}");
        }
    }

    #[test]
    fn requests_to_our_address_stay_on_it() {
        for host in ["192.168.71.1", "192.168.1.50", "192.168.1.50:80"] {
            assert_eq!(redirect_target(Some(host)), "/", "{host}");
        }
        assert_eq!(redirect_target(None), "/");
    }
}
