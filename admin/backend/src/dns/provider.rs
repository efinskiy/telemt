use super::*;
use crate::state::node;

async fn cloudflare(state:&State,token:&str,method:reqwest::Method,path:&str,body:Option<&Value>)->anyhow::Result<Value> {
    anyhow::ensure!(path.starts_with("/zones/") && !path.contains(['?','#','\\']),"invalid Cloudflare path");
    let base="https://api.cloudflare.com/client/v4";
    #[cfg(test)]
    let base=state.cloudflare_origin.as_deref().unwrap_or(base);
    let mut req=state.client.request(method,format!("{base}{path}")).bearer_auth(token);
    if let Some(body)=body{req=req.json(body);}
    let mut response=req.send().await?;let status=response.status();let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await?{anyhow::ensure!(bytes.len()+chunk.len()<=1024*1024,"Cloudflare response too large");bytes.extend_from_slice(&chunk);}
    let value:Value=serde_json::from_slice(&bytes)?;
    anyhow::ensure!(status.is_success()&&value["success"]==true,"Cloudflare request failed");Ok(value["result"].clone())
}
pub(crate) async fn read_records(state:&State,document:&Value,token:&str)->anyhow::Result<Value> {
    let zone=document["zone_id"].as_str().ok_or_else(||anyhow::anyhow!("missing zone"))?;let mut records=json!({});
    for (key,family) in [("record_a_id","A"),("record_aaaa_id","AAAA")] {
        let Some(id)=document[key].as_str().filter(|s|!s.is_empty())else{continue;};
        let record=cloudflare(state,token,reqwest::Method::GET,&format!("/zones/{zone}/dns_records/{id}"),None).await?;
        anyhow::ensure!(record["name"]==document["domain"]&&record["type"]==family&&record["proxied"]==false,"record enrollment mismatch");
        records[family]=json!({"id":id,"content":record["content"],"ttl":record["ttl"],"modified_on":record["modified_on"]});
    }Ok(records)
}

pub(super) async fn eligible(state:&State,group:&sqlx::postgres::PgRow,target:Uuid)->Result<Value> {
    if !crate::managed::admission::ready(state,target).await? {return Err(ApiError(StatusCode::CONFLICT,"managed_node_not_ready"));}
    crate::rotation::selection::same_profile(state,group,target).await?;
    let document:Value=group.get("document");let nodes=document["node_ids"].as_array().ok_or_else(bad)?;
    if !nodes.iter().any(|id|id.as_str()==Some(target.to_string().as_str())){return Err(bad());}
    let candidate=node(state,target).await?;
    let candidate_doc:Value=candidate.get("document");let snapshot:Value=candidate.get("snapshot");
    if candidate_doc["port"]!=document["port"]||candidate_doc["sni"]!=document["sni"]
        || !fresh(candidate.get("collected_at"),15)||snapshot["ready"]!=true||!candidate.get::<bool,_>("enabled")
        || document["fingerprint"].as_str().is_none()
        || snapshot["compatibility"]["fingerprint"]!=document["fingerprint"] {
        return Err(ApiError(axum::http::StatusCode::CONFLICT,"incompatible_or_unready_reserve"));
    }
    let mut intent=json!({});
    for (key,family,version) in [("record_a_id","A",4),("record_aaaa_id","AAAA",6)] {
        if document[key].as_str().is_none_or(|s|s.is_empty()){continue;}
        let ip=crate::rotation::selection::candidate(state,target,version==4).await?;
        intent[family]=json!({"id":document[key],"content":ip,"ttl":60,"proxied":false});
    }Ok(intent)
}

async fn record_cooldown(state:&State,document:&Value)->Result<bool> {
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM dns_record_cooldown WHERE zone_id=$1 AND record_id=ANY($2) AND last_switch>now()-interval '5 minutes'")
        .bind(document["zone_id"].as_str().ok_or_else(bad)?)
        .bind(["record_a_id","record_aaaa_id"].iter().filter_map(|key|document[*key].as_str()).collect::<Vec<_>>()).fetch_one(&state.db).await?;
    Ok(count>0)
}

pub(super) async fn change(state:&State,id:Uuid,target:Uuid,actor:Option<Uuid>)->Result<()> {
    let mut connection=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *connection).await?;
    if !locked{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_coordinator_busy"));}
    let result=write_owned(state,id,target,actor,None).await;
    connection.rollback().await?;
    result
}

/// The caller must hold the shared DNS coordinator transaction lock.
pub(crate) async fn rotate_locked(state:&State,id:Uuid,node:Uuid,actor:Option<Uuid>,rotation:Uuid,intent:Value)->Result<()> {
    write_owned(state,id,node,actor,Some((rotation,intent))).await
}

