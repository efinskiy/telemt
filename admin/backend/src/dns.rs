//! Cloudflare reconciliation owns only enrolled record IDs and fences every uncertain write.
use axum::{http::{Method,StatusCode},Json};
use chrono::{Utc,DateTime};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{auth::Session,error::{Result,bad,ApiError},state::{State,field,parse_id,ok,audit,fresh},probes::{self,Health}};
pub(crate) mod provider;
mod management;
use provider::{read_records,eligible,change};

pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>> {
    match parts {
        ["groups"] if *method==Method::GET=>{
            let rows=sqlx::query("SELECT * FROM failover_groups ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"document":r.get::<Value,_>("document"),"active_node":r.get::<Option<Uuid>,_>("active_node"),"automatic":r.get::<bool,_>("automatic"),"last_switch":r.get::<Option<DateTime<Utc>>,_>("last_switch"),"status":r.get::<String,_>("status"),"observed":r.get::<Value,_>("observed")})).collect::<Vec<_>>()))))
        }
        ["groups"] if *method==Method::POST=>{
            user.require("owner")?;
            let invalid=|code|ApiError(StatusCode::BAD_REQUEST,code);
            let name=crate::state::bounded_name(&body,"name").map_err(|_|invalid("dns_invalid_name"))?;
            let domain=field(&body,"domain").map_err(|_|invalid("dns_invalid_domain"))?;
            if domain.len()>253||!domain.bytes().all(|c|c.is_ascii_alphanumeric()||b".-".contains(&c)){return Err(invalid("dns_invalid_domain"));}
            let zone=field(&body,"zone_id").map_err(|_|invalid("dns_invalid_zone_id"))?;
            if zone.len()!=32||hex::decode(zone).is_err(){return Err(invalid("dns_invalid_zone_id"));}
            let nodes=body.get("node_ids").and_then(Value::as_array).ok_or_else(||invalid("dns_invalid_members"))?;
            if nodes.is_empty()||nodes.len()>20{return Err(invalid("dns_invalid_members"));}
            let mut unique=std::collections::HashSet::new();let mut ordered=Vec::new();
            for node in nodes {
                let node=node.as_str().and_then(|id|Uuid::parse_str(id).ok()).ok_or_else(||invalid("dns_invalid_members"))?;
                if !unique.insert(node){return Err(invalid("dns_invalid_members"));}ordered.push(node);
                crate::state::node(state,node).await.map_err(|error|if error.1=="invalid_request"{invalid("dns_member_not_found")}else{error})?;
            }
            for (key,code) in [("record_a_id","dns_invalid_a_record_id"),("record_aaaa_id","dns_invalid_aaaa_record_id")] {
                if let Some(id)=body.get(key).and_then(Value::as_str).filter(|s|!s.is_empty()){if id.len()!=32||hex::decode(id).is_err(){return Err(invalid(code));}}
            }
            if body["record_a_id"].as_str().is_none_or(|s|s.is_empty())&&body["record_aaaa_id"].as_str().is_none_or(|s|s.is_empty()){return Err(invalid("dns_record_required"));}
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM failover_groups").fetch_one(&state.db).await?;
            if count>=20{return Err(ApiError(StatusCode::CONFLICT,"dns_group_limit"));}
            let id=Uuid::new_v4();let mut document=json!({"domain":domain.to_ascii_lowercase(),"zone_id":zone,"node_ids":ordered,"record_a_id":body.get("record_a_id"),"record_aaaa_id":body.get("record_aaaa_id"),"configuration_verified":true});
            let token=field(&body,"token").map_err(|_|invalid("dns_invalid_token"))?;
            if token.is_empty()||token.len()>1024{return Err(invalid("dns_invalid_token"));}
            let observed=read_records(state,&document,token).await?;
            let active=body["active_node"].as_str().and_then(|id|Uuid::parse_str(id).ok()).ok_or_else(||invalid("dns_invalid_active_node"))?;
            if !unique.contains(&active){return Err(invalid("dns_invalid_active_node"));}
            let n=crate::state::node(state,active).await?;let doc:Value=n.get("document");
            let snapshot:Value=n.get("snapshot");
            // Operational prerequisites are not form errors; preserve the checks and identify the failed gate.
            if !fresh(n.get("collected_at"),15){return Err(ApiError(StatusCode::CONFLICT,"dns_active_node_stale"));}
            if snapshot["compatibility"]["fingerprint"].as_str().is_none(){return Err(ApiError(StatusCode::CONFLICT,"dns_active_node_compatibility_unavailable"));}
            document["fingerprint"]=snapshot["compatibility"]["fingerprint"].clone();
            document["port"]=doc["port"].clone();document["sni"]=doc["sni"].clone();
            for (key,record) in observed.as_object().ok_or_else(bad)? {
                if record["content"]!=doc[if key=="A"{"ipv4"}else{"ipv6"}]{return Err(ApiError(StatusCode::CONFLICT,if key=="A"{"dns_active_ipv4_mismatch"}else{"dns_active_ipv6_mismatch"}));}
            }
            let cipher=state.vault.seal(token,&format!("dns:{id}"))?;
            sqlx::query("INSERT INTO failover_groups(id,name,document,token_cipher,active_node,observed) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(name).bind(document).bind(cipher).bind(active).bind(observed).execute(&state.db).await?;
            audit(state,Some(user.id),"dns.enroll",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({"id":id}))))
        }
        ["groups",id] if *method==Method::PATCH=>management::patch(state,user,parse_id(id)?,body).await,
        ["groups",id] if *method==Method::DELETE=>management::remove(state,user,parse_id(id)?).await,
        ["groups",id,"records"] if *method==Method::GET=>{user.require("owner")?;let id=parse_id(id)?;let group=load(state,id).await?;let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{id}"))?;Ok(Json(ok(read_records(state,&group.get::<Value,_>("document"),&token).await?)))}
        ["groups",id,"repair"] if *method==Method::POST=>{user.require("owner")?;provider::repair(state,parse_id(id)?,parse_id(field(&body,"node_id")?)?,body.get("expected_records").ok_or_else(bad)?,user.id).await?;Ok(Json(ok(json!({}))))}
        ["groups",id,"verify-configuration"] if *method==Method::POST=>management::verify(state,user,parse_id(id)?).await,
        ["groups",id,"preview"] if *method==Method::POST=>{user.require("owner")?;let id=parse_id(id)?;let target=parse_id(field(&body,"node_id")?)?;let group=load(state,id).await?;let intent=eligible(state,&group,target).await?;Ok(Json(ok(json!({"node_id":target,"intent":intent,"ttl":60,"dns_only":true}))))}
        ["groups",id,"switch"] if *method==Method::POST=>{user.require("owner")?;change(state,parse_id(id)?,parse_id(field(&body,"node_id")?)?,Some(user.id)).await?;Ok(Json(ok(json!({}))))}
        ["groups",id,"reconcile"] if *method==Method::POST=>{user.require("owner")?;provider::recover(state,parse_id(id)?).await?;Ok(Json(ok(json!({}))))}
        ["groups",id,"history"] if *method==Method::GET=>{let rows=sqlx::query("SELECT id,old_node,new_node,intent,state,result,created_at FROM dns_changes WHERE group_id=$1 ORDER BY created_at DESC LIMIT 100").bind(parse_id(id)?).fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"old_node":r.get::<Option<Uuid>,_>("old_node"),"new_node":r.get::<Uuid,_>("new_node"),"intent":r.get::<Value,_>("intent"),"state":r.get::<String,_>("state"),"result":r.get::<Value,_>("result"),"created_at":r.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>()))))}
        _=>Err(bad()),
    }
}
pub(super) async fn load(state:&State,id:Uuid)->Result<sqlx::postgres::PgRow>{sqlx::query("SELECT * FROM failover_groups WHERE id=$1").bind(id).fetch_optional(&state.db).await?.ok_or_else(bad)}
pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    let groups=sqlx::query("SELECT * FROM failover_groups WHERE automatic ORDER BY id LIMIT 20").fetch_all(&state.db).await?;
    for group in groups {
        let id:Uuid=group.get("id");let active:Option<Uuid>=group.get("active_node");let Some(active)=active else{continue;};
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown')").bind(id).fetch_one(&state.db).await?;
        if pending>0{let _=provider::recover(state,id).await;continue;}
        let document:Value=group.get("document");
        let owning_node=sqlx::query("SELECT snapshot,collected_at FROM nodes WHERE id=$1").bind(active).fetch_one(&state.db).await?;let snapshot:Value=owning_node.get("snapshot");
        if fresh(owning_node.get("collected_at"),15) && snapshot.pointer("/compatibility/fingerprint").is_some_and(|fingerprint|fingerprint!=&document["fingerprint"]) {
            sqlx::query("UPDATE failover_groups SET automatic=false,document=jsonb_set(document,'{configuration_verified}','false'),status='configuration_drift' WHERE id=$1").bind(id).execute(&state.db).await?;continue;
        }
        let families:Vec<_>=[("record_a_id",4),("record_aaaa_id",6)].into_iter().filter(|(key,_)|document[*key].as_str().is_some_and(|s|!s.is_empty())).map(|(_,f)|f).collect();
        let mut failed=false;let mut unknown=false;
        for family in &families{let health=probes::health(state,active,*family).await?;failed|=health==Health::Failed;unknown|=health==Health::Unknown;}
        let cooldown=fresh(group.get("last_switch"),300);
        let mut status=if unknown{"probe_state_unknown"}else if failed{"no_healthy_reserve"}else{"healthy"};
        if failed&&!unknown&&!cooldown {
            if let Some(nodes)=document["node_ids"].as_array(){for candidate in nodes {let Some(raw)=candidate.as_str()else{continue;};let candidate:Uuid=raw.parse()?;if candidate==active{continue;}
                if eligible(state,&group,candidate).await.is_ok(){match change(state,id,candidate,None).await{Ok(())=>status="switched",Err(error)=>status=error.1,}break;}
            }}
        }else if failed&&cooldown{status="cooldown";}
        sqlx::query("UPDATE failover_groups SET status=$2,updated_at=now() WHERE id=$1 AND automatic").bind(id).bind(status).execute(&state.db).await?;
    }Ok(())
}
