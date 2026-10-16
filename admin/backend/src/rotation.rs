//! Managed endpoint pools retain listeners for independent recovery probes after DNS rotation.
use axum::{http::{Method,StatusCode},Json};
use chrono::{DateTime,Utc};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{state::{State,node,node_request,field,parse_id,ok,fresh},auth::Session,error::{Result,bad,ApiError}};
mod workflow;
mod policy;
mod recovery;
// One eligibility policy orders local address rotation before same-profile failover.
pub(crate) mod selection;
use policy::{inventory,addresses,listener_plan,has_listener,health};
pub(crate) use workflow::{tick,finish_dns};

pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>> {
    match parts {
        ["ip-pools"] if *method==Method::GET=>{
            let rows=sqlx::query("SELECT * FROM ip_pools ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|row|json!({"id":row.get::<Uuid,_>("id"),"name":row.get::<String,_>("name"),"node_id":row.get::<Uuid,_>("node_id"),"group_id":row.get::<Uuid,_>("group_id"),"addresses":row.get::<Value,_>("addresses"),"automatic":row.get::<bool,_>("automatic"),"status":row.get::<String,_>("status"),"deployment":row.get::<Option<i64>,_>("deployment"),"preparation_error":row.get::<Option<String>,_>("preparation_error"),"preparation_checked_at":row.get::<Option<DateTime<Utc>>,_>("preparation_checked_at"),"last_rotation":row.get::<Option<DateTime<Utc>>,_>("last_rotation")})).collect::<Vec<_>>()))))
        }
        ["ip-pools"] if *method==Method::POST=>{
            user.require("owner")?;let node_id=parse_id(field(&body,"node_id")?)?;let group_id=parse_id(field(&body,"group_id")?)?;
            let assigned=inventory(state,node_id).await?;let selected=addresses(&body["addresses"],&assigned)?;
            let owning=node(state,node_id).await?;let doc:Value=owning.get("document");
            let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            let group=super::dns::load(state,group_id).await?;let group_doc:Value=group.get("document");
            if !group_doc["node_ids"].as_array().is_some_and(|ids|ids.contains(&json!(node_id)))||group_doc["configuration_verified"]!=true{return Err(conflict("pool_node_must_belong_to_verified_group"));}
            crate::rotation::selection::same_profile(state,&group,node_id).await?;
            for address in &selected {let family=if address.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4(){"record_a_id"}else{"record_aaaa_id"};if group_doc[family].as_str().is_none_or(str::is_empty){return Err(conflict("address_family_without_enrolled_record"));}}
            for key in ["ipv4","ipv6"] {if let Some(ip)=doc[key].as_str().filter(|ip|!ip.is_empty()) && group_doc[if key=="ipv4"{"record_a_id"}else{"record_aaaa_id"}].as_str().is_some_and(|id|!id.is_empty())&&!selected.iter().any(|selected|selected==ip){return Err(conflict("include_current_dns_addresses_in_pool"));}}
            let id=Uuid::new_v4();let name=crate::state::bounded_name(&body,"name")?;
            sqlx::query("INSERT INTO ip_pools(id,node_id,group_id,name,addresses,owner_id) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(node_id).bind(group_id).bind(name).bind(json!(selected)).bind(user.id).execute(&mut *tx).await?;
            audit_tx(&mut tx,user.id,"ip_pool.create",id,json!({"node":node_id,"group":group_id})).await?;tx.commit().await?;Ok(Json(ok(json!({"id":id}))))
        }
        ["ip-pools",id] if *method==Method::PATCH=>{
            user.require("owner")?;let id=parse_id(id)?;let automatic=body["automatic"].as_bool().ok_or_else(bad)?;
            let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            let pool=load(state,id).await?;
            if automatic {workflow::ready(state,&pool).await?;let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('queued','dns_pending','unknown')").bind(id).fetch_one(&mut *tx).await?;if pending>0{return Err(conflict("rotation_pending"));}}
            sqlx::query("UPDATE ip_pools SET automatic=$2,status=$3,owner_id=$4,updated_at=now() WHERE id=$1").bind(id).bind(automatic).bind(if automatic{"waiting_for_quorum"}else if pool.get::<String,_>("status")=="preparing"{"preparing"}else{"disabled"}).bind(user.id).execute(&mut *tx).await?;
            audit_tx(&mut tx,user.id,"ip_pool.automation",id,json!({"automatic":automatic})).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
        }
        ["ip-pools",id] if *method==Method::DELETE=>{
            user.require("owner")?;let id=parse_id(id)?;let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;let pool=load(state,id).await?;
            let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('queued','dns_pending','unknown')").bind(id).fetch_one(&mut *tx).await?;
            if pool.get::<bool,_>("automatic")||pending>0{return Err(conflict("disable_and_reconcile_before_removal"));}
            audit_tx(&mut tx,user.id,"ip_pool.remove",id,json!({"addresses":pool.get::<Value,_>("addresses"),"listeners_and_dns_unchanged":true})).await?;
            sqlx::query("DELETE FROM ip_rotations WHERE pool_id=$1").bind(id).execute(&mut *tx).await?;sqlx::query("DELETE FROM ip_pools WHERE id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
        }
        ["ip-pools",id,"preview"] if *method==Method::GET=>{
            let pool=load(state,parse_id(id)?).await?;let node_id:Uuid=pool.get("node_id");let assigned=inventory(state,node_id).await?;
            let plan=listener_plan(&assigned["active_listeners"],&pool.get::<Value,_>("addresses"),&workflow::listener_document(state,&pool).await?)?;
            Ok(Json(ok(json!({"inventory":assigned,"listener_patch":{"server":{"listeners":plan}},"deployment_required":true,"restart_required":true,"dns_unchanged":true}))))
        }
        ["ip-pools",id,"prepare"] if *method==Method::POST=>{
            user.require("owner")?;workflow::prepare(state,parse_id(id)?,user.id,field(&body,"revision")?).await?;Ok(Json(ok(json!({"state":"preparing"}))))
        }
        ["ip-pools",id,"acknowledge-dns"] if *method==Method::POST=>{user.require("owner")?;recovery::acknowledge_dns(state,parse_id(id)?,user.id,body.get("expected_records").ok_or_else(bad)?).await?;Ok(Json(ok(json!({}))))}
        ["ip-pools",id,"rotation-preview"] if *method==Method::POST=>{
            user.require("owner")?;let pool=load(state,parse_id(id)?).await?;let assigned=workflow::ready(state,&pool).await?;let ip=field(&body,"ip")?;let node_id:Uuid=pool.get("node_id");
            if !pool.get::<Value,_>("addresses").as_array().is_some_and(|addresses|addresses.iter().any(|address|address==ip)){return Err(bad());}
            let family=if ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4(){4}else{6};let doc:Value=node(state,node_id).await?.get("document");let old=&doc[if family==4{"ipv4"}else{"ipv6"}];
            if old==ip||!has_listener(&assigned,ip,doc["port"].as_u64().ok_or_else(bad)?)||!matches!(health(state,node_id,ip).await?,crate::probes::Health::Healthy|crate::probes::Health::Recovered){return Err(conflict("reserve_ip_not_prepared_or_without_quorum"));}
            let group=crate::dns::load(state,pool.get("group_id")).await?;let group_doc:Value=group.get("document");let observed:Value=group.get("observed");let version=if family==4{"A"}else{"AAAA"};
            Ok(Json(ok(json!({"ip":ip,"old_ip":old,"expected_records":observed,"dns_change":{"family":version,"record_id":group_doc[if family==4{"record_a_id"}else{"record_aaaa_id"}],"content":ip,"ttl":60,"proxied":false},"listeners_retained":true}))))
        }
        ["ip-pools",id,"status"] if *method==Method::GET=>{
            let pool=load(state,parse_id(id)?).await?;let node_id:Uuid=pool.get("node_id");let assigned=inventory(state,node_id).await?;let doc:Value=node(state,node_id).await?.get("document");let selected:Value=pool.get("addresses");let mut results=Vec::new();
            for ip in selected.as_array().ok_or_else(bad)? {let ip=ip.as_str().ok_or_else(bad)?;let health=health(state,node_id,ip).await?;let ports=policy::port_health(state,node_id,ip).await?;results.push(json!({"ip":ip,"native":ports[0],"web":ports[1],"native_port":doc["port"],"web_port":443,"health":format!("{health:?}").to_ascii_lowercase(),"listening":has_listener(&assigned,ip,doc["port"].as_u64().ok_or_else(bad)?),"current":doc["ipv4"]==ip||doc["ipv6"]==ip}));}
            Ok(Json(ok(json!({"addresses":results,"inventory":assigned}))))
        }
        ["ip-pools",id,"dns-repair-preview"] if *method==Method::GET=>{user.require("owner")?;let pool=load(state,parse_id(id)?).await?;Ok(Json(ok(recovery::dns_preview(state,&pool).await?)))}
        ["ip-pools",id,"repair-dns"] if *method==Method::POST=>{user.require("owner")?;recovery::repair_dns(state,parse_id(id)?,user.id,&body).await?;Ok(Json(ok(json!({}))))}
        ["ip-pools",id,"cancel"] if *method==Method::POST=>{
            user.require("owner")?;let id=parse_id(id)?;let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            let changed=sqlx::query("UPDATE ip_rotations SET state='failed',result='{\"reason\":\"cancelled_before_dns_write\"}',updated_at=now() WHERE pool_id=$1 AND state='queued' AND dns_change IS NULL").bind(id).execute(&mut *tx).await?;
            if changed.rows_affected()!=1{return Err(conflict("rotation_cannot_be_cancelled_after_dns_write"));}
            sqlx::query("UPDATE ip_pools SET automatic=false,status='cancelled' WHERE id=$1").bind(id).execute(&mut *tx).await?;audit_tx(&mut tx,user.id,"ip.rotate.cancel",id,json!({"dns_unchanged":true})).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
        }
        ["ip-pools",id,"rotate"] if *method==Method::POST=>{
            user.require("owner")?;let id=parse_id(id)?;let rotation=workflow::enqueue(state,id,field(&body,"ip")?,Some(user.id),"manual",Some(&body)).await?;Ok(Json(ok(json!({"id":rotation,"state":"queued"}))))
        }
        ["ip-pools",id,"reconcile"] if *method==Method::POST=>{
            user.require("owner")?;let pool=load(state,parse_id(id)?).await?;crate::dns::provider::recover(state,pool.get("group_id")).await?;Ok(Json(ok(json!({}))))
        }
        ["ip-pools",id,"history"] if *method==Method::GET=>{
            let rows=sqlx::query("SELECT * FROM ip_rotations WHERE pool_id=$1 ORDER BY created_at DESC LIMIT 100").bind(parse_id(id)?).fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|row|json!({"id":row.get::<Uuid,_>("id"),"old_ip":row.get::<String,_>("old_ip"),"new_ip":row.get::<String,_>("new_ip"),"state":row.get::<String,_>("state"),"reason":row.get::<String,_>("reason"),"dns_change":row.get::<Option<Uuid>,_>("dns_change"),"result":row.get::<Value,_>("result"),"created_at":row.get::<DateTime<Utc>,_>("created_at")})).collect::<Vec<_>>()))))
        }
        _=>Err(bad()),
    }
}
fn conflict(code:&'static str)->ApiError{ApiError(StatusCode::CONFLICT,code)}
async fn load(state:&State,id:Uuid)->Result<sqlx::postgres::PgRow>{sqlx::query("SELECT * FROM ip_pools WHERE id=$1").bind(id).fetch_optional(&state.db).await?.ok_or_else(bad)}
async fn audit_tx(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,actor:Uuid,action:&str,id:Uuid,outcome:Value)->Result<()> {
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,$2,$3,$4)").bind(actor).bind(action).bind(id.to_string()).bind(outcome).execute(&mut **tx).await?;Ok(())
}

/// Ordinary edits are fenced while a rotation may have already changed DNS.
pub(crate) async fn guard_node(state:&State,node:Uuid)->Result<()> {
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations r JOIN ip_pools p ON p.id=r.pool_id WHERE p.node_id=$1 AND r.state IN('queued','dns_pending','unknown')").bind(node).fetch_one(&state.db).await?;
    if count>0{return Err(conflict("ip_rotation_pending_requires_reconciliation"));}Ok(())
}
