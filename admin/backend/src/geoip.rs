//! Offline city lookup; client addresses never leave the Fleet control plane.
use std::{net::IpAddr, sync::Arc};
use maxminddb::{Reader, geoip2};
use serde::Serialize;

const MAX_DATABASE_BYTES: usize = 256 * 1024 * 1024;

/// Immutable database shared by bounded blocking lookup workers.
pub(crate) struct GeoIp {
    reader: Option<Reader<Vec<u8>>>,
    /// Missing or invalid data is reported separately from unknown client coordinates.
    pub status: &'static str,
    /// A cancelled HTTP request cannot release capacity while its blocking worker still runs.
    pub slots: Arc<tokio::sync::Semaphore>,
}

/// Approximate network location, never a physical subscriber address.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct Location {
    /// Latitude in degrees, validated before serialization.
    pub latitude: f64,
    /// Longitude in degrees, validated before serialization.
    pub longitude: f64,
    /// Russian name when supplied by the database, otherwise English.
    pub country: String,
    /// Empty when the database has no city-level name.
    pub city: String,
}

impl GeoIp {
    /// Missing or invalid optional data must not prevent administration startup.
    pub async fn load(path: Option<std::path::PathBuf>) -> Self {
        let Some(path) = path else { return Self::unavailable("missing"); };
        let bytes = match crate::bounded_file(&path, MAX_DATABASE_BYTES).await {
            Ok(bytes) => bytes,
            Err(error) => {
                let status = if error.downcast_ref::<std::io::Error>().is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) { "missing" } else { "invalid" };
                tracing::warn!(status, "connection map database unavailable");
                return Self::unavailable(status);
            }
        };
        match tokio::task::spawn_blocking(move || Reader::from_source(bytes)).await {
            Ok(Ok(reader)) if reader.metadata.database_type.to_ascii_lowercase().contains("city") => Self { reader: Some(reader), status: "ready", slots: Arc::new(tokio::sync::Semaphore::new(4)) },
            _ => Self::unavailable("invalid"),
        }
    }

    /// Constructs the explicit no-database state used by optional deployments and tests.
    pub fn unavailable(status: &'static str) -> Self {
        Self { reader: None, status, slots: Arc::new(tokio::sync::Semaphore::new(4)) }
    }

    /// Returns the database release timestamp without disclosing its filesystem location.
    pub fn build_epoch(&self) -> Option<u64> { self.reader.as_ref().map(|reader| reader.metadata.build_epoch) }

    /// Rejects non-public ranges even if a supplied database gives them coordinates.
    pub fn locate(&self, ip: IpAddr) -> Option<Location> {
        if !public_ip(ip) { return None; }
        let record: geoip2::City<'_> = self.reader.as_ref()?.lookup(ip).ok()??;
        let location = record.location?;
        let latitude = location.latitude?; let longitude = location.longitude?;
        if !latitude.is_finite() || !longitude.is_finite() || !(-90.0..=90.0).contains(&latitude) || !(-180.0..=180.0).contains(&longitude) { return None; }
        let name = |names: Option<std::collections::BTreeMap<&str, &str>>| names.and_then(|names| names.get("ru").or_else(|| names.get("en")).map(|text| text.chars().take(128).collect::<String>())).unwrap_or_default();
        Some(Location { latitude, longitude, country: name(record.country.and_then(|v| v.names)), city: name(record.city.and_then(|v| v.names)) })
    }
}

/// Excludes private, shared, documentation and other non-geographic address space.
pub(crate) fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a,b,c,_] = ip.octets();
            !(ip.is_private() || ip.is_loopback() || ip.is_link_local() || ip.is_documentation() || a == 0 || a >= 224
                || (a == 100 && (64..=127).contains(&b)) || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0) || (a == 192 && b == 88 && c == 99))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() { return public_ip(IpAddr::V4(v4)); }
            let s = ip.segments();
            // Restrict to global unicast and omit documentation, benchmarking and transition ranges.
            (s[0] & 0xe000) == 0x2000 && !(s[0] == 0x2001 && (s[1] <= 0x1ff || s[1] == 0xdb8)) && s[0] != 0x2002 && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn non_public_addresses_are_never_plotted() {
        for ip in ["127.0.0.1", "10.0.0.1", "100.64.0.1", "169.254.1.1", "192.0.2.1", "198.19.0.1", "224.0.0.1", "::1", "fc00::1", "fe80::1", "2001:db8::1", "::ffff:192.168.1.1"] { assert!(!public_ip(ip.parse().unwrap()), "{ip}"); }
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] { assert!(public_ip(ip.parse().unwrap()), "{ip}"); }
        assert!(GeoIp::unavailable("missing").locate("1.1.1.1".parse().unwrap()).is_none());
    }
    #[tokio::test]
    #[ignore = "requires TELEMT_TEST_GEOIP_DATABASE pointing to a real city MMDB"]
    async fn installed_city_database_locates_both_address_families() {
        let path = std::env::var_os("TELEMT_TEST_GEOIP_DATABASE").expect("test database path required");
        let geo = GeoIp::load(Some(path.into())).await;
        assert_eq!(geo.status, "ready");
        assert!(geo.build_epoch().is_some());
        for ip in ["1.1.1.1", "8.8.8.8", "2606:4700:4700::1111"] {
            let location = geo.locate(ip.parse().unwrap()).expect("public address location");
            assert!(!location.country.is_empty());
            assert!((-90.0..=90.0).contains(&location.latitude));
        }
        assert!(geo.locate("10.0.0.1".parse().unwrap()).is_none());
    }
    #[tokio::test]
    async fn missing_database_is_optional() {
        let geo = GeoIp::load(None).await;
        assert_eq!(geo.status, "missing"); assert_eq!(geo.build_epoch(), None);
    }
}
