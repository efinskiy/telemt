use super::*;

/// External DNS drift can only be acknowledged against an exact fresh read, with automation stopped.
pub(super) async fn acknowledge_dns(state:&State,id:Uuid,actor:Uuid,expected:&Value)->Result<()> {
    let mut tx=workflow::coordinator(state).await?;let pool=load(state,id).await?;let group=crate::dns::load(state,pool.get("group_id")).await?;
    let document:Value=group.get("document");let group_id:Uuid=pool.get("group_id");let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{group_id}"))?;
    let actual=crate::dns::provider::read_records(state,&document,&token).await?;
    if &actual!=expected{return Err(conflict("dns_preview_changed"));}
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state IN('dns_pending','unknown')").bind(id).fetch_one(&mut *tx).await?;
    if pending>0{return Err(conflict("reconcile_pending_rotation_before_acknowledging_external_dns"));}
    sqlx::query("UPDATE failover_groups SET observed=$2,automatic=false,status='external_dns_acknowledged' WHERE id=$1").bind(group_id).bind(actual).execute(&mut *tx).await?;
    sqlx::query("UPDATE ip_pools SET automatic=false,status='external_dns_acknowledged' WHERE id=$1").bind(id).execute(&mut *tx).await?;
    // An old queued preview must not take effect after acknowledgment; submit a fresh manual preview.
    sqlx::query("UPDATE ip_rotations SET state='failed',result='{\"reason\":\"superseded_by_owner_dns_acknowledgment\"}',updated_at=now() WHERE pool_id=$1 AND state='queued'").bind(id).execute(&mut *tx).await?;
    audit_tx(&mut tx,actor,"ip_pool.dns_acknowledged",id,json!({"expected_records":expected})).await?;tx.commit().await?;Ok(())
}

/// Recovery retries require a fresh owner-reviewed record snapshot and retain the rotation link.
pub(super) async fn dns_preview(state:&State,pool:&sqlx::postgres::PgRow)->Result<Value> {
    let rotation=sqlx::query("SELECT r.*,d.state AS dns_state FROM ip_rotations r JOIN dns_changes d ON d.id=r.dns_change WHERE r.pool_id=$1 AND r.state='dns_pending' AND d.state IN('writing','unknown') LIMIT 1").bind(pool.get::<Uuid,_>("id")).fetch_optional(&state.db).await?.ok_or_else(bad)?;
    let group_id:Uuid=pool.get("group_id");let group=crate::dns::load(state,group_id).await?;let document:Value=group.get("document");let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{group_id}"))?;
    let records=crate::dns::provider::read_records(state,&document,&token).await?;
    Ok(json!({"rotation_id":rotation.get::<Uuid,_>("id"),"dns_change":rotation.get::<Uuid,_>("dns_change"),"old_ip":rotation.get::<String,_>("old_ip"),"new_ip":rotation.get::<String,_>("new_ip"),"family":rotation.get::<i32,_>("family"),"expected_records":records,"ttl":60,"dns_only":true}))
}
pub(super) async fn repair_dns(state:&State,id:Uuid,actor:Uuid,expected:&Value)->Result<()> {
    let mut tx=workflow::coordinator(state).await?;let pool=load(state,id).await?;let preview=dns_preview(state,&pool).await?;
    if &preview!=expected{return Err(conflict("rotation_dns_preview_changed"));}
    let assigned=workflow::ready(state,&pool).await?;let node_id:Uuid=pool.get("node_id");let document:Value=node(state,node_id).await?.get("document");
    let new_ip=preview["new_ip"].as_str().ok_or_else(bad)?;let family=preview["family"].as_i64().ok_or_else(bad)?;
    if document[if family==4{"ipv4"}else{"ipv6"}]!=preview["old_ip"]||!has_listener(&assigned,new_ip,document["port"].as_u64().ok_or_else(bad)?)||!matches!(health(state,node_id,new_ip).await?,crate::probes::Health::Healthy|crate::probes::Health::Recovered){return Err(conflict("rotation_prerequisites_changed"));}
    // An accepted update must be reconciled, never rewritten merely because its response was lost.
    let original_intent:Value=sqlx::query_scalar("SELECT intent FROM dns_changes WHERE id=$1").bind(parse_id(preview["dns_change"].as_str().ok_or_else(bad)?)?).fetch_one(&mut *tx).await?;
    if crate::dns::provider::matches_intent(&preview["expected_records"],&original_intent){return Err(conflict("reconcile_accepted_dns_write_first"));}
    let rotation=parse_id(preview["rotation_id"].as_str().ok_or_else(bad)?)?;let change=parse_id(preview["dns_change"].as_str().ok_or_else(bad)?)?;
    sqlx::query("UPDATE dns_changes SET state='failed',result=$2 WHERE id=$1").bind(change).bind(json!({"reason":"superseded_by_owner_pool_repair","actor":actor})).execute(&mut *tx).await?;
    sqlx::query("UPDATE ip_rotations SET state='queued',dns_change=NULL,actor=$2,reason='owner_dns_repair',result=$3,updated_at=now() WHERE id=$1").bind(rotation).bind(actor).bind(json!({"previous_dns_change":change})).execute(&mut *tx).await?;
    sqlx::query("UPDATE failover_groups SET observed=$2,automatic=false,status='pool_repair_queued' WHERE id=$1").bind(pool.get::<Uuid,_>("group_id")).bind(&preview["expected_records"]).execute(&mut *tx).await?;
    sqlx::query("UPDATE ip_pools SET automatic=false,status='dns_repair_queued' WHERE id=$1").bind(id).execute(&mut *tx).await?;
    audit_tx(&mut tx,actor,"ip.rotate.dns_repair",id,preview).await?;tx.commit().await?;Ok(())
}
