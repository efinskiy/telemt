use super::*;
use crate::probes::{classify,Health};

pub(super) async fn inventory(state:&State,node_id:Uuid)->Result<Value> {
    let (status,response)=node_request(state,node_id,reqwest::Method::GET,"/v1/runtime/interfaces",None,None).await?;
    if status!=200||response["data"]["addresses"].as_array().is_none(){return Err(conflict("assigned_address_inventory_unavailable"));}
    let mut data=response["data"].clone();data["revision"]=response["revision"].clone();Ok(data)
}
pub(super) fn addresses(value:&Value,inventory:&Value)->Result<Vec<String>> {
    let selected=value.as_array().ok_or_else(bad)?;if !(2..=8).contains(&selected.len()){return Err(bad());}
    let assigned=inventory["addresses"].as_array().ok_or_else(bad)?;let mut unique=std::collections::HashSet::new();let mut result=Vec::new();
    for value in selected {let ip:std::net::IpAddr=value.as_str().ok_or_else(bad)?.parse().map_err(|_|bad())?;let ip=ip.to_string();if !unique.insert(ip.clone())||!assigned.iter().any(|entry|entry["ip"]==ip&&entry["usable"]==true){return Err(conflict("pool_address_not_assigned_or_usable"));}result.push(ip);}
    Ok(result)
}
fn endpoint(listener:&Value,ip:&str,port:u64)->bool {
    listener["ip"]==ip&&listener["port"].as_u64().unwrap_or(port)==port&&listener["transport"].as_str().unwrap_or("mtproxy")=="mtproxy"
}
pub(super) fn has_listener(inventory:&Value,ip:&str,port:u64)->bool {inventory["active_listeners"].as_array().is_some_and(|listeners|listeners.iter().any(|listener|endpoint(listener,ip,port)))}
/// A wildcard cannot coexist with staged specific sockets without changing SO_REUSEPORT semantics.
pub(super) fn listener_plan(listeners:&Value,selected:&Value,document:&Value)->Result<Value> {
    let listeners=listeners.as_array().ok_or_else(bad)?;let selected=selected.as_array().ok_or_else(bad)?;let port=document["port"].as_u64().ok_or_else(bad)?;
    if listeners.len()>128||listeners.iter().any(|listener|listener["synlimit"].as_str().is_some_and(|mode|mode!="off")){return Err(conflict("synlimited_listener_rebind_requires_deployment_change"));}
    let mut result=listeners.clone();
    for ip in selected {
        let ip=ip.as_str().ok_or_else(bad)?;let family=ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4();
        if listeners.iter().any(|listener|listener["port"].as_u64().unwrap_or(port)==port&&listener["ip"].as_str().and_then(|s|s.parse::<std::net::IpAddr>().ok()).is_some_and(|bound|bound.is_unspecified()&&bound.is_ipv4()==family)){return Err(conflict("wildcard_listener_requires_explicit_ip_binding"));}
        if result.iter().any(|listener|endpoint(listener,ip,port)){continue;}
        let anchor=document[if family{"ipv4"}else{"ipv6"}].as_str().ok_or_else(bad)?;
        let mut listener=listeners.iter().find(|listener|endpoint(listener,anchor,port)).cloned().ok_or(conflict("active_mtproxy_listener_not_found"))?;
        listener["ip"]=json!(ip);result.push(listener);
    }
    if let Some(domain)=document["announce_domain"].as_str() {for listener in &mut result {if selected.iter().any(|ip|ip.as_str().is_some_and(|ip|endpoint(listener,ip,port))){listener["announce"]=json!(domain);listener["announce_ip"]=Value::Null;}}}
    if result.is_empty()||result.len()>128{return Err(bad());}Ok(json!(result))
}

