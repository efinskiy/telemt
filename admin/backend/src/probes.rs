//! Expiring, one-use probe assignments; reported network identity is owned by the controller.
use axum::{extract::State as ExtractState,http::{HeaderMap,StatusCode},Json};
use chrono::Utc;
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{crypto,error::{Result,ApiError,bad},state::{State,field,parse_id,ok}};

async fn agent(state:&State,headers:&HeaderMap)->Result<Uuid> {
    let token=headers.get("authorization").and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).ok_or(ApiError(StatusCode::UNAUTHORIZED,"agent_auth_required"))?;
    if token.len()!=64{return Err(bad());}
    let id=sqlx::query_scalar("SELECT id FROM agents WHERE token_hash=$1 AND enabled").bind(crypto::hash(token)).fetch_optional(&state.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"agent_auth_required"))?;
    sqlx::query("UPDATE agents SET last_seen=now() WHERE id=$1").bind(id).execute(&state.db).await?;Ok(id)
}
pub(crate) async fn jobs(ExtractState(state):ExtractState<State>,headers:HeaderMap)->Result<Json<Value>> {
    let agent=agent(&state,&headers).await?;
    let rows=sqlx::query("UPDATE probe_jobs SET claimed_at=now() WHERE id IN (SELECT id FROM probe_jobs WHERE agent_id=$1 AND claimed_at IS NULL AND expires_at>now() ORDER BY issued_at LIMIT 200 FOR UPDATE SKIP LOCKED) RETURNING id,node_id,family,round,expires_at,address")
        .bind(agent).fetch_all(&state.db).await?;
    let enrolled=sqlx::query("SELECT id,document,probe_cipher FROM nodes WHERE enabled LIMIT 20").fetch_all(&state.db).await?;
    let enrolled:std::collections::HashMap<Uuid,_>=enrolled.into_iter().map(|node|(node.get("id"),node)).collect();
    let mut jobs=Vec::new();
    for row in rows {let node:Uuid=row.get("node_id");let n=enrolled.get(&node);
        let Some(n)=n else{continue;};let document:Value=n.get("document");let family:i32=row.get("family");let address:String=row.try_get("address")?;
        let secret=state.vault.open(&n.get::<String,_>("probe_cipher"),&format!("probe:{node}"))?;
        jobs.push(json!({"id":row.get::<Uuid,_>("id"),"node_id":node,"round":row.get::<i64,_>("round"),"family":family,"ip":address,"port":document["port"],"sni":document["sni"],"dc":document["probe_dc"],"secret":secret.as_str(),"web_base_path":document["web_base_path"],"expires_at":row.get::<chrono::DateTime<Utc>,_>("expires_at")}));
    }Ok(Json(ok(json!(jobs))))
}
pub(crate) async fn result(ExtractState(state):ExtractState<State>,headers:HeaderMap,Json(body):Json<Value>)->Result<Json<Value>> {
    let agent=agent(&state,&headers).await?;let id=parse_id(field(&body,"id")?)?;
    let network_ok=body.get("network_ok").and_then(Value::as_bool).ok_or_else(bad)?;let reachable=body.get("reachable").and_then(Value::as_bool).ok_or_else(bad)?;
    let stage=field(&body,"stage")?;let latency=body.get("latency_ms").and_then(Value::as_u64).ok_or_else(bad)?;if !["ok","network_unknown","tcp","fake_tls","mtproto","timeout","expired","web"].contains(&stage)||latency>5000{return Err(bad());}
    if reachable != (network_ok && stage=="ok") || (!network_ok && stage!="network_unknown" && stage!="expired") {return Err(bad());}
    let known=known_network(network_ok,stage);
    for key in ["native","web"] {
        let port=&body[key];let port_stage=port["stage"].as_str().ok_or_else(bad)?;
        if port["latency_ms"].as_u64().is_none_or(|value|value>5000)
            || !["ok","network_unknown","tcp","fake_tls","mtproto","timeout","expired","web"].contains(&port_stage)
            || (known && (port["reachable"].as_bool().is_none() || (port["reachable"]==true)!=(port_stage=="ok")))
            || (!known && (!port["reachable"].is_null() || !["expired","network_unknown"].contains(&port_stage))) {return Err(bad());}
    }
    if reachable!=(known && body["native"]["reachable"]==true && body["web"]["reachable"]==true){return Err(bad());}
    let document=json!({"web_checked":known,"network_ok":known,"reachable":reachable,"stage":stage,"native":body["native"],"web":body["web"],"latency_ms":latency});
    let row=sqlx::query("UPDATE probe_jobs SET completed_at=now(),result=$3 WHERE id=$1 AND agent_id=$2 AND claimed_at IS NOT NULL AND completed_at IS NULL AND expires_at>now() RETURNING id").bind(id).bind(agent).bind(document).fetch_optional(&state.db).await?;
    if row.is_none(){return Err(ApiError(StatusCode::CONFLICT,"expired_or_replayed_probe"));}Ok(Json(ok(json!({"accepted":true}))))
}
pub(crate) async fn schedule(state:&State)->anyhow::Result<()> {
    let round=state.probe_round();
    // One bounded cross product avoids expiring assignments while issuing thousands of row writes.
    sqlx::query("WITH targets AS (SELECT id AS node_id,4 AS family,document->>'ipv4' AS address FROM nodes WHERE enabled AND coalesce(document->>'ipv4','')<>'' UNION SELECT id,6,document->>'ipv6' FROM nodes WHERE enabled AND coalesce(document->>'ipv6','')<>'' UNION SELECT n.id,CASE WHEN strpos(ip,':')>0 THEN 6 ELSE 4 END,ip FROM ip_pools p JOIN nodes n ON n.id=p.node_id CROSS JOIN LATERAL jsonb_array_elements_text(p.addresses) AS ip WHERE n.enabled) INSERT INTO probe_jobs(id,round,agent_id,node_id,family,address,expires_at) SELECT gen_random_uuid(),$1,a.id,t.node_id,t.family,t.address,now()+interval '14 seconds' FROM targets t CROSS JOIN agents a WHERE a.enabled ON CONFLICT DO NOTHING").bind(round).execute(&state.db).await?;Ok(())
}

