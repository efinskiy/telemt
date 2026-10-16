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
/// Issue only registered TCP/TLS port-22 targets without proxy credentials.
pub(crate) async fn jobs(ExtractState(state):ExtractState<State>,headers:HeaderMap)->Result<Json<Value>> {
    let agent=agent(&state,&headers).await?;
    let rows=sqlx::query("UPDATE probe_jobs SET claimed_at=now() WHERE id IN (SELECT id FROM probe_jobs WHERE agent_id=$1 AND claimed_at IS NULL AND expires_at>now() ORDER BY issued_at LIMIT 200 FOR UPDATE SKIP LOCKED) RETURNING id,node_id,family,round,expires_at,address")
        .bind(agent).fetch_all(&state.db).await?;
    let enrolled=sqlx::query("SELECT id,document FROM nodes WHERE enabled LIMIT 20").fetch_all(&state.db).await?;
    let enrolled:std::collections::HashMap<Uuid,_>=enrolled.into_iter().map(|node|(node.get("id"),node)).collect();
    let mut jobs=Vec::new();
    for row in rows {let node:Uuid=row.get("node_id");let n=enrolled.get(&node);
        let Some(n)=n else{continue;};let document:Value=n.get("document");let family:i32=row.get("family");let address:String=row.try_get("address")?;
        jobs.push(json!({"id":row.get::<Uuid,_>("id"),"node_id":node,"round":row.get::<i64,_>("round"),"family":family,"ip":address,"port":22,"check":"tcp_tls22","sni":document["sni"],"expires_at":row.get::<chrono::DateTime<Utc>,_>("expires_at")}));
    }Ok(Json(ok(json!(jobs))))
}
/// Accept one bounded, authenticated port-22 report per unexpired assignment.
pub(crate) async fn result(ExtractState(state):ExtractState<State>,headers:HeaderMap,Json(body):Json<Value>)->Result<Json<Value>> {
    let agent=agent(&state,&headers).await?;let id=parse_id(field(&body,"id")?)?;
    let network_ok=body.get("network_ok").and_then(Value::as_bool).ok_or_else(bad)?;
    let port=&body["tcp22"];let stage=field(port,"stage")?;let latency=port["latency_ms"].as_u64().ok_or_else(bad)?;
    if body["check"]!="tcp_tls22" || port["port"]!=22 || latency>5000
        || !["ok","wrong_version_number","timeout","refused","tcp","tls","target","expired","network_unknown"].contains(&stage)
        || (network_ok && (port["reachable"].as_bool().is_none() || !["tcp","tls"].contains(&port["phase"].as_str().unwrap_or(""))
            || port["reachable"].as_bool()!=Some(matches!(stage,"ok"|"wrong_version_number")) || matches!(stage,"expired"|"network_unknown")
            || (matches!(stage,"ok"|"wrong_version_number") && port["phase"]!="tls")))
        || (!network_ok && (!port["reachable"].is_null() || !port["phase"].is_null() || !["expired","network_unknown"].contains(&stage))) {return Err(bad());}
    let document=json!({"check":"tcp_tls22","network_ok":network_ok,"reachable":port["reachable"],"stage":stage,"tcp22":port,"latency_ms":latency});
    let row=sqlx::query("UPDATE probe_jobs SET completed_at=now(),result=$3 WHERE id=$1 AND agent_id=$2 AND claimed_at IS NOT NULL AND completed_at IS NULL AND expires_at>now() RETURNING id").bind(id).bind(agent).bind(document).fetch_optional(&state.db).await?;
    if row.is_none(){return Err(ApiError(StatusCode::CONFLICT,"expired_or_replayed_probe"));}Ok(Json(ok(json!({"accepted":true}))))
}
pub(crate) async fn schedule(state:&State)->anyhow::Result<()> {
    let round=state.probe_round();
    // One bounded cross product avoids expiring assignments while issuing thousands of row writes.
    sqlx::query("WITH targets AS (SELECT id AS node_id,4 AS family,document->>'ipv4' AS address FROM nodes WHERE enabled AND coalesce(document->>'ipv4','')<>'' UNION SELECT id,6,document->>'ipv6' FROM nodes WHERE enabled AND coalesce(document->>'ipv6','')<>'' UNION SELECT n.id,CASE WHEN strpos(ip,':')>0 THEN 6 ELSE 4 END,ip FROM ip_pools p JOIN nodes n ON n.id=p.node_id CROSS JOIN LATERAL jsonb_array_elements_text(p.addresses) AS ip WHERE n.enabled) INSERT INTO probe_jobs(id,round,agent_id,node_id,family,address,expires_at) SELECT gen_random_uuid(),$1,a.id,t.node_id,t.family,t.address,now()+interval '14 seconds' FROM targets t CROSS JOIN agents a WHERE a.enabled ON CONFLICT DO NOTHING").bind(round).execute(&state.db).await?;Ok(())
}

