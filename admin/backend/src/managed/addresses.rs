//! Replace assigned node addresses without requiring the obsolete listeners to remain reachable.
use crate::{auth::Session,error::{Result,ApiError,bad},state::{State,ok}};
use axum::{http::StatusCode,Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

fn conflict(code:&'static str)->ApiError{ApiError(StatusCode::CONFLICT,code)}
fn addresses(body:&Value)->Result<Value> {
    let mut result=json!({});
    for (key,v4) in [("ipv4",true),("ipv6",false)] {
        let value=body.get(key).ok_or_else(bad)?;
        let ip=if value.is_null(){None}else{Some(value.as_str().ok_or_else(bad)?.trim()).filter(|ip|!ip.is_empty())};
        result[key]=if let Some(ip)=ip {
            let parsed:std::net::IpAddr=ip.parse().map_err(|_|conflict("node_address_invalid"))?;
            if parsed.is_ipv4()!=v4||parsed.is_unspecified()||parsed.is_multicast()||parsed.is_loopback()||matches!(parsed,std::net::IpAddr::V6(ip) if ip.to_ipv4_mapped().is_some()) {return Err(conflict("node_address_invalid"));}
            json!(parsed.to_string())
        }else{Value::Null};
    }
    if result["ipv4"].is_null()&&result["ipv6"].is_null(){return Err(conflict("node_address_required"));}Ok(result)
}

/// Queue a replacement under the same lock order as profile publication and DNS writes.
pub(super) async fn update(state:&State,user:&Session,id:Uuid,body:&Value)->Result<Json<Value>> {
    user.require("owner")?;
    let desired=addresses(body)?;let expected=addresses(body.get("expected_addresses").ok_or_else(bad)?)?;
    let mut tx=state.db.begin().await?;
    crate::profiles::lock_profile(&mut tx,id).await?;
    let row=sqlx::query("SELECT n.document,m.revoked FROM nodes n JOIN managed_nodes m ON m.node_id=n.id WHERE n.id=$1 FOR UPDATE OF n,m").bind(id).fetch_one(&mut *tx).await?;
    if row.get::<bool,_>("revoked"){return Err(conflict("node_access_revoked"));}
    let mut document:Value=row.get("document");let old=addresses(&json!({"ipv4":document["ipv4"],"ipv6":document["ipv6"]}))?;
    if old!=expected{return Err(conflict("node_addresses_changed"));}
    if old==desired{return Err(conflict("node_addresses_unchanged"));}
    let groups:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM failover_groups WHERE active_node=$1 OR document->'node_ids' @> $2::jsonb").bind(id).bind(json!([id])).fetch_all(&mut *tx).await?;
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dns_changes WHERE (group_id=ANY($1) OR old_node=$2 OR new_node=$2) AND state IN('writing','unknown')) OR EXISTS(SELECT 1 FROM ip_rotations r JOIN ip_pools p ON p.id=r.pool_id WHERE (p.node_id=$2 OR p.group_id=ANY($1)) AND r.state IN('queued','dns_pending','unknown','retiring'))").bind(&groups).bind(id).fetch_one(&mut *tx).await?;
    if pending{return Err(conflict("node_address_operation_pending"));}
    if let Some(pool)=sqlx::query("SELECT id,addresses FROM ip_pools WHERE node_id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await? {
        let mut ips:Vec<String>=serde_json::from_value(pool.get("addresses")).map_err(|_|bad())?;
        for key in ["ipv4","ipv6"] {
            if old[key]!=desired[key] {
                ips.retain(|ip|Some(ip.as_str())!=old[key].as_str());
                if let Some(ip)=desired[key].as_str() && !ips.iter().any(|value|value==ip){ips.push(ip.to_owned());}
            }
        }
        if !(2..=8).contains(&ips.len()){return Err(conflict("node_address_pool_size"));}
        sqlx::query("UPDATE ip_pools SET addresses=$2,automatic=false,status='preparing',updated_at=now() WHERE id=$1").bind(pool.get::<Uuid,_>("id")).bind(json!(ips)).execute(&mut *tx).await?;
    }
    for key in ["ipv4","ipv6"]{document[key]=desired[key].clone();}
    sqlx::query("UPDATE nodes SET document=$2,snapshot='{}',collected_at=NULL WHERE id=$1").bind(id).bind(document).execute(&mut *tx).await?;
    sqlx::query("UPDATE managed_nodes SET cached_reads_cipher=NULL,cached_at=NULL WHERE node_id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM address_health WHERE node_id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE failover_groups SET automatic=false,updated_at=now() WHERE id=ANY($1)").bind(&groups).execute(&mut *tx).await?;
    sqlx::query("UPDATE profiles SET resume_automatic=false WHERE group_id=ANY($1)").bind(&groups).execute(&mut *tx).await?;
    sqlx::query("UPDATE ip_pools SET automatic=false,updated_at=now() WHERE node_id=$1 OR group_id=ANY($2)").bind(id).bind(&groups).execute(&mut *tx).await?;
    let deployment=crate::profiles::enqueue(state,&mut tx,id,Some(user.id)).await?;
    sqlx::query("UPDATE ip_pools SET deployment=$2 WHERE node_id=$1").bind(id).bind(deployment).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'node.addresses',$2,$3)").bind(user.id).bind(id.to_string()).bind(json!({"old":old,"addresses":desired,"deployment":deployment,"dns_unchanged":true})).execute(&mut *tx).await?;
    tx.commit().await?;Ok(Json(ok(json!({"addresses":desired,"deployment":deployment,"dns_unchanged":true}))))
}
