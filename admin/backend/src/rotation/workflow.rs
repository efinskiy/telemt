use super::*;
use crate::probes::Health;

/// Serialize DNS and listener mutations with the existing group coordinator.
pub(super) async fn coordinator(state:&State)->Result<sqlx::Transaction<'_,sqlx::Postgres>> {
    let mut tx=state.db.begin().await?;let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *tx).await?;
    if !locked{return Err(conflict("dns_coordinator_busy"));}Ok(tx)
}
/// Validate managed listeners, ownership and compatibility before admitting a rotation.
pub(super) async fn ready(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let node_id:Uuid=pool.get("node_id");let owning=node(state,node_id).await?;let doc:Value=owning.get("document");
    let group=crate::dns::load(state,pool.get("group_id")).await?;let group_doc:Value=group.get("document");
    selection::same_profile(state,&group,node_id).await?;
    // A deleted active node can be repaired manually; compatibility and readiness still apply.
    if !owning.get::<bool,_>("enabled")||(group_doc["configuration_verified"]!=true&&group.get::<Option<Uuid>,_>("active_node").is_some()){return Err(conflict("rotation_group_or_configuration_not_ready"));}
    let enabled_owner:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE id=$1 AND role='owner' AND enabled)").bind(pool.get::<Uuid,_>("owner_id")).fetch_one(&state.db).await?;
    if !enabled_owner{return Err(conflict("rotation_owner_revoked"));}
    let assigned=prepared(state,pool).await?;let port=doc["port"].as_u64().ok_or_else(bad)?;
    for (key,record) in [("ipv4","record_a_id"),("ipv6","record_aaaa_id")] {if group_doc[record].as_str().is_some_and(|id|!id.is_empty())&&!has_listener(&assigned,doc[key].as_str().ok_or_else(bad)?,port){return Err(conflict("active_ip_not_listening"));}}
    let (status,compat)=node_request(state,node_id,reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;
    let (ready_status,readiness)=node_request(state,node_id,reqwest::Method::GET,"/v1/health/ready",None,None).await?;
    if status!=200||ready_status!=200||readiness["data"]["ready"]!=true||compat["data"]["fingerprint"]!=group_doc["fingerprint"]||doc["port"]!=group_doc["port"]||doc["sni"]!=group_doc["sni"]{return Err(conflict("rotation_node_not_ready_or_compatible"));}
    Ok(assigned)
}
// Completing listener preparation must not wait for public probes or DNS-group admission.
async fn prepared(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let node_id:Uuid=pool.get("node_id");let doc:Value=node(state,node_id).await?.get("document");
    if !crate::managed::admission::ready(state,node_id).await? {return Err(conflict("managed_node_not_ready"));}
    let deployment=sqlx::query("SELECT d.id,d.bundle_cipher FROM deployments d JOIN managed_nodes m ON m.desired_revision=d.id WHERE m.node_id=$1 AND m.applied_revision=d.id").bind(node_id).fetch_optional(&state.db).await?.ok_or(conflict("reserve_ip_not_prepared"))?;
    let revision:i64=deployment.get("id");let raw=state.vault.open(&deployment.get::<String,_>("bundle_cipher"),&format!("deployment:{node_id}:{revision}"))?;
    let bundle:telemt_managed::Bundle=serde_json::from_str(&raw).map_err(|_|bad())?;
    if !pool.get::<Value,_>("addresses").as_array().ok_or_else(bad)?.iter().all(|ip|ip.as_str().is_some_and(|ip|bundle.public_ips.iter().any(|bound|bound==ip))){return Err(conflict("reserve_ip_not_prepared"));}
    let assigned=inventory(state,node_id).await?;addresses(&pool.get::<Value,_>("addresses"),&assigned)?;
    let port=doc["port"].as_u64().ok_or_else(bad)?;
    for ip in pool.get::<Value,_>("addresses").as_array().ok_or_else(bad)? {if !has_listener(&assigned,ip.as_str().ok_or_else(bad)?,port){return Err(conflict("reserve_ip_not_prepared"));}}
    Ok(assigned)
}
/// Stage a shared-profile deployment while leaving DNS unchanged.
pub(super) async fn prepare(state:&State,id:Uuid,actor:Uuid,revision:&str)->Result<()> {
    let mut tx=coordinator(state).await?;let pool=load(state,id).await?;let node_id:Uuid=pool.get("node_id");
    guard_node(state,node_id).await?;
    let assigned=inventory(state,node_id).await?;addresses(&pool.get::<Value,_>("addresses"),&assigned)?;
    if assigned["revision"]!=revision{return Err(conflict("revision_conflict"));}
    let deployment=crate::profiles::enqueue(state,&mut tx,node_id,Some(actor)).await?;
    sqlx::query("UPDATE ip_pools SET deployment=$2,automatic=false,status='preparing',preparation_error=NULL,preparation_checked_at=NULL,updated_at=now() WHERE id=$1").bind(id).bind(deployment).execute(&mut *tx).await?;
    audit_tx(&mut tx,actor,"ip_pool.prepare",id,json!({"deployment":deployment,"dns_unchanged":true,"restart_required":true})).await?;tx.commit().await?;Ok(())
}
/// Persist a rotation only after rechecking the reviewed target and failure policy.
pub(super) async fn enqueue(state:&State,id:Uuid,new_ip:&str,actor:Option<Uuid>,reason:&str,preview:Option<&Value>)->Result<Uuid> {
    let mut tx=coordinator(state).await?;let pool=load(state,id).await?;let node_id:Uuid=pool.get("node_id");
    if actor.is_none()&&!pool.get::<bool,_>("automatic"){return Err(conflict("ip_rotation_disabled"));}
    if crate::dns::load(state,pool.get("group_id")).await?.get::<Option<Uuid>,_>("active_node")!=Some(node_id){return Err(conflict("rotation_requires_active_node"));}
    let assigned=ready(state,&pool).await?;let selected:Value=pool.get("addresses");
    if !selected.as_array().is_some_and(|ips|ips.iter().any(|ip|ip==new_ip)){return Err(bad());}
    let family=if new_ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4(){4}else{6};let doc:Value=node(state,node_id).await?.get("document");
    let old_ip=doc[if family==4{"ipv4"}else{"ipv6"}].as_str().ok_or_else(bad)?;
    if let Some(preview)=preview {let group=crate::dns::load(state,pool.get("group_id")).await?;if preview["old_ip"]!=old_ip||preview["expected_records"]!=group.get::<Value,_>("observed"){return Err(conflict("rotation_preview_changed"));}}
    if new_ip==old_ip||!has_listener(&assigned,new_ip,doc["port"].as_u64().ok_or_else(bad)?){return Err(conflict("reserve_ip_not_prepared"));}
    if fresh(pool.get("last_rotation"),300){return Err(conflict("ip_rotation_cooldown"));}
    if !matches!(health(state,node_id,new_ip).await?,Health::Healthy|Health::Recovered){return Err(conflict("reserve_ip_without_probe_quorum"));}
    if actor.is_none()&&health(state,node_id,old_ip).await?!=Health::Failed{return Err(conflict("current_ip_failure_not_confirmed"));}
    let id=Uuid::new_v4();sqlx::query("INSERT INTO ip_rotations(id,pool_id,actor,old_ip,new_ip,family,state,reason) VALUES($1,$2,$3,$4,$5,$6,'queued',$7)").bind(id).bind(pool.get::<Uuid,_>("id")).bind(actor).bind(old_ip).bind(new_ip).bind(family).bind(reason).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'ip.rotate.submit',$2,$3)").bind(actor).bind(id.to_string()).bind(json!({"pool":pool.get::<Uuid,_>("id"),"old_ip":old_ip,"new_ip":new_ip,"reason":reason})).execute(&mut *tx).await?;tx.commit().await?;Ok(id)
}

