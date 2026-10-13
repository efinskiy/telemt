//! Bounded live map snapshots derived from node admission state and a local city database.
use std::net::IpAddr;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;
use crate::{error::{Result, ApiError}, geoip::{GeoIp, Location, public_ip}, state::{State, node, node_request}};

#[derive(Deserialize)]
struct Client { ip: IpAddr, connections: u64, users: u64 }
#[derive(Deserialize)]
struct Snapshot { ips: Vec<Client>, total_ips: u64, connections: u64, active_users: u64, cleanup_pending: u64, truncated: bool }
#[derive(Serialize)]
struct LocatedClient { ip: IpAddr, connections: u64, users: u64, location: Option<Location> }

/// Authenticated read-only endpoint; only registered enabled nodes can supply addresses.
pub(crate) async fn read(state: &State, id: Uuid) -> Result<Value> {
    let row = node(state, id).await?;
    if !row.get::<bool, _>("enabled") { return Ok(json!({"available":false,"reason":"node_disabled"})); }
    // Bound both upstream requests and CPU workers, including requests abandoned by the browser.
    let permit = state.geoip.slots.clone().try_acquire_owned().map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "map_busy"))?;
    let (status, response) = node_request(state, id, reqwest::Method::GET, "/v1/runtime/clients", None, None).await?;
    if status == 404 { return Ok(json!({"available":false,"reason":"node_update_required"})); }
    if status != 200 || response["ok"] != true { return Err(ApiError(StatusCode::BAD_GATEWAY, "map_node_unavailable")); }
    let snapshot: Snapshot = serde_json::from_value(response["data"]["snapshot"].clone()).map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "map_invalid_snapshot"))?;
    validate(&snapshot).map_err(|_| ApiError(StatusCode::BAD_GATEWAY, "map_invalid_snapshot"))?;
    let document: Value = row.get("document"); let geoip = state.geoip.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        render(&geoip, snapshot, &document)
    }).await.map_err(|_| ApiError(StatusCode::SERVICE_UNAVAILABLE, "map_unavailable"))
}

fn validate(snapshot: &Snapshot) -> std::result::Result<(), ()> {
    let mut ips = std::collections::HashSet::new();
    if snapshot.ips.len() > 10_000 || snapshot.total_ips < snapshot.ips.len() as u64 || (!snapshot.truncated && snapshot.total_ips != snapshot.ips.len() as u64) { return Err(()); }
    let mut connections = 0u64;
    for row in &snapshot.ips {
        if row.connections == 0 || row.users == 0 || row.users > row.connections || row.users > snapshot.active_users || !ips.insert(row.ip) { return Err(()); }
        connections = connections.checked_add(row.connections).ok_or(())?;
    }
    if connections > snapshot.connections || (!snapshot.truncated && connections != snapshot.connections) { return Err(()); }
    Ok(())
}

fn render(geoip: &GeoIp, snapshot: Snapshot, document: &Value) -> Value {
    let mut located_ips = 0usize; let mut non_public_ips = 0usize;
    let ips: Vec<_> = snapshot.ips.into_iter().map(|row| {
        let location = geoip.locate(row.ip);
        if location.is_some() { located_ips += 1; }
        if !public_ip(row.ip) { non_public_ips += 1; }
        LocatedClient { ip: row.ip, connections: row.connections, users: row.users, location }
    }).collect();
    let endpoints: Vec<_> = ["ipv4", "ipv6"].iter().filter_map(|key| document[*key].as_str()?.parse::<IpAddr>().ok()).map(|ip| json!({"ip":ip,"location":geoip.locate(ip)})).collect();
    json!({"available":true,"generated_at":chrono::Utc::now().to_rfc3339(),"database":{"status":geoip.status,"build_epoch":geoip.build_epoch()},
        "ips":ips,"endpoints":endpoints,"total_ips":snapshot.total_ips,"connections":snapshot.connections,"active_users":snapshot.active_users,
        "cleanup_pending":snapshot.cleanup_pending,"truncated":snapshot.truncated,"located_ips":located_ips,"non_public_ips":non_public_ips})
}

#[cfg(test)]
mod tests {
    use super::*;
    fn snapshot() -> Snapshot { Snapshot { ips: vec![Client { ip: "1.1.1.1".parse().unwrap(), connections: 3, users: 2 }], total_ips: 1, connections: 3, active_users: 2, cleanup_pending: 0, truncated: false } }
    #[test]
    fn rejects_inconsistent_or_duplicate_snapshots() {
        assert!(validate(&snapshot()).is_ok());
        let mut s = snapshot(); s.connections = 0; assert!(validate(&s).is_err());
        let mut s = snapshot(); s.ips.push(Client { ip: s.ips[0].ip, connections: 1, users: 1 }); s.total_ips = 2; s.connections = 4; assert!(validate(&s).is_err());
        let mut s = snapshot(); s.ips[0].users = 4; assert!(validate(&s).is_err());
        let mut s = snapshot(); s.total_ips = 2; assert!(validate(&s).is_err()); s.truncated = true; assert!(validate(&s).is_ok());
    }
    #[test]
    fn missing_database_preserves_live_counts_without_inventing_coordinates() {
        let value = render(&GeoIp::unavailable("missing"), snapshot(), &json!({"ipv4":"1.0.0.1"}));
        assert_eq!(value["connections"], 3); assert_eq!(value["located_ips"], 0);
        assert!(value["ips"][0]["location"].is_null()); assert!(value["endpoints"][0]["location"].is_null());
    }
}