// Expired assignments and a failed reference describe the agent, not node reachability.
fn known_network(reference_ok:bool,stage:&str)->bool {reference_ok&&!matches!(stage,"expired"|"network_unknown")}

#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub(crate) enum Health {Healthy,Recovered,Failed,Unknown}
/// Requires complete consecutive rounds, not just two observations from one network.
pub(crate) fn classify(rounds:&[(usize,usize)],complete:bool)->Health {
    if !complete||rounds.is_empty(){return Health::Unknown;}
    if rounds.len()>=3&&rounds[..3].iter().all(|(_,bad)|*bad>=2){return Health::Failed;}
    if rounds.len()>=6&&rounds[..6].iter().all(|(good,_)|*good>=2){return Health::Recovered;}
    if rounds.len()>=2&&rounds[..2].iter().all(|(good,_)|*good>=2){return Health::Healthy;}Health::Unknown
}
pub(crate) async fn health(state:&State,node:Uuid,family:i32)->anyhow::Result<Health> {
    let last=state.probe_round()-1;
    let document:Value=sqlx::query_scalar("SELECT document FROM nodes WHERE id=$1").bind(node).fetch_one(&state.db).await?;
    let ip=document[if family==4{"ipv4"}else{"ipv6"}].as_str().unwrap_or("");
    let rows=sqlx::query("SELECT j.round,a.network,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.family=$2 AND j.round BETWEEN $3-5 AND $3 AND j.address=$4 AND j.completed_at IS NOT NULL AND a.enabled AND a.country='RU' ORDER BY j.round DESC").bind(node).bind(family).bind(last).bind(ip).fetch_all(&state.db).await?;
    let mut networks:std::collections::HashMap<(i64,String),Vec<bool>>=std::collections::HashMap::new();
    for row in rows {let result:Value=row.get("result");if result["network_ok"]!=true||result["native"]["reachable"].as_bool().is_none()||result["web"]["reachable"].as_bool().is_none(){continue;}
        if let Some(reachable)=result["reachable"].as_bool(){networks.entry((row.get("round"),row.get("network"))).or_default().push(reachable);}}
    let rounds:Vec<_>=(0..6).map(|offset|{let values:Vec<_>=networks.iter().filter(|((round,_),_)|*round==last-offset).map(|(_,v)|v).collect();(values.iter().filter(|v|v.iter().all(|x|*x)).count(),values.iter().filter(|v|v.iter().all(|x|!*x)).count())}).collect();
    let calculated=classify(&rounds,true);
    let mut tx=state.db.begin().await?;
    sqlx::query("INSERT INTO node_health(node_id,family) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(node).bind(family).execute(&mut *tx).await?;
    let previous=sqlx::query("SELECT status FROM node_health WHERE node_id=$1 AND family=$2 FOR UPDATE").bind(node).bind(family).fetch_one(&mut *tx).await?.get::<String,_>("status");
    let quarantined=["failed","recovering"].contains(&previous.as_str());
    let result=if quarantined && calculated!=Health::Recovered && calculated!=Health::Failed{Health::Unknown}else{calculated};
    let status=match result{Health::Failed=>"failed",Health::Recovered=>"recovered",Health::Healthy=>"healthy",Health::Unknown if quarantined=>"recovering",_=>"unknown"};
    sqlx::query("UPDATE node_health SET status=$3,last_round=$4 WHERE node_id=$1 AND family=$2").bind(node).bind(family).bind(status).bind(last).execute(&mut *tx).await?;
    tx.commit().await?;Ok(result)
}
#[cfg(test)]mod tests{use super::*;#[test]fn expired_agent_jobs_remain_unknown_even_with_a_good_reference(){assert!(!known_network(true,"expired"));assert!(!known_network(true,"network_unknown"));assert!(!known_network(false,"tcp"));assert!(known_network(true,"timeout"));}#[test]fn missing_quorum_never_marks_failed(){assert_eq!(classify(&[(0,1);6],true),Health::Unknown);assert_eq!(classify(&[(0,2);6],false),Health::Unknown);}#[test]fn sustained_success_and_failure_have_distinct_thresholds(){assert_eq!(classify(&[(0,2);3],true),Health::Failed);assert_eq!(classify(&[(2,0);2],true),Health::Healthy);assert_eq!(classify(&[(2,0);6],true),Health::Recovered);assert_eq!(classify(&[(2,2);6],true),Health::Failed);}}
