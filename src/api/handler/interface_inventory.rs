//! Assigned addresses are read on demand; the controller never creates OS addresses.
use super::*;

pub(super) fn collect(cfg: &ProxyConfig, generation: u64) -> Result<serde_json::Value, ApiFailure> {
    let mut addresses = std::collections::BTreeMap::<(std::net::IpAddr, String), serde_json::Value>::new();
    #[cfg(unix)]
    {
        let interfaces = nix::ifaddrs::getifaddrs().map_err(|_| ApiFailure::new(
            StatusCode::SERVICE_UNAVAILABLE, "interface_inventory_unavailable", "Cannot enumerate assigned interface addresses",
        ))?;
        for (index, interface) in interfaces.enumerate() {
            if index >= 4096 { return Err(ApiFailure::new(StatusCode::SERVICE_UNAVAILABLE, "interface_inventory_capacity", "Interface inventory exceeds its bound")); }
            let Some(address) = interface.address else { continue; };
            let ip = if let Some(v4) = address.as_sockaddr_in() { std::net::IpAddr::V4(v4.ip()) }
                else if let Some(v6) = address.as_sockaddr_in6() { std::net::IpAddr::V6(v6.ip()) }
                else { continue; };
            let up = interface.flags.contains(nix::net::if_::InterfaceFlags::IFF_UP);
            let link_local = match ip { std::net::IpAddr::V4(v4) => v4.is_link_local(), std::net::IpAddr::V6(v6) => v6.is_unicast_link_local() };
            let usable = up && !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() && !link_local;
            addresses.insert((ip, interface.interface_name.clone()), serde_json::json!({"ip": ip, "interface": interface.interface_name, "up": up, "usable": usable, "family": if ip.is_ipv4() {4} else {6}}));
            if addresses.len() > 512 { return Err(ApiFailure::new(StatusCode::SERVICE_UNAVAILABLE, "interface_inventory_capacity", "Too many assigned addresses")); }
        }
    }
    Ok(serde_json::json!({"addresses": addresses.into_values().collect::<Vec<_>>(), "active_listeners": cfg.server.listeners, "runtime_generation": generation, "legacy_port": cfg.server.port}))
}