/// Endpoint evidence state; unknown observations must not trigger failover.
#[derive(Clone,Copy,Debug,PartialEq,Eq)]pub(crate) enum Health {Healthy,Recovered,Failed,Unknown}
// Retain extra 15-second rounds for completion jitter; elapsed time, not count, gates failure.
const FAILURE_WINDOW_SECONDS:i64=300;
const FAILURE_WINDOW_ROUNDS:usize=24;
const MAX_OBSERVATION_GAP_MS:i64=30_000;
const RESERVE_SUCCESS_ROUNDS:usize=2;
const RECOVERY_SUCCESS_ROUNDS:usize=6;

/// Require uninterrupted timeouts from one agent and fresh evidence before any decision.
fn classify(observations:&[(Uuid,i64,Option<bool>,i64)],last:i64,now:i64)->Health {
    let mut agents:std::collections::HashMap<Uuid,[Option<(Option<bool>,i64)>;FAILURE_WINDOW_ROUNDS]>=std::collections::HashMap::new();
    for &(agent,round,result,completed) in observations {
        let Some(offset)=last.saturating_add(1).checked_sub(round).and_then(|offset|usize::try_from(offset).ok()).filter(|offset|*offset<FAILURE_WINDOW_ROUNDS)else{continue;};
        agents.entry(agent).or_insert([None;FAILURE_WINDOW_ROUNDS])[offset]=Some((result,completed));
    }
    // A result already received in the current round must interrupt an older failure streak.
    // If that round is still pending, require evidence from the immediately preceding round.
    for rounds in agents.values_mut() {if rounds[0].is_none(){rounds.copy_within(1..,0);rounds[FAILURE_WINDOW_ROUNDS-1]=None;}}
    agents.retain(|_,rounds|rounds[0].is_some_and(|(_,completed)|(0..=MAX_OBSERVATION_GAP_MS).contains(&now.saturating_sub(completed))));
    for rounds in agents.values() {
        let Some((Some(false),latest))=rounds[0]else{continue;};
        let mut previous=latest;
        for observation in rounds.iter().skip(1) {
            let Some((Some(false),completed))=*observation else{break;};
            if !(1..=MAX_OBSERVATION_GAP_MS).contains(&previous.saturating_sub(completed)){break;}
            if latest.saturating_sub(completed)>=FAILURE_WINDOW_SECONDS*1000{return Health::Failed;}
            previous=completed;
        }
    }
    if agents.values().any(|rounds|rounds[..RECOVERY_SUCCESS_ROUNDS].iter().all(|value|matches!(value,Some((Some(true),_))))){return Health::Recovered;}
    if agents.values().any(|rounds|rounds[..RESERVE_SUCCESS_ROUNDS].iter().all(|value|matches!(value,Some((Some(true),_))))){return Health::Healthy;}
    Health::Unknown
}

// Only a measured deadline expiry is failure evidence; refusals and protocol errors reset it.
fn transport_result(port:&Value)->Option<bool> {
    match (port["reachable"].as_bool(),port["stage"].as_str()) {
        (Some(true),Some("ok"|"wrong_version_number"))=>Some(true),
        (Some(false),Some("timeout"))=>Some(false),
        _=>None,
    }
}