async fn write_owned(state:&State,id:Uuid,target:Uuid,actor:Option<Uuid>,rotation:Option<(Uuid,Value)>)->Result<()> {
    if rotation.is_none() { guard_pool(state,id).await?; }
    let group=load(state,id).await?;
    if rotation.is_none() && let Some(active)=group.get::<Option<Uuid>,_>("active_node") && active!=target && !crate::rotation::selection::exhausted(state,active).await? {
        return Err(ApiError(StatusCode::CONFLICT,"current_node_addresses_not_exhausted"));
    }
    if rotation.is_none() && actor.is_none() && !group.get::<bool,_>("automatic") { return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_automation_disabled")); }
    if fresh(group.get("last_switch"),300)||record_cooldown(state,&group.get::<Value,_>("document")).await?{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_cooldown"));}
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown')").bind(id).fetch_one(&state.db).await?;
    if pending>0{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_outcome_unknown"));}
    let intent=if let Some((_,intent))=&rotation{intent.clone()}else{eligible(state,&group,target).await?};let document:Value=group.get("document");
    let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{id}"))?;
    let observed=read_records(state,&document,&token).await?;let expected:Value=group.get("observed");
    if observed!=expected {
        sqlx::query("UPDATE failover_groups SET status='external_dns_conflict',automatic=false WHERE id=$1").bind(id).execute(&state.db).await?;
        return Err(ApiError(axum::http::StatusCode::CONFLICT,"external_dns_conflict"));
    }
    let change=Uuid::new_v4();let old:Option<Uuid>=group.get("active_node");
    // This committed intent survives a crash between Cloudflare accepting the update and its response.
    let mut intent_tx=state.db.begin().await?;
    sqlx::query("INSERT INTO dns_changes(id,group_id,actor,old_node,new_node,intent,state) VALUES($1,$2,$3,$4,$5,$6,'writing')").bind(change).bind(id).bind(actor).bind(old).bind(target).bind(&intent).execute(&mut *intent_tx).await?;
    if let Some((rotation,_))=rotation {
        sqlx::query("UPDATE ip_rotations SET state='dns_pending',dns_change=$2,updated_at=now() WHERE id=$1 AND state='queued'").bind(rotation).bind(change).execute(&mut *intent_tx).await?;
    }
    sqlx::query("UPDATE failover_groups SET status='dns_write_pending' WHERE id=$1").bind(id).execute(&mut *intent_tx).await?;intent_tx.commit().await?;
    let write=async {
        for (_,record) in intent.as_object().ok_or_else(||anyhow::anyhow!("invalid intent"))? {
            let path=format!("/zones/{}/dns_records/{}",document["zone_id"].as_str().ok_or_else(||anyhow::anyhow!("missing zone"))?,record["id"].as_str().ok_or_else(||anyhow::anyhow!("missing record"))?);
            cloudflare(state,&token,reqwest::Method::PATCH,&path,Some(&json!({"content":record["content"],"ttl":60,"proxied":false}))).await?;
        }
        let actual=read_records(state,&document,&token).await?;
        anyhow::ensure!(matches_intent(&actual,&intent),"DNS verification mismatch");Ok::<Value,anyhow::Error>(actual)
    }.await;
    match write {
        Ok(actual)=>finish(state,change,id,target,actual,actor).await,
        Err(_)=>{
            sqlx::query("UPDATE dns_changes SET state='unknown',result=$2 WHERE id=$1").bind(change).bind(json!({"reason":"write_or_verification_failed"})).execute(&state.db).await?;
            sqlx::query("UPDATE failover_groups SET status='dns_outcome_unknown' WHERE id=$1").bind(id).execute(&state.db).await?;
            audit(state,actor,"dns.unknown",&change.to_string(),json!({"group":id})).await?;
            Err(ApiError(axum::http::StatusCode::BAD_GATEWAY,"dns_outcome_unknown"))
        }
    }
}
pub(crate) fn matches_intent(actual:&Value,intent:&Value)->bool {
    intent.as_object().is_some_and(|records| !records.is_empty()&&records.iter().all(|(family,record)| actual[family]["id"]==record["id"]&&actual[family]["content"]==record["content"]&&actual[family]["ttl"]==60))
}
async fn finish(state:&State,change:Uuid,group:Uuid,target:Uuid,actual:Value,actor:Option<Uuid>)->Result<()> {
    let mut tx=state.db.begin().await?;
    crate::rotation::finish_dns(&mut tx,change).await?;
    for (family,key) in [("A","ipv4"),("AAAA","ipv6")] {if let Some(ip)=actual[family]["content"].as_str(){
        sqlx::query("UPDATE nodes SET document=jsonb_set(document,$2,$3) WHERE id=$1").bind(target).bind(vec![key]).bind(json!(ip)).execute(&mut *tx).await?;
    }}
    sqlx::query("UPDATE dns_changes SET state='verified',result=$2 WHERE id=$1").bind(change).bind(&actual).execute(&mut *tx).await?;
    sqlx::query("UPDATE failover_groups SET active_node=$2,observed=$3,last_switch=now(),status='healthy',updated_at=now() WHERE id=$1").bind(group).bind(target).bind(actual).execute(&mut *tx).await?;
    let document:Value=sqlx::query_scalar("SELECT document FROM failover_groups WHERE id=$1").bind(group).fetch_one(&mut *tx).await?;
    for key in ["record_a_id","record_aaaa_id"] {if let Some(record)=document[key].as_str().filter(|record|!record.is_empty()) {
        sqlx::query("INSERT INTO dns_record_cooldown(zone_id,record_id,last_switch) VALUES($1,$2,now()) ON CONFLICT(zone_id,record_id) DO UPDATE SET last_switch=now()").bind(document["zone_id"].as_str().ok_or_else(bad)?).bind(record).execute(&mut *tx).await?;
    }}
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'dns.verified',$2,$3)").bind(actor).bind(change.to_string()).bind(json!({"group":group,"node":target})).execute(&mut *tx).await?;tx.commit().await?;Ok(())
}
pub(crate) async fn recover(state:&State,group_id:Uuid)->Result<()> {
    let mut connection=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *connection).await?;
    if !locked{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_coordinator_busy"));}
    let result=async {
        let rows=sqlx::query("SELECT * FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown') ORDER BY created_at LIMIT 1").bind(group_id).fetch_all(&state.db).await?;
        for row in rows {let group=load(state,group_id).await?;let doc:Value=group.get("document");let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{group_id}"))?;
            let actual=read_records(state,&doc,&token).await?;let intent:Value=row.get("intent");
            if matches_intent(&actual,&intent){finish(state,row.get("id"),group_id,row.get("new_node"),actual,row.get("actor")).await?;}
            else{sqlx::query("UPDATE dns_changes SET state='unknown',result=$2 WHERE id=$1").bind(row.get::<Uuid,_>("id")).bind(json!({"reason":"unresolved_partial_or_external_change","actual":actual})).execute(&state.db).await?;}
        }Ok::<_,ApiError>(())
    }.await;
    connection.rollback().await?;result
}



/// An owner explicitly acknowledges the displayed current state before completing a conflicted change.
pub(super) async fn repair(state:&State,id:Uuid,target:Uuid,expected:&Value,actor:Uuid)->Result<()> {
    let mut connection=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *connection).await?;
    if !locked{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_coordinator_busy"));}
    let result=async {
        guard_pool(state,id).await?;
        let group=load(state,id).await?;let document:Value=group.get("document");eligible(state,&group,target).await?;
        let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{id}"))?;
        let actual=read_records(state,&document,&token).await?;
        if &actual!=expected{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_preview_changed"));}
        if fresh(group.get("last_switch"),300)||record_cooldown(state,&group.get::<Value,_>("document")).await?{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_cooldown"));}
        let mut tx=state.db.begin().await?;
        sqlx::query("UPDATE failover_groups SET observed=$2,automatic=false WHERE id=$1").bind(id).bind(actual).execute(&mut *tx).await?;
        sqlx::query("UPDATE dns_changes SET state='failed',result=$2 WHERE group_id=$1 AND state IN ('writing','unknown')").bind(id).bind(json!({"reason":"superseded_by_owner_repair","actor":actor})).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'dns.repair_acknowledged',$2,$3)").bind(actor).bind(id.to_string()).bind(json!({"node":target})).execute(&mut *tx).await?;tx.commit().await?;
        write_owned(state,id,target,Some(actor),None).await
    }.await;
    connection.rollback().await?;result
}


async fn guard_pool(state:&State,id:Uuid)->Result<()> {
    let owned:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM ip_pools p JOIN ip_rotations r ON r.pool_id=p.id WHERE p.group_id=$1 AND r.state IN('queued','dns_pending','unknown'))").bind(id).fetch_one(&state.db).await?;
    if owned{return Err(ApiError(axum::http::StatusCode::CONFLICT,"ip_rotation_pending_requires_reconciliation"));}Ok(())
}

#[cfg(test)]mod tests{use super::*;#[test]fn partial_dns_write_does_not_verify(){let intent=json!({"A":{"id":"a","content":"192.0.2.2"},"AAAA":{"id":"b","content":"2001:db8::2"}});let actual=json!({"A":{"id":"a","content":"192.0.2.2","ttl":60},"AAAA":{"id":"b","content":"2001:db8::1","ttl":60}});assert!(!matches_intent(&actual,&intent));assert!(!matches_intent(&json!({}),&json!({})));}}
