//! Endpoint-scoped evidence is the only authority for exhausting a server's pool.
use super::*;
use crate::probes::Health;

/// Every group member must share the same managed profile and credential source.
pub(crate) async fn same_profile(state:&State,group:&sqlx::postgres::PgRow,target:Uuid)->Result<()> {
    let document:Value=group.get("document");
    let profile=document["profile_id"].as_str().and_then(|id|Uuid::parse_str(id).ok()).ok_or(conflict("reserve_requires_same_profile"))?;
    let same:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM managed_nodes WHERE node_id=$1 AND profile_id=$2 AND NOT revoked)").bind(target).bind(profile).fetch_one(&state.db).await?;
    if !same||!document["node_ids"].as_array().is_some_and(|ids|ids.contains(&json!(target))){return Err(conflict("reserve_requires_same_profile"));}Ok(())
}

async fn endpoints(state:&State,node_id:Uuid)->Result<Vec<String>> {
    let pool:Option<Value>=sqlx::query_scalar("SELECT addresses FROM ip_pools WHERE node_id=$1").bind(node_id).fetch_optional(&state.db).await?;
    if let Some(pool)=pool{return serde_json::from_value(pool).map_err(|_|bad());}
    let doc:Value=node(state,node_id).await?.get("document");
    Ok(["ipv4","ipv6"].iter().filter_map(|key|doc[*key].as_str().filter(|s|!s.is_empty()).map(str::to_owned)).collect())
}

/// Unknown, incomplete and recovering evidence cannot trigger migration to another server.
pub(crate) async fn exhausted(state:&State,node_id:Uuid)->Result<bool> {
    let addresses=endpoints(state,node_id).await?;
    if addresses.is_empty(){return Ok(false);}
    for address in addresses {if health(state,node_id,&address).await?!=Health::Failed{return Ok(false);}}
    Ok(true)
}

/// Pick a prepared endpoint of the requested family; never infer success from a sibling address.
pub(crate) async fn candidate(state:&State,node_id:Uuid,ipv4:bool)->Result<String> {
    if let Some(pool)=sqlx::query("SELECT * FROM ip_pools WHERE node_id=$1").bind(node_id).fetch_optional(&state.db).await? {workflow::ready(state,&pool).await?;}
    let doc:Value=node(state,node_id).await?.get("document");let current=doc[if ipv4{"ipv4"}else{"ipv6"}].as_str();
    let mut addresses=endpoints(state,node_id).await?;
    addresses.sort_by_key(|ip|Some(ip.as_str())!=current);
    for ip in addresses {if ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4()==ipv4 && matches!(health(state,node_id,&ip).await?,Health::Healthy|Health::Recovered){return Ok(ip);}}
    Err(conflict("reserve_without_probe_quorum"))
}

/// Owner-forced DNS uses registered addresses even while the node is offline or unsynchronized.
pub(crate) async fn configured(state:&State,node_id:Uuid,ipv4:bool)->Result<String> {
    let doc:Value=node(state,node_id).await?.get("document");
    let ip=doc[if ipv4{"ipv4"}else{"ipv6"}].as_str().ok_or(conflict("reserve_ip_not_prepared"))?;
    let address:std::net::IpAddr=ip.parse().map_err(|_|conflict("node_address_invalid"))?;
    if address.is_ipv4()!=ipv4||address.is_unspecified()||address.is_multicast()||address.is_loopback()||matches!(address,std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()){return Err(conflict("node_address_invalid"));}
    Ok(ip.to_owned())
}
