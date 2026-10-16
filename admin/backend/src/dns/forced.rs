//! Owner-forced server selection bypasses runtime state while retaining DNS write fencing.
use super::*;

/// Owner-only requests serialize with automatic DNS and local IP rotation.
pub(super) async fn route(state:&State,user:&Session,id:Uuid,action:&str,body:&Value)->Result<Json<Value>> {
    user.require("owner")?;
    let target=parse_id(field(body,"node_id")?)?;
    let mut tx=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *tx).await?;
    if !locked{return Err(ApiError(StatusCode::CONFLICT,"dns_coordinator_busy"));}
    let result=if action=="force-preview" {
        let group=load(state,id).await?;
        preview(state,&group,target).await.map(|value|Json(ok(value)))
    }else {
        let expected=body.get("preview").ok_or_else(bad)?;
        provider::force_locked(state,id,target,user.id,expected).await.map(|()|Json(ok(json!({}))))
    };
    tx.rollback().await?;
    result
}

/// The coordinator lock must be held through preview comparison and intent persistence.
pub(super) async fn preview(state:&State,group:&sqlx::postgres::PgRow,target:Uuid)->Result<Value> {
    let id:Uuid=group.get("id");let active:Option<Uuid>=group.get("active_node");
    provider::guard_pool(state,id).await?;
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown'))").bind(id).fetch_one(&state.db).await?;
    if pending{return Err(ApiError(StatusCode::CONFLICT,"dns_outcome_unknown"));}
    let document:Value=group.get("document");
    let intent=provider::eligible_mode(state,group,target,true).await?;
    if active==Some(target)&&provider::matches_intent(&group.get::<Value,_>("observed"),&intent){return Err(ApiError(StatusCode::CONFLICT,"dns_target_already_active"));}
    let token=state.vault.open(&group.get::<String,_>("token_cipher"),&format!("dns:{id}"))?;
    let observed=provider::read_records(state,&document,&token).await?;
    if observed!=group.get::<Value,_>("observed"){return Err(ApiError(StatusCode::CONFLICT,"external_dns_conflict"));}
    // Synchronization can progress independently; confirmation binds only DNS ownership and addresses.
    let configuration=json!({"domain":document["domain"],"zone_id":document["zone_id"],"record_a_id":document["record_a_id"],"record_aaaa_id":document["record_aaaa_id"],"node_ids":document["node_ids"],"profile_id":document["profile_id"]});
    Ok(json!({"group_id":id,"old_node":active,"node_id":target,"intent":intent,"observed":observed,"configuration":configuration}))
}