/// DNS verification and the controller's new node address commit together, including crash recovery.
pub(crate) async fn finish_dns(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,change:Uuid)->Result<()> {
    let rotation=sqlx::query("SELECT r.*,p.node_id FROM ip_rotations r JOIN ip_pools p ON p.id=r.pool_id WHERE r.dns_change=$1 AND r.state='dns_pending' FOR UPDATE OF r").bind(change).fetch_optional(&mut **tx).await?;
    if let Some(rotation)=rotation {let family:i32=rotation.get("family");let node_id:Uuid=rotation.get("node_id");let ip:String=rotation.get("new_ip");let old:String=rotation.get("old_ip");
        sqlx::query("UPDATE nodes SET document=jsonb_set(jsonb_set(document,$2,$3),'{ip_rotated}','true'),collected_at=NULL WHERE id=$1").bind(node_id).bind(vec![if family==4{"ipv4"}else{"ipv6"}]).bind(json!(ip)).execute(&mut **tx).await?;
        sqlx::query("UPDATE ip_rotations SET state='succeeded',result='{\"dns_verified\":true,\"listeners_retained\":true}',updated_at=now() WHERE id=$1").bind(rotation.get::<Uuid,_>("id")).execute(&mut **tx).await?;
        sqlx::query("UPDATE ip_pools SET last_rotation=now(),status='rotated' WHERE id=$1").bind(rotation.get::<Uuid,_>("pool_id")).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO address_health(node_id,address,status) VALUES($1,$2,'failed') ON CONFLICT(node_id,address) DO UPDATE SET status='failed'").bind(node_id).bind(old).execute(&mut **tx).await?;
    }Ok(())
}