/// Shared bounded TCP/TLS port-22 evidence for node decisions and local IP rotation.
pub(crate) async fn evidence(state:&State,node:Uuid,ip:&str)->anyhow::Result<Health> {
    let last=state.probe_round()-1;
    let rows=sqlx::query("SELECT j.round,j.agent_id,j.result,j.completed_at FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.address=$2 AND j.round BETWEEN $3 AND $4 AND j.completed_at>now()-make_interval(secs => $5) AND a.enabled AND a.country='RU'")
        .bind(node).bind(ip).bind(last-FAILURE_WINDOW_ROUNDS as i64+1).bind(last+1).bind((FAILURE_WINDOW_ROUNDS*15) as f64).fetch_all(&state.db).await?;
    let mut observations=Vec::new();
    for row in rows {
        let result:Value=row.get("result");let agent:Uuid=row.get("agent_id");let round:i64=row.get("round");
        let completed=row.get::<chrono::DateTime<Utc>,_>("completed_at").timestamp_millis();
        // Old WEB/MTProxy reports cannot contribute to TCP/TLS port-22 decisions.
        let value=if result["network_ok"]==true && result["check"]=="tcp_tls22" && result["tcp22"]["port"]==22 {transport_result(&result["tcp22"])}else{None};
        observations.push((agent,round,value,completed));
    }
    Ok(classify(&observations,last,Utc::now().timestamp_millis()))
}
/// Preserve recovery quarantine while evaluating the node's currently selected address.
pub(crate) async fn health(state:&State,node:Uuid,family:i32)->anyhow::Result<Health> {
    let last=state.probe_round()-1;
    let document:Value=sqlx::query_scalar("SELECT document FROM nodes WHERE id=$1").bind(node).fetch_one(&state.db).await?;
    let ip=document[if family==4{"ipv4"}else{"ipv6"}].as_str().unwrap_or("");
    let calculated=evidence(state,node,ip).await?;
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
    fn samples(agent:Uuid,count:i64,good:Option<bool>)->Vec<(Uuid,i64,Option<bool>,i64)>{(0..count).map(|offset|(agent,100-offset,good,1_000_000-offset*15_000)).collect()}
    #[test]fn failure_requires_five_elapsed_minutes_without_interruptions(){
        let agent=Uuid::from_u128(1);let rows=samples(agent,21,Some(false));
        assert_eq!(classify(&rows[..20],100,1_000_000),Health::Unknown);
        assert_eq!(classify(&rows,100,1_000_000),Health::Failed);
        let mut short=rows.clone();short[20].3+=1;
        assert_eq!(classify(&short,100,1_000_000),Health::Unknown);
        for interruption in [None,Some(true)] {let mut interrupted=rows.clone();interrupted[10].2=interruption;assert_eq!(classify(&interrupted,100,1_000_000),Health::Unknown);}
        let mut missing=rows.clone();missing.remove(10);assert_eq!(classify(&missing,100,1_000_000),Health::Unknown);
        let mut mixed=rows.clone();mixed[10].0=Uuid::from_u128(2);assert_eq!(classify(&mixed,100,1_000_000),Health::Unknown);
        let mut compressed=rows.clone();for row in &mut compressed {row.3=1_000_000;}assert_eq!(classify(&compressed,100,1_000_000),Health::Unknown);
        assert_eq!(classify(&rows,101,1_000_000),Health::Unknown);
        let mut current=rows.clone();current.push((agent,101,Some(true),1_015_000));
        assert_eq!(classify(&current,100,1_015_000),Health::Unknown);
        current.last_mut().unwrap().2=None;assert_eq!(classify(&current,100,1_015_000),Health::Unknown);
        assert_eq!(classify(&rows,100,1_030_001),Health::Unknown);
        assert_eq!(classify(&samples(agent,2,Some(true)),100,1_000_000),Health::Healthy);
        assert_eq!(classify(&samples(agent,6,Some(true)),100,1_000_000),Health::Recovered);
    }
    #[test]fn only_timeout_is_failure_and_plaintext_tls_response_is_alive(){
        for stage in ["tcp","refused","tls","target","expired","network_unknown"] {assert_eq!(transport_result(&json!({"reachable":false,"stage":stage})),None);}
        assert_eq!(transport_result(&json!({"reachable":false,"stage":"timeout"})),Some(false));
        assert_eq!(transport_result(&json!({"reachable":true,"stage":"wrong_version_number"})),Some(true));
        assert_eq!(transport_result(&json!({"reachable":false,"stage":"wrong_version_number"})),None);
    }
}
