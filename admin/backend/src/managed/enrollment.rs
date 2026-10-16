use crate::{state::{State,field,parse_id,ok,audit},auth::Session,error::{Result,ApiError,bad},crypto};
use axum::{extract::{Path,State as ExtractState},http::{HeaderMap,Method,StatusCode},Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

/// Apply owner-only managed enrollment and deployment actions through the existing coordinator.
pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>>{
    user.require("owner")?;
    match parts{
        ["nodes",id,"addresses"] if *method==Method::PATCH=>super::addresses::update(state,user,parse_id(id)?,&body).await,
        ["managed-nodes"] if *method==Method::POST=>{
            let name=crate::state::bounded_name(&body,"name")?;let profile=parse_id(field(&body,"profile_id")?)?;
            let ip:std::net::IpAddr=field(&body,"ip")?.parse().map_err(|_|bad())?;
            if ip.is_unspecified()||ip.is_multicast()||ip.is_loopback(){return Err(bad());}
            let id=Uuid::new_v4();let enrollment=crypto::random();
            let compose=compose(state,id,&enrollment)?;
            let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            let p=sqlx::query("SELECT domain,probe_cipher,published_revision FROM profiles WHERE id=$1 FOR UPDATE").bind(profile).fetch_one(&mut *tx).await?;
            // Telegram confirmation must enroll the profile revision that the owner reviewed.
            if let Some(expected)=body.get("expected_profile_revision") {
                if expected.as_i64()!=Some(p.get::<i64,_>("published_revision")){return Err(ApiError(StatusCode::CONFLICT,"telegram_profile_changed"));}
            }
            if p.get::<i64,_>("published_revision")==0{return Err(ApiError(StatusCode::CONFLICT,"publish_profile_first"));}
            let token=format!("Bearer {}",crypto::random());let probe=state.vault.open(&p.get::<String,_>("probe_cipher"),&format!("profile-probe:{profile}"))?;
            let document=json!({"ipv4":if ip.is_ipv4(){Some(ip.to_string())}else{None},"ipv6":if ip.is_ipv6(){Some(ip.to_string())}else{None},"port":8443,"sni":p.get::<String,_>("domain"),"probe_dc":2,"managed":true,"profile_id":profile});
            sqlx::query("INSERT INTO nodes(id,name,token_cipher,probe_cipher,document) VALUES($1,$2,$3,$4,$5)").bind(id).bind(name).bind(state.vault.seal(&token,&format!("node:{id}"))?).bind(state.vault.seal(&probe,&format!("probe:{id}"))?).bind(document).execute(&mut *tx).await?;
            sqlx::query("INSERT INTO managed_nodes(node_id,profile_id,enrollment_hash,enrollment_expires) VALUES($1,$2,$3,now()+interval '24 hours')").bind(id).bind(profile).bind(crypto::hash(&enrollment)).execute(&mut *tx).await?;
            crate::profiles::enqueue(state,&mut tx,id,Some(user.id)).await?;
            tx.commit().await?;
            audit(state,Some(user.id),"node.enrollment.create",&id.to_string(),json!({"profile":profile})).await?;
            Ok(Json(ok(json!({"id":id,"compose":compose,"expires_in_seconds":86400}))))
        }
        ["nodes",id,"enrollment"] if *method==Method::POST=>{
            let id=parse_id(id)?;let token=crypto::random();
            // A failed Compose configuration must not invalidate a previously issued token.
            let compose=compose(state,id,&token)?;
            let row=sqlx::query("UPDATE managed_nodes SET enrollment_hash=$2,enrollment_expires=now()+interval '24 hours' WHERE node_id=$1 AND credential_hash IS NULL AND NOT revoked RETURNING node_id").bind(id).bind(crypto::hash(&token)).fetch_optional(&state.db).await?;
            if row.is_none(){return Err(ApiError(StatusCode::CONFLICT,"node_already_registered"));}
            audit(state,Some(user.id),"node.enrollment.renew",&id.to_string(),json!({})).await?;
            Ok(Json(ok(json!({"compose":compose,"expires_in_seconds":86400}))))
        }
        ["nodes",id,"redeploy"] if *method==Method::POST=>{
            let id=parse_id(id)?;let mut tx=state.db.begin().await?;let deployment=crate::profiles::enqueue(state,&mut tx,id,Some(user.id)).await?;tx.commit().await?;
            audit(state,Some(user.id),"node.redeploy",&id.to_string(),json!({"deployment":deployment})).await?;
            Ok(Json(ok(json!({"deployment":deployment}))))
        }
        ["nodes",id,"managed"] if *method==Method::PATCH=>{
            let id=parse_id(id)?;let overrides=body.get("overrides").cloned().unwrap_or(json!({}));
            if overrides.as_object().is_none_or(|m|m.keys().any(|k|!["upload_bps","download_bps"].contains(&k.as_str()))) || overrides.as_object().is_some_and(|m|m.values().any(|v|v.as_u64().is_none_or(|v|v==0))) {return Err(bad());}
            let mut tx=state.db.begin().await?;
            crate::profiles::lock_profile(&mut tx,id).await?;
            sqlx::query("UPDATE managed_nodes SET overrides=$2 WHERE node_id=$1").bind(id).bind(overrides).execute(&mut *tx).await?;
            let deployment=crate::profiles::enqueue(state,&mut tx,id,Some(user.id)).await?;tx.commit().await?;
            audit(state,Some(user.id),"node.overrides",&id.to_string(),json!({"deployment":deployment})).await?;
            Ok(Json(ok(json!({"deployment":deployment}))))
        }
        ["nodes",id,"revoke"] if *method==Method::POST=>{
            let id=parse_id(id)?;let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            sqlx::query("UPDATE managed_nodes SET revoked=true,status='revoked',enrollment_hash=NULL,credential_hash=NULL WHERE node_id=$1").bind(id).execute(&mut *tx).await?;
            sqlx::query("UPDATE nodes SET enabled=false WHERE id=$1").bind(id).execute(&mut *tx).await?;
            sqlx::query("UPDATE failover_groups SET automatic=false,status='node_revoked' WHERE document->'node_ids' @> $1::jsonb").bind(json!([id])).execute(&mut *tx).await?;tx.commit().await?;
            audit(state,Some(user.id),"node.revoke",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({}))))
        }
        _=>Err(bad())
    }
}
fn compose(state:&State,id:Uuid,token:&str)->Result<String>{
    let image=std::env::var("MANAGED_NODE_IMAGE").map_err(|_|ApiError(StatusCode::CONFLICT,"managed_image_not_configured"))?;
    validate_image(&image)?;
    // JSON scalars are valid YAML and avoid interpreting names or origins as Compose interpolation.
    let origin=serde_json::to_string(state.origin.as_ref()).map_err(|_|bad())?;
    Ok(format!("name: telemt-managed\nservices:\n  node:\n    image: {image}\n    network_mode: host\n    restart: unless-stopped\n    read_only: true\n    cap_drop: [ALL]\n    cap_add: [NET_BIND_SERVICE, SETUID, SETGID, CHOWN, KILL]\n    security_opt: [no-new-privileges:true]\n    stop_grace_period: 70s\n    stop_signal: SIGTERM\n    tmpfs: [/tmp]\n    environment:\n      FLEET_URL: {origin}\n      NODE_ID: {id}\n      ENROLLMENT_TOKEN: {token}\n    volumes: [state:/var/lib/telemt-managed]\n    ulimits:\n      nofile: {{soft: 65536, hard: 262144}}\n    healthcheck:\n      test: [CMD, /app/telemt-node-agent, healthcheck]\n      interval: 30s\n      timeout: 5s\n      retries: 3\n    logging:\n      driver: json-file\n      options: {{max-size: 50m, max-file: '3'}}\nvolumes:\n  state:\n"))
}

