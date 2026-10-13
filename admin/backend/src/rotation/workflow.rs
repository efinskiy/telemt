use super::*;
use crate::probes::Health;

pub(super) async fn coordinator(state:&State)->Result<sqlx::Transaction<'_,sqlx::Postgres>> {
    let mut tx=state.db.begin().await?;let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *tx).await?;
    if !locked{return Err(conflict("dns_coordinator_busy"));}Ok(tx)
}
async fn no_operations(state:&State)->Result<()> {
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM operations WHERE state IN('queued','running','unknown') AND (state<>'unknown' OR results @> '[{\"state\":\"unknown\"}]'::jsonb)").fetch_one(&state.db).await?;
    // The encrypted fleet intents are not indexed by target; conservatively fence the entire fleet.
    if count>0{return Err(conflict("configuration_operation_pending_or_unknown"));}Ok(())
}
pub(super) async fn ready(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let node_id:Uuid=pool.get("node_id");let owning=node(state,node_id).await?;let doc:Value=owning.get("document");
    let group=crate::dns::load(state,pool.get("group_id")).await?;let group_doc:Value=group.get("document");
    if !owning.get::<bool,_>("enabled")||group.get::<Option<Uuid>,_>("active_node")!=Some(node_id)||group.get::<bool,_>("automatic")||group_doc["configuration_verified"]!=true{return Err(conflict("rotation_group_or_configuration_not_ready"));}
    let enabled_owner:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE id=$1 AND role='owner' AND enabled)").bind(pool.get::<Uuid,_>("owner_id")).fetch_one(&state.db).await?;
    if !enabled_owner{return Err(conflict("rotation_owner_revoked"));}
    let assigned=inventory(state,node_id).await?;addresses(&pool.get::<Value,_>("addresses"),&assigned)?;
    let port=doc["port"].as_u64().ok_or_else(bad)?;
    for (key,record) in [("ipv4","record_a_id"),("ipv6","record_aaaa_id")] {if group_doc[record].as_str().is_some_and(|id|!id.is_empty())&&!has_listener(&assigned,doc[key].as_str().ok_or_else(bad)?,port){return Err(conflict("active_ip_not_listening"));}}
    let (status,compat)=node_request(state,node_id,reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;
    let (ready_status,readiness)=node_request(state,node_id,reqwest::Method::GET,"/v1/health/ready",None,None).await?;
    if status!=200||ready_status!=200||readiness["data"]["ready"]!=true||compat["data"]["fingerprint"]!=group_doc["fingerprint"]||doc["port"]!=group_doc["port"]||doc["sni"]!=group_doc["sni"]{return Err(conflict("rotation_node_not_ready_or_compatible"));}
    Ok(assigned)
}
pub(super) async fn queue_patch(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,state:&State,node_id:Uuid,actor:Uuid,listeners:Value,revision:&str)->Result<Uuid> {
    let id=Uuid::new_v4();let request=json!({"node_ids":[node_id],"method":"PATCH","path":"/v1/config?reload=drain&timeout_secs=300","body":{"server":{"listeners":listeners}},"revisions":{node_id.to_string():revision}});
    let cipher=state.vault.seal(&request.to_string(),&format!("operation:{id}"))?;
    sqlx::query("INSERT INTO operations(id,actor,state,request_cipher) VALUES($1,$2,'queued',$3)").bind(id).bind(actor).bind(cipher).execute(&mut **tx).await?;Ok(id)
}
pub(super) async fn prepare(state:&State,id:Uuid,actor:Uuid,revision:&str)->Result<()> {
    let mut tx=coordinator(state).await?;let pool=load(state,id).await?;let node_id:Uuid=pool.get("node_id");no_operations(state).await?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('queued','dns_pending','retiring','unknown')").bind(id).fetch_one(&mut *tx).await?;if pending>0{return Err(conflict("rotation_pending"));}
    let assigned=inventory(state,node_id).await?;addresses(&pool.get::<Value,_>("addresses"),&assigned)?;
    let (status,saved)=node_request(state,node_id,reqwest::Method::GET,"/v1/config",None,None).await?;
    if status!=200||saved["revision"]!=revision||assigned["revision"]!=revision{return Err(conflict("revision_conflict"));}
    if saved.pointer("/data/server/listeners")!=Some(&assigned["active_listeners"]){return Err(conflict("saved_and_active_listeners_differ"));}
    let listeners=listener_plan(&assigned["active_listeners"],&pool.get::<Value,_>("addresses"),&listener_document(state,&pool).await?,None)?;
    let operation=queue_patch(&mut tx,state,node_id,actor,listeners,revision).await?;
    sqlx::query("UPDATE ip_pools SET preparation=$2,automatic=false,status='preparing',updated_at=now() WHERE id=$1").bind(id).bind(operation).execute(&mut *tx).await?;
    audit_tx(&mut tx,actor,"ip_pool.prepare",id,json!({"operation":operation,"dns_unchanged":true})).await?;tx.commit().await?;Ok(())
}
pub(super) async fn enqueue(state:&State,id:Uuid,new_ip:&str,actor:Option<Uuid>,reason:&str,preview:Option<&Value>)->Result<Uuid> {
    let mut tx=coordinator(state).await?;let pool=load(state,id).await?;let node_id:Uuid=pool.get("node_id");no_operations(state).await?;
    if actor.is_none()&&!pool.get::<bool,_>("automatic"){return Err(conflict("ip_rotation_disabled"));}
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
        sqlx::query("UPDATE ip_rotations SET state='retiring',updated_at=now() WHERE id=$1").bind(rotation.get::<Uuid,_>("id")).execute(&mut **tx).await?;
        sqlx::query("UPDATE ip_pools SET last_rotation=now(),status='retiring_old_listener' WHERE id=$1").bind(rotation.get::<Uuid,_>("pool_id")).execute(&mut **tx).await?;
        sqlx::query("INSERT INTO address_health(node_id,address,status) VALUES($1,$2,'failed') ON CONFLICT(node_id,address) DO UPDATE SET status='failed'").bind(node_id).bind(old).execute(&mut **tx).await?;
    }Ok(())
}

