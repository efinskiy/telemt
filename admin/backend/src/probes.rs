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

/// Endpoint evidence state; unknown observations must not trigger failover.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub(crate) enum Health {Healthy,Recovered,Failed,Unknown}
// The scheduler uses 15-second rounds; twenty completed rounds cover the five-minute window.
const FAILURE_WINDOW_SECONDS:i64=300;
const FAILURE_WINDOW_ROUNDS:usize=20;
const FAILURE_THRESHOLD:usize=10;
const RESERVE_SUCCESS_ROUNDS:usize=2;
const RECOVERY_SUCCESS_ROUNDS:usize=6;

/// Count distinct failures per agent, with current evidence required before any decision.
fn classify(observations:&[(Uuid,i64,Option<bool>)],last:i64)->Health {
    let mut agents:std::collections::HashMap<Uuid,[Option<bool>;FAILURE_WINDOW_ROUNDS]>=std::collections::HashMap::new();
    for &(agent,round,result) in observations {
        let Some(offset)=last.checked_sub(round).and_then(|offset|usize::try_from(offset).ok()).filter(|offset|*offset<FAILURE_WINDOW_ROUNDS)else{continue;};
        agents.entry(agent).or_insert([None;FAILURE_WINDOW_ROUNDS])[offset]=result;
    }
    if agents.values().any(|rounds|rounds[0]==Some(false)&&rounds.iter().filter(|value|**value==Some(false)).count()>=FAILURE_THRESHOLD){return Health::Failed;}
    if agents.values().any(|rounds|rounds[..RECOVERY_SUCCESS_ROUNDS].iter().all(|value|*value==Some(true))){return Health::Recovered;}
    if agents.values().any(|rounds|rounds[..RESERVE_SUCCESS_ROUNDS].iter().all(|value|*value==Some(true))){return Health::Healthy;}
    Health::Unknown
}

/// Shared bounded evidence for node decisions, local IP rotation and transport diagnostics.
pub(crate) async fn evidence(state:&State,node:Uuid,ip:&str)->anyhow::Result<[Health;3]> {
    let last=state.probe_round()-1;
    let rows=sqlx::query("SELECT j.round,j.agent_id,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.address=$2 AND j.round BETWEEN $3 AND $4 AND j.completed_at>now()-make_interval(secs => $5) AND a.enabled AND a.country='RU'")
        .bind(node).bind(ip).bind(last-FAILURE_WINDOW_ROUNDS as i64+1).bind(last).bind(FAILURE_WINDOW_SECONDS as f64).fetch_all(&state.db).await?;
    let mut observations=[Vec::new(),Vec::new(),Vec::new()];
    for row in rows {
        let result:Value=row.get("result");let agent:Uuid=row.get("agent_id");let round:i64=row.get("round");
        let native=result["native"]["reachable"].as_bool();let web=result["web"]["reachable"].as_bool();
        // An unavailable reference or incomplete transport result is never a server failure.
        let values=if result["network_ok"]==true {match (native,web){(Some(native),Some(web))=>[Some(native&&web),Some(native),Some(web)],_=>[None;3]}}else{[None;3]};
        for (index,value) in values.into_iter().enumerate(){observations[index].push((agent,round,value));}
    }
    Ok(observations.map(|values|classify(&values,last)))
}
pub(crate) async fn health(state:&State,node:Uuid,family:i32)->anyhow::Result<Health> {
    let last=state.probe_round()-1;
    let document:Value=sqlx::query_scalar("SELECT document FROM nodes WHERE id=$1").bind(node).fetch_one(&state.db).await?;
    let ip=document[if family==4{"ipv4"}else{"ipv6"}].as_str().unwrap_or("");
    let calculated=evidence(state,node,ip).await?[0];
    let mut tx=state.db.begin().await?;
    sqlx::query("INSERT INTO node_health(node_id,family) VALUES($1,$2) ON CONFLICT DO NOTHING").bind(node).bind(family).execute(&mut *tx).await?;
    let previous=sqlx::query("SELECT status FROM node_health WHERE node_id=$1 AND family=$2 FOR UPDATE").bind(node).bind(family).fetch_one(&mut *tx).await?.get::<String,_>("status");
    let quarantined=["failed","recovering"].contains(&previous.as_str());
    let result=if quarantined && calculated!=Health::Recovered && calculated!=Health::Failed{Health::Unknown}else{calculated};
    let status=match result{Health::Failed=>"failed",Health::Recovered=>"recovered",Health::Healthy=>"healthy",Health::Unknown if quarantined=>"recovering",_=>"unknown"};
    sqlx::query("UPDATE node_health SET status=$3,last_round=$4 WHERE node_id=$1 AND family=$2").bind(node).bind(family).bind(status).bind(last).execute(&mut *tx).await?;
    tx.commit().await?;Ok(result)
}
#[cfg(test)]mod tests {
    use super::*;
    fn samples(agent:Uuid,count:i64,good:Option<bool>)->Vec<(Uuid,i64,Option<bool>)>{(0..count).map(|offset|(agent,100-offset,good)).collect()}
    #[test]fn expired_agent_jobs_remain_unknown_even_with_a_good_reference(){assert!(!known_network(true,"expired"));assert!(!known_network(true,"network_unknown"));assert!(!known_network(false,"tcp"));assert!(known_network(true,"timeout"));}
    #[test]fn failure_requires_ten_distinct_rounds_from_one_current_agent(){
        let agent=Uuid::from_u128(1);let mut rows=samples(agent,9,Some(false));
        assert_eq!(classify(&rows,100),Health::Unknown);
        rows.extend(samples(Uuid::from_u128(2),9,Some(false)));assert_eq!(classify(&rows,100),Health::Unknown);
        rows.extend(samples(agent,9,Some(false)));assert_eq!(classify(&rows,100),Health::Unknown);
        rows.push((agent,80,Some(false)));assert_eq!(classify(&rows,100),Health::Unknown);
        rows.push((agent,81,Some(false)));assert_eq!(classify(&rows,100),Health::Failed);
        assert_eq!(classify(&rows,101),Health::Unknown);
        rows.push((agent,100,None));assert_eq!(classify(&rows,100),Health::Unknown);
    }
    #[test]fn sustained_success_and_failure_have_distinct_thresholds(){
        let agent=Uuid::from_u128(1);
        assert_eq!(classify(&samples(agent,10,Some(false)),100),Health::Failed);
        assert_eq!(classify(&samples(agent,2,Some(true)),100),Health::Healthy);
        assert_eq!(classify(&samples(agent,6,Some(true)),100),Health::Recovered);
        let mut rows:Vec<_>=(0..20).map(|offset|(agent,100-offset,Some(offset%2!=0))).collect();
        assert_eq!(classify(&rows,100),Health::Failed);
        rows.extend(samples(Uuid::from_u128(2),6,Some(true)));assert_eq!(classify(&rows,100),Health::Failed);
        rows.push((agent,100,Some(true)));assert_eq!(classify(&rows,100),Health::Recovered);
    }
}