// Compose supplies an empty string for an unset variable; absence and emptiness are equivalent.
fn validate_image(image:&str)->Result<()> {
    if image.trim().is_empty(){return Err(ApiError(StatusCode::CONFLICT,"managed_image_not_configured"));}
    let invalid=||ApiError(StatusCode::CONFLICT,"managed_image_invalid");
    let Some((name,digest))=image.split_once("@sha256:")else{return Err(invalid());};
    if !name.starts_with("ghcr.io/")||name.contains(['\n','\r','\"','\'','$',' '])||digest.len()!=64||hex::decode(digest).is_err(){return Err(invalid());}
    Ok(())
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn enrollment_image_errors_identify_fleet_configuration() {
        for image in ["", " \t\n"] {
            let error=validate_image(image).unwrap_err();
            assert_eq!(error.0,StatusCode::CONFLICT);
            assert_eq!(error.1,"managed_image_not_configured");
        }
        for image in ["ghcr.io/efinskiy/telemt-managed-node:latest", "ghcr.io/efinskiy/telemt-managed-node@sha256:invalid"] {
            assert_eq!(validate_image(image).unwrap_err().1,"managed_image_invalid");
        }
        assert!(validate_image(&format!("ghcr.io/efinskiy/telemt-managed-node@sha256:{}","a".repeat(64))).is_ok());
    }
}