async fn pending_step(state:&State,id:Uuid)->Result<()> {
    let row=sqlx::query("SELECT * FROM ip_rotations WHERE id=$1").bind(id).fetch_one(&state.db).await?;let pool=load(state,row.get("pool_id")).await?;
    if row.get::<String,_>("state")=="dns_pending" {crate::dns::provider::recover(state,pool.get("group_id")).await?;return Ok(());}
    let mut tx=coordinator(state).await?;let row=sqlx::query("SELECT * FROM ip_rotations WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
    let pool=load(state,row.get("pool_id")).await?;let node_id:Uuid=pool.get("node_id");let doc:Value=node(state,node_id).await?.get("document");let new_ip:String=row.get("new_ip");
    if row.get::<String,_>("state")=="queued" {
            if crate::dns::load(state,pool.get("group_id")).await?.get::<Option<Uuid>,_>("active_node")!=Some(node_id){return Err(conflict("rotation_requires_active_node"));}
            if row.get::<Option<Uuid>,_>("actor").is_none()&&!pool.get::<bool,_>("automatic"){sqlx::query("UPDATE ip_rotations SET state='failed',result='{\"reason\":\"automation_disabled_before_dns_write\"}' WHERE id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;return Ok(());}
            let assigned=ready(state,&pool).await?;
            let family:i32=row.get("family");let old:String=row.get("old_ip");
            if doc[if family==4{"ipv4"}else{"ipv6"}]!=old||!has_listener(&assigned,&new_ip,doc["port"].as_u64().ok_or_else(bad)?)||!matches!(health(state,node_id,&new_ip).await?,Health::Healthy|Health::Recovered){return Err(conflict("rotation_prerequisites_changed"));}
            if row.get::<Option<Uuid>,_>("actor").is_none()&&health(state,node_id,&old).await?!=Health::Failed{return Err(conflict("current_ip_failure_not_confirmed"));}
            let group=crate::dns::load(state,pool.get("group_id")).await?;let group_doc:Value=group.get("document");let observed:Value=group.get("observed");let mut intent=json!({});
            for (key,version) in [("record_a_id","A"),("record_aaaa_id","AAAA")] {if let Some(record)=group_doc[key].as_str().filter(|s|!s.is_empty()){intent[version]=json!({"id":record,"content":if (version=="A")== (family==4){json!(new_ip)}else{observed[version]["content"].clone()},"ttl":60,"proxied":false});}}
            crate::dns::provider::rotate_locked(state,pool.get("group_id"),node_id,row.get("actor"),id,intent).await?;
    }tx.commit().await?;Ok(())
}
/// Advance pending operations and reconcile the current automation status.
pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    tick_inner(state).await.map_err(|error|anyhow::anyhow!(error.1))
}
async fn tick_inner(state:&State)->Result<()> {
    let pending:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM ip_rotations WHERE state IN('queued','dns_pending') ORDER BY created_at LIMIT 20").fetch_all(&state.db).await?;
    for id in pending {if let Err(error)=pending_step(state,id).await {sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=(SELECT pool_id FROM ip_rotations WHERE id=$1)").bind(id).bind(error.1).execute(&state.db).await?;}}
    // Selection diagnostics stop being current after failover or an automation pause.
    // Preserve preparation and unresolved-operation errors; this statement never changes eligibility or DNS.
    sqlx::query("UPDATE ip_pools p SET status=CASE WHEN p.automatic THEN 'standby' ELSE 'disabled' END FROM failover_groups g WHERE g.id=p.group_id AND (NOT p.automatic OR g.active_node IS DISTINCT FROM p.node_id) AND p.status IN ('no_healthy_ip_reserve','probe_state_unknown','healthy','cooldown','waiting_for_quorum','standby') AND p.status IS DISTINCT FROM CASE WHEN p.automatic THEN 'standby' ELSE 'disabled' END AND NOT EXISTS(SELECT 1 FROM ip_rotations r WHERE r.pool_id=p.id AND r.state IN ('queued','dns_pending','unknown'))").execute(&state.db).await?;
    let pools=sqlx::query("SELECT * FROM ip_pools ORDER BY id LIMIT 20").fetch_all(&state.db).await?;
    for pool in pools {let id:Uuid=pool.get("id");let node_id:Uuid=pool.get("node_id");
        if pool.get::<String,_>("status")=="preparing" {
            let error=prepared(state,&pool).await.err().map(|error|error.1);
            // A newer preparation must not inherit the result of an older in-flight inventory check.
            sqlx::query("UPDATE ip_pools SET status=CASE WHEN $2::text IS NULL THEN 'prepared' ELSE 'preparing' END,preparation_error=$2,preparation_checked_at=now() WHERE id=$1 AND status='preparing' AND deployment IS NOT DISTINCT FROM $3")
                .bind(id).bind(error).bind(pool.get::<Option<i64>,_>("deployment")).execute(&state.db).await?;
        }
        if crate::dns::load(state,pool.get("group_id")).await?.get::<Option<Uuid>,_>("active_node")!=Some(node_id){continue;}
        if !pool.get::<bool,_>("automatic"){continue;}
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('queued','dns_pending','unknown')").bind(id).fetch_one(&state.db).await?;if pending>0{continue;}
        if fresh(pool.get("last_rotation"),300){sqlx::query("UPDATE ip_pools SET status='cooldown' WHERE id=$1").bind(id).execute(&state.db).await?;continue;}
        let assigned=match ready(state,&pool).await{Ok(assigned)=>assigned,Err(error)=>{sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=$1").bind(id).bind(error.1).execute(&state.db).await?;continue;}};
        let doc:Value=node(state,node_id).await?.get("document");let selected:Value=pool.get("addresses");let mut failed=false;let mut unknown=false;
        let group_doc:Value=crate::dns::load(state,pool.get("group_id")).await?.get("document");
        for (key,record) in [("ipv4","record_a_id"),("ipv6","record_aaaa_id")] {if group_doc[record].as_str().is_none_or(str::is_empty){continue;}
            if let Some(ip)=doc[key].as_str().filter(|ip|!ip.is_empty()){let current=health(state,node_id,ip).await?;failed|=current==Health::Failed;unknown|=current==Health::Unknown;}}
        let mut status=if unknown{"probe_state_unknown"}else if failed{"no_healthy_ip_reserve"}else{"healthy"};
        if failed&&!unknown {for ip in selected.as_array().ok_or_else(bad)? {let Some(ip)=ip.as_str()else{continue;};let family=ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4();let key=if family{"ipv4"}else{"ipv6"};let Some(old)=doc[key].as_str()else{continue;};if old==ip||!has_listener(&assigned,ip,doc["port"].as_u64().ok_or_else(bad)?)||health(state,node_id,old).await?!=Health::Failed{continue;}
            if matches!(health(state,node_id,ip).await?,Health::Healthy|Health::Recovered) {match enqueue(state,id,ip,None,"tcp_tls22_continuous_timeouts_five_minutes",None).await{Ok(_)=>status="rotation_queued",Err(error)=>status=error.1,}break;}
        }}
        sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=$1 AND automatic").bind(id).bind(status).execute(&state.db).await?;
    }Ok(())
}

/// Use the group domain when constructing retained listener announcements.
pub(super) async fn listener_document(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let mut document:Value=node(state,pool.get("node_id")).await?.get("document");let group=crate::dns::load(state,pool.get("group_id")).await?;document["announce_domain"]=group.get::<Value,_>("document")["domain"].clone();Ok(document)
}