pub(super) async fn health(state:&State,node_id:Uuid,ip:&str)->Result<Health> {
    let last=state.probe_round()-1;
    let rows=sqlx::query("SELECT j.round,a.network,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.address=$2 AND j.round BETWEEN $3-5 AND $3 AND j.completed_at IS NOT NULL AND a.enabled AND a.country='RU'").bind(node_id).bind(ip).bind(last).fetch_all(&state.db).await?;
    let mut networks:std::collections::HashMap<(i64,String),Vec<bool>>=std::collections::HashMap::new();
    for row in rows {let result:Value=row.get("result");if result["network_ok"]==true && result["native"]["reachable"].is_boolean() && result["web"]["reachable"].is_boolean() && let Some(good)=result["reachable"].as_bool(){networks.entry((row.get("round"),row.get("network"))).or_default().push(good);}}
    let rounds:Vec<_>=(0..6).map(|offset|{let values:Vec<_>=networks.iter().filter(|((round,_),_)|*round==last-offset).map(|(_,values)|values).collect();(values.iter().filter(|values|values.iter().all(|good|*good)).count(),values.iter().filter(|values|values.iter().all(|good|!*good)).count())}).collect();
    let current=classify(&rounds,true);let mut tx=state.db.begin().await?;
    sqlx::query("INSERT INTO address_health(node_id,address) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(node_id).bind(ip).execute(&mut *tx).await?;
    let previous:String=sqlx::query_scalar("SELECT status FROM address_health WHERE node_id=$1 AND address=$2 FOR UPDATE").bind(node_id).bind(ip).fetch_one(&mut *tx).await?;
    let quarantined=["failed","recovering"].contains(&previous.as_str());let current=if quarantined&&!matches!(current,Health::Failed|Health::Recovered){Health::Unknown}else{current};
    let status=match current{Health::Failed=>"failed",Health::Healthy=>"healthy",Health::Recovered=>"recovered",Health::Unknown if quarantined=>"recovering",_=>"unknown"};
    sqlx::query("UPDATE address_health SET status=$3 WHERE node_id=$1 AND address=$2").bind(node_id).bind(ip).bind(status).execute(&mut *tx).await?;tx.commit().await?;Ok(current)
}

/// Display independent quorum evidence for each transport, with the same freshness window.
pub(super) async fn port_health(state:&State,node:Uuid,ip:&str)->Result<[String;2]> {
    let last=state.probe_round()-1;
    let rows=sqlx::query("SELECT j.round,a.network,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.address=$2 AND j.round BETWEEN $3-5 AND $3 AND j.completed_at IS NOT NULL AND a.enabled AND a.country='RU'").bind(node).bind(ip).bind(last).fetch_all(&state.db).await?;
    let mut output=[String::new(),String::new()];
    for (index,key) in ["native","web"].iter().enumerate(){
        let mut networks:std::collections::HashMap<(i64,String),Vec<bool>>=std::collections::HashMap::new();
        for row in &rows {let result:Value=row.get("result");if result["network_ok"]==true && let Some(good)=result[*key]["reachable"].as_bool(){networks.entry((row.get("round"),row.get("network"))).or_default().push(good);}}
        let rounds:Vec<_>=(0..6).map(|offset|{let values:Vec<_>=networks.iter().filter(|((round,_),_)|*round==last-offset).map(|(_,values)|values).collect();(values.iter().filter(|v|v.iter().all(|x|*x)).count(),values.iter().filter(|v|v.iter().all(|x|!*x)).count())}).collect();
        output[index]=format!("{:?}",classify(&rounds,true)).to_ascii_lowercase();
    }Ok(output)
}

#[cfg(test)]mod tests {
    use super::*;
    #[test]fn pool_rejects_unassigned_and_duplicate_addresses(){let inventory=json!({"addresses":[{"ip":"192.0.2.1","usable":true},{"ip":"192.0.2.2","usable":true},{"ip":"127.0.0.1","usable":false}]});assert!(addresses(&json!(["192.0.2.1","192.0.2.2"]),&inventory).is_ok());assert!(addresses(&json!(["192.0.2.1","192.0.2.1"]),&inventory).is_err());assert!(addresses(&json!(["192.0.2.1","192.0.2.3"]),&inventory).is_err());assert!(addresses(&json!(["192.0.2.1","127.0.0.1"]),&inventory).is_err());}
    #[test]fn preparation_preserves_policy_and_existing_endpoints(){let listeners=json!([{"ip":"192.0.2.1","port":443,"transport":"mtproxy","proxy_protocol":false,"synlimit":"off"},{"ip":"127.0.0.1","port":8080,"transport":"web"}]);let doc=json!({"port":443,"ipv4":"192.0.2.1","sni":"example.org"});let plan=listener_plan(&listeners,&json!(["192.0.2.1","192.0.2.2"]),&doc).unwrap();assert_eq!(plan.as_array().unwrap().len(),3);assert_eq!(plan[2]["proxy_protocol"],false);assert!(has_listener(&json!({"active_listeners":plan}),"192.0.2.1",443));assert!(has_listener(&json!({"active_listeners":plan}),"192.0.2.2",443));}
    #[test]fn wildcard_and_synlimit_require_deployment_preparation(){let doc=json!({"port":443,"ipv4":"192.0.2.1"});for listener in [json!({"ip":"0.0.0.0","port":443,"transport":"mtproxy"}),json!({"ip":"192.0.2.1","port":443,"synlimit":"iptables"})]{assert!(listener_plan(&json!([listener]),&json!(["192.0.2.1","192.0.2.2"]),&doc).is_err());}}
}