/// Enrollment and polling never accept a browser session or an external target URL.
pub(crate) async fn agent(ExtractState(state):ExtractState<State>,Path(action):Path<String>,headers:HeaderMap,Json(body):Json<Value>)->Result<Json<Value>>{
    let id=parse_id(field(&body,"node_id")?)?;
    if action=="register"{
        let enrollment=field(&body,"enrollment_token")?;let credential=field(&body,"credential")?;let installation=parse_id(field(&body,"installation_id")?)?;
        if enrollment.len()!=64||credential.len()!=64||hex::decode(credential).is_err(){return Err(bad());}
        let mut tx=state.db.begin().await?;let row=sqlx::query("SELECT * FROM managed_nodes WHERE node_id=$1 AND NOT revoked FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
        let hash=crypto::hash(credential);
        if let Some(current)=row.get::<Option<String>,_>("credential_hash"){
            if !crypto::equal(&current,&hash)||row.get::<Option<Uuid>,_>("installation_id")!=Some(installation){return Err(ApiError(StatusCode::CONFLICT,"enrollment_consumed"));}
        }else{
            if row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("enrollment_expires").is_none_or(|at|at<=chrono::Utc::now()) || !row.get::<Option<String>,_>("enrollment_hash").is_some_and(|h|crypto::equal(&h,&crypto::hash(enrollment))){return Err(ApiError(StatusCode::UNAUTHORIZED,"enrollment_invalid"));}
            sqlx::query("UPDATE managed_nodes SET credential_hash=$2,installation_id=$3,enrollment_hash=NULL,enrollment_expires=NULL,status='registered' WHERE node_id=$1").bind(id).bind(hash).bind(installation).execute(&mut *tx).await?;
        }tx.commit().await?;return Ok(Json(ok(json!({"registered":true}))));
    }
    let token=headers.get("authorization").and_then(|v|v.to_str().ok()).and_then(|v|v.strip_prefix("Bearer ")).filter(|v|v.len()==64).ok_or(ApiError(StatusCode::UNAUTHORIZED,"node_auth_required"))?;
    let row=sqlx::query("SELECT m.* FROM managed_nodes m JOIN nodes n ON n.id=m.node_id WHERE m.node_id=$1 AND NOT m.revoked AND n.enabled").bind(id).fetch_optional(&state.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"node_auth_required"))?;
    if !row.get::<Option<String>,_>("credential_hash").is_some_and(|v|crypto::equal(&v,&crypto::hash(token))){return Err(ApiError(StatusCode::UNAUTHORIZED,"node_auth_required"));}
    if action!="poll"{return Err(bad());}
    if let Some(receipts)=body.get("receipts").and_then(Value::as_array){if receipts.len()>32{return Err(bad());}for item in receipts{
        let receipt:telemt_managed::Receipt=serde_json::from_value(item.clone()).map_err(|_|bad())?;let cid=parse_id(&receipt.id)?;
        let cipher=state.vault.seal(&item.to_string(),&format!("node-receipt:{id}:{cid}"))?;
        sqlx::query("UPDATE node_commands SET response_cipher=$3,state='completed' WHERE id=$1 AND node_id=$2 AND response_cipher IS NULL AND claimed_at IS NOT NULL").bind(cid).bind(id).bind(cipher).execute(&state.db).await?;
    }}
    if let Some(cache)=body.get("reads"){
        if cache.as_object().is_none_or(|m|m.len()>16||m.keys().any(|p|!telemt_managed::allowed_command("GET",p))){return Err(bad());}
        let cipher=state.vault.seal(&cache.to_string(),&format!("node-cache:{id}"))?;
        sqlx::query("UPDATE managed_nodes SET cached_reads_cipher=$2,cached_at=now() WHERE node_id=$1").bind(id).bind(cipher).execute(&state.db).await?;
    }
    if let Some(report)=body.get("deployment").filter(|v|!v.is_null()){
        let revision=report["revision"].as_i64().ok_or_else(bad)?;let digest=report["digest"].as_str().ok_or_else(bad)?;let status=report["state"].as_str().ok_or_else(bad)?;
        if !["applied","failed","drift"].contains(&status){return Err(bad());}
        let mut tx=state.db.begin().await?;
        sqlx::query("SELECT node_id FROM managed_nodes WHERE node_id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
        let deployment=sqlx::query("SELECT digest FROM deployments WHERE id=$1 AND node_id=$2 FOR UPDATE").bind(revision).bind(id).fetch_one(&mut *tx).await?;
        if !crypto::equal(digest,&deployment.get::<String,_>("digest")){return Err(bad());}
        let public=json!({"revision":revision,"state":status,"certificate_fingerprint":report["certificate_fingerprint"],"error":report["error"].as_str().filter(|s|s.len()<128)});
        sqlx::query("UPDATE deployments SET state=$2,result=$3,updated_at=now() WHERE id=$1").bind(revision).bind(status).bind(&public).execute(&mut *tx).await?;
        sqlx::query("UPDATE managed_nodes SET applied_revision=CASE WHEN $3='applied' THEN $2 ELSE applied_revision END,status=$3,report=$4 WHERE node_id=$1 AND desired_revision=$2").bind(id).bind(revision).bind(status).bind(public).execute(&mut *tx).await?;tx.commit().await?;
    }
    sqlx::query("UPDATE managed_nodes SET last_seen=now() WHERE node_id=$1").bind(id).execute(&state.db).await?;
    let desired:i64=row.get("desired_revision");let current=body["revision"].as_i64().unwrap_or(0);
    let bundle=if desired>current{let cipher:String=sqlx::query_scalar("SELECT bundle_cipher FROM deployments WHERE id=$1 AND node_id=$2").bind(desired).bind(id).fetch_one(&state.db).await?;let raw=state.vault.open(&cipher,&format!("deployment:{id}:{desired}"))?;serde_json::from_str::<Value>(&raw).map_err(|_|bad())?}else{Value::Null};
    let rows=sqlx::query("UPDATE node_commands SET state='claimed',claimed_at=now() WHERE id IN (SELECT id FROM node_commands WHERE node_id=$1 AND state='queued' AND expires_at>now() ORDER BY created_at LIMIT 1 FOR UPDATE SKIP LOCKED) RETURNING id,request_cipher").bind(id).fetch_all(&state.db).await?;
    let mut commands=Vec::new();for row in rows{let cid:Uuid=row.get("id");let raw=state.vault.open(&row.get::<String,_>("request_cipher"),&format!("node-command:{id}:{cid}"))?;commands.push(serde_json::from_str::<Value>(&raw).map_err(|_|bad())?);}
    Ok(Json(ok(json!({"bundle":bundle,"commands":commands,"receipts_accepted":true}))))
}