async fn pending_step(state:&State,id:Uuid)->Result<()> {
    let row=sqlx::query("SELECT * FROM ip_rotations WHERE id=$1").bind(id).fetch_one(&state.db).await?;let pool=load(state,row.get("pool_id")).await?;
    if row.get::<String,_>("state")=="dns_pending" {crate::dns::provider::recover(state,pool.get("group_id")).await?;return Ok(());}
    let mut tx=coordinator(state).await?;let row=sqlx::query("SELECT * FROM ip_rotations WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
    let pool=load(state,row.get("pool_id")).await?;let node_id:Uuid=pool.get("node_id");let doc:Value=node(state,node_id).await?.get("document");let new_ip:String=row.get("new_ip");
    match row.get::<String,_>("state").as_str() {
        "queued"=>{
            if row.get::<Option<Uuid>,_>("actor").is_none()&&!pool.get::<bool,_>("automatic"){sqlx::query("UPDATE ip_rotations SET state='failed',result='{\"reason\":\"automation_disabled_before_dns_write\"}' WHERE id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;return Ok(());}
            no_operations(state).await?;let assigned=ready(state,&pool).await?;
            let family:i32=row.get("family");let old:String=row.get("old_ip");
            if doc[if family==4{"ipv4"}else{"ipv6"}]!=old||!has_listener(&assigned,&new_ip,doc["port"].as_u64().ok_or_else(bad)?)||!matches!(health(state,node_id,&new_ip).await?,Health::Healthy|Health::Recovered){return Err(conflict("rotation_prerequisites_changed"));}
            if row.get::<Option<Uuid>,_>("actor").is_none()&&health(state,node_id,&old).await?!=Health::Failed{return Err(conflict("current_ip_failure_not_confirmed"));}
            let group=crate::dns::load(state,pool.get("group_id")).await?;let group_doc:Value=group.get("document");let observed:Value=group.get("observed");let mut intent=json!({});
            for (key,version) in [("record_a_id","A"),("record_aaaa_id","AAAA")] {if let Some(record)=group_doc[key].as_str().filter(|s|!s.is_empty()){intent[version]=json!({"id":record,"content":if (version=="A")== (family==4){json!(new_ip)}else{observed[version]["content"].clone()},"ttl":60,"proxied":false});}}
            crate::dns::provider::rotate_locked(state,pool.get("group_id"),node_id,row.get("actor"),id,intent).await?;
        }
        "retiring"=>{
            let old:String=row.get("old_ip");
            if let Some(operation)=row.get::<Option<Uuid>,_>("retirement") {
                let status:String=sqlx::query_scalar("SELECT state FROM operations WHERE id=$1").bind(operation).fetch_one(&mut *tx).await?;
                if status=="succeeded" {let assigned=inventory(state,node_id).await?;if has_listener(&assigned,&old,doc["port"].as_u64().ok_or_else(bad)?)||!has_listener(&assigned,&new_ip,doc["port"].as_u64().ok_or_else(bad)?){return Err(conflict("listener_retirement_not_observed"));}
                    verify_fingerprint(state,&pool).await?;complete(&mut tx,id,pool.get("id"),json!({"dns_verified":true,"old_listener_retired":true,"reload_completed":true})).await?;
                }else if ["failed","partial","unknown"].contains(&status.as_str()){sqlx::query("UPDATE ip_rotations SET state='unknown',result=$2 WHERE id=$1").bind(id).bind(json!({"reason":"listener_retirement_unconfirmed","operation":operation})).execute(&mut *tx).await?;}
            }else{
                no_operations(state).await?;let assigned=inventory(state,node_id).await?;let (status,saved)=node_request(state,node_id,reqwest::Method::GET,"/v1/config",None,None).await?;
                if status!=200||saved.pointer("/data/server/listeners")!=Some(&assigned["active_listeners"]){return Err(conflict("saved_and_active_listeners_differ"));}
                let plan=listener_plan(&assigned["active_listeners"],&pool.get::<Value,_>("addresses"),&doc,Some(&old))?;
                let operation=queue_patch(&mut tx,state,node_id,pool.get("owner_id"),plan,saved["revision"].as_str().ok_or_else(bad)?).await?;
                sqlx::query("UPDATE ip_rotations SET retirement=$2 WHERE id=$1").bind(id).bind(operation).execute(&mut *tx).await?;
            }
        }
        _=>{}
    }tx.commit().await?;Ok(())
}
async fn verify_fingerprint(state:&State,pool:&sqlx::postgres::PgRow)->Result<()> {
    let (status,compat)=node_request(state,pool.get("node_id"),reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;let group=crate::dns::load(state,pool.get("group_id")).await?;
    if status!=200||compat["data"]["fingerprint"]!=group.get::<Value,_>("document")["fingerprint"]{return Err(conflict("configuration_verification_required"));}Ok(())
}
async fn complete(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,id:Uuid,pool:Uuid,result:Value)->Result<()> {
    sqlx::query("UPDATE ip_rotations SET state='succeeded',result=$2,updated_at=now() WHERE id=$1").bind(id).bind(&result).execute(&mut **tx).await?;
    // Endpoint-only preparation was checked against the immutable compatibility fingerprint.
    sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{configuration_verified}','true') WHERE id=(SELECT group_id FROM ip_pools WHERE id=$1)").bind(pool).execute(&mut **tx).await?;
    sqlx::query("UPDATE ip_pools SET status='rotated',updated_at=now() WHERE id=$1").bind(pool).execute(&mut **tx).await?;
    sqlx::query("INSERT INTO audit(action,target,outcome) VALUES('ip.rotate.verified',$1,$2)").bind(id.to_string()).bind(result).execute(&mut **tx).await?;Ok(())
}

pub(super) async fn reconcile_retirement(state:&State,pool:&sqlx::postgres::PgRow,actor:Uuid)->Result<()> {
    let mut tx=coordinator(state).await?;let rows=sqlx::query("SELECT * FROM ip_rotations WHERE pool_id=$1 AND state='unknown' AND retirement IS NOT NULL LIMIT 1").bind(pool.get::<Uuid,_>("id")).fetch_all(&mut *tx).await?;
    for row in rows {let node_id:Uuid=pool.get("node_id");let assigned=inventory(state,node_id).await?;let doc:Value=node(state,node_id).await?.get("document");
        let port=doc["port"].as_u64().ok_or_else(bad)?;
        if has_listener(&assigned,&row.get::<String,_>("old_ip"),port)||!has_listener(&assigned,&row.get::<String,_>("new_ip"),port){return Err(conflict("listener_retirement_not_observed"));}
        let (status,compat)=node_request(state,node_id,reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;let group=crate::dns::load(state,pool.get("group_id")).await?;
        if status!=200||compat["data"]["fingerprint"]!=group.get::<Value,_>("document")["fingerprint"]{return Err(conflict("configuration_verification_required"));}
        complete(&mut tx,row.get("id"),pool.get("id"),json!({"dns_verified":true,"old_listener_retired":true,"reload_completed":null,"observed_by_owner":actor})).await?;
    }tx.commit().await?;Ok(())
}

pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    tick_inner(state).await.map_err(|error|anyhow::anyhow!(error.1))
}
async fn tick_inner(state:&State)->Result<()> {
    let pending:Vec<Uuid>=sqlx::query_scalar("SELECT id FROM ip_rotations WHERE state IN('queued','dns_pending','retiring') ORDER BY created_at LIMIT 20").fetch_all(&state.db).await?;
    for id in pending {if let Err(error)=pending_step(state,id).await {sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=(SELECT pool_id FROM ip_rotations WHERE id=$1)").bind(id).bind(error.1).execute(&state.db).await?;}}
    let pools=sqlx::query("SELECT * FROM ip_pools ORDER BY id LIMIT 20").fetch_all(&state.db).await?;
    for pool in pools {let id:Uuid=pool.get("id");let node_id:Uuid=pool.get("node_id");
        if pool.get::<String,_>("status")=="preparing" {if let Some(operation)=pool.get::<Option<Uuid>,_>("preparation") {let status:String=sqlx::query_scalar("SELECT state FROM operations WHERE id=$1").bind(operation).fetch_one(&state.db).await?;
            if status=="succeeded" {let mut tx=coordinator(state).await?;let (status,compat)=node_request(state,node_id,reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;let group=crate::dns::load(state,pool.get("group_id")).await?;
                if status==200&&compat["data"]["fingerprint"]==group.get::<Value,_>("document")["fingerprint"]{sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{configuration_verified}','true'),automatic=false WHERE id=$1").bind(pool.get::<Uuid,_>("group_id")).execute(&mut *tx).await?;sqlx::query("UPDATE ip_pools SET status='prepared' WHERE id=$1").bind(id).execute(&mut *tx).await?;}
                tx.commit().await?;
            }else if ["failed","unknown","partial"].contains(&status.as_str()){sqlx::query("UPDATE ip_pools SET status='preparation_failed_or_unknown' WHERE id=$1").bind(id).execute(&state.db).await?;}
        }}
        if !pool.get::<bool,_>("automatic"){continue;}
        let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('queued','dns_pending','retiring','unknown')").bind(id).fetch_one(&state.db).await?;if pending>0{continue;}
        if fresh(pool.get("last_rotation"),300){sqlx::query("UPDATE ip_pools SET status='cooldown' WHERE id=$1").bind(id).execute(&state.db).await?;continue;}
        let assigned=match ready(state,&pool).await{Ok(assigned)=>assigned,Err(error)=>{sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=$1").bind(id).bind(error.1).execute(&state.db).await?;continue;}};
        let doc:Value=node(state,node_id).await?.get("document");let selected:Value=pool.get("addresses");let mut failed=false;let mut unknown=false;
        let group_doc:Value=crate::dns::load(state,pool.get("group_id")).await?.get("document");
        for (key,record) in [("ipv4","record_a_id"),("ipv6","record_aaaa_id")] {if group_doc[record].as_str().is_none_or(str::is_empty){continue;}
            if let Some(ip)=doc[key].as_str().filter(|ip|!ip.is_empty()){let current=health(state,node_id,ip).await?;failed|=current==Health::Failed;unknown|=current==Health::Unknown;}}
        let mut status=if unknown{"probe_state_unknown"}else if failed{"no_healthy_ip_reserve"}else{"healthy"};
        if failed&&!unknown {for ip in selected.as_array().ok_or_else(bad)? {let Some(ip)=ip.as_str()else{continue;};let family=ip.parse::<std::net::IpAddr>().map_err(|_|bad())?.is_ipv4();let key=if family{"ipv4"}else{"ipv6"};let Some(old)=doc[key].as_str()else{continue;};if old==ip||!has_listener(&assigned,ip,doc["port"].as_u64().ok_or_else(bad)?)||health(state,node_id,old).await?!=Health::Failed{continue;}
            if matches!(health(state,node_id,ip).await?,Health::Healthy|Health::Recovered) {match enqueue(state,id,ip,None,"three_failed_rounds_two_independent_networks",None).await{Ok(_)=>status="rotation_queued",Err(error)=>status=error.1,}break;}
        }}
        sqlx::query("UPDATE ip_pools SET status=$2 WHERE id=$1 AND automatic").bind(id).bind(status).execute(&state.db).await?;
    }Ok(())
}

pub(super) async fn listener_document(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let mut document:Value=node(state,pool.get("node_id")).await?.get("document");let group=crate::dns::load(state,pool.get("group_id")).await?;document["announce_domain"]=group.get::<Value,_>("document")["domain"].clone();Ok(document)
}
