use super::*;

/// Enrollment ownership and cooldown persist even when a group is retired.
pub(super) async fn patch(state:&State,user:&Session,id:Uuid,body:Value)->Result<Json<Value>> {
    user.require("owner")?;
    let keys=body.as_object().ok_or_else(bad)?;
    if keys.is_empty()||keys.keys().any(|key|!["automatic","node_ids","token"].contains(&key.as_str())){return Err(bad());}
    let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
    let group=load(state,id).await?;let mut document:Value=group.get("document");
    let mut changed=false;let mut cipher=group.get::<String,_>("token_cipher");
    if let Some(nodes)=body.get("node_ids") {
        let nodes=nodes.as_array().ok_or_else(bad)?;if !(1..=20).contains(&nodes.len()){return Err(bad());}
        let mut unique=std::collections::HashSet::new();let mut ordered=Vec::new();
        for node in nodes {let node=parse_id(node.as_str().ok_or_else(bad)?)?;if !unique.insert(node){return Err(bad());}crate::state::node(state,node).await?;ordered.push(node);}
        if group.get::<Option<Uuid>,_>("active_node").is_none_or(|active|!unique.contains(&active)){return Err(bad());}
        document["node_ids"]=json!(ordered);changed=true;
    }
    if let Some(token)=body.get("token").and_then(Value::as_str).filter(|token|!token.is_empty()) {
        if token.len()>1024{return Err(bad());}read_records(state,&document,token).await?;
        cipher=state.vault.seal(token,&format!("dns:{id}"))?;changed=true;
    }
    let pools:i64=sqlx::query_scalar("SELECT count(*) FROM ip_pools WHERE group_id=$1").bind(id).fetch_one(&mut *tx).await?;
    if pools>0 && (changed||body["automatic"]==true){return Err(ApiError(axum::http::StatusCode::CONFLICT,"group_owned_by_ip_rotation_pool"));}
    let enabled=body.get("automatic").map(|value|value.as_bool().ok_or_else(bad)).transpose()?.unwrap_or(false);
    if enabled && (changed||document["configuration_verified"]!=true||["configuration_change_requires_verification","endpoint_change_requires_verification"].contains(&group.get::<String,_>("status").as_str())) {
        return Err(ApiError(axum::http::StatusCode::CONFLICT,"configuration_verification_required"));
    }
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown')").bind(id).fetch_one(&mut *tx).await?;
    if enabled&&pending>0{return Err(ApiError(axum::http::StatusCode::CONFLICT,"dns_outcome_unknown"));}
    let status=if document["configuration_verified"]!=true{"configuration_change_requires_verification"}else if changed{"group_changed_automation_disabled"}else if enabled{"waiting_for_quorum"}else{"disabled"};
    sqlx::query("UPDATE profiles SET resume_automatic=false WHERE group_id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("UPDATE failover_groups SET document=$2,token_cipher=$3,automatic=$4,status=$5,updated_at=now() WHERE id=$1").bind(id).bind(document).bind(cipher).bind(enabled).bind(status).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'dns.group.update',$2,$3)").bind(user.id).bind(id.to_string()).bind(json!({"automatic":enabled,"membership_or_token_changed":changed})).execute(&mut *tx).await?;
    tx.commit().await?;Ok(Json(ok(json!({}))))
}

/// Removing enrollment never performs a Cloudflare write or discards an unresolved intent.
pub(super) async fn remove(state:&State,user:&Session,id:Uuid)->Result<Json<Value>> {
    user.require("owner")?;let mut tx=state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
    let group=load(state,id).await?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE group_id=$1 AND state IN ('writing','unknown')").bind(id).fetch_one(&mut *tx).await?;
    if group.get::<bool,_>("automatic")||pending>0{return Err(ApiError(axum::http::StatusCode::CONFLICT,"disable_and_reconcile_before_removal"));}
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'dns.group.remove',$2,$3)").bind(user.id).bind(id.to_string()).bind(json!({"document":group.get::<Value,_>("document"),"observed":group.get::<Value,_>("observed"),"dns_unchanged":true})).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM failover_groups WHERE id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
}

/// A fresh owning-node read and the coordinator fence prevent a stale editor from clearing a pause.
pub(super) async fn verify(state:&State,user:&Session,id:Uuid)->Result<Json<Value>> {
    user.require("owner")?;let mut tx=state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM operations WHERE state IN ('queued','running')").fetch_one(&mut *tx).await?;
    if pending>0{return Err(ApiError(axum::http::StatusCode::CONFLICT,"configuration_operation_pending"));}
    let group=load(state,id).await?;let active=group.get::<Option<Uuid>,_>("active_node").ok_or_else(bad)?;
    let node=crate::state::node(state,active).await?;let node_doc:Value=node.get("document");
    let (status,response)=crate::state::node_request(state,active,reqwest::Method::GET,"/v1/fleet/compatibility",None,None).await?;
    let fingerprint=response.pointer("/data/fingerprint").and_then(Value::as_str).ok_or_else(bad)?;
    if status!=200||fingerprint.len()!=64||hex::decode(fingerprint).is_err(){return Err(bad());}
    let mut document:Value=group.get("document");document["configuration_verified"]=json!(true);document["fingerprint"]=json!(fingerprint);
    document["port"]=node_doc["port"].clone();document["sni"]=node_doc["sni"].clone();
    sqlx::query("UPDATE failover_groups SET document=$2,status='configuration_verified',automatic=false WHERE id=$1").bind(id).bind(document).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'dns.configuration_verified',$2,'{}')").bind(user.id).bind(id.to_string()).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
}
