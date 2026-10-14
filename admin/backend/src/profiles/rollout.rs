use super::model;
use crate::{state::State,error::{Result,bad}};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

/// Generate a deployment under the profile lock; its identity is independent of certificate renewals.
pub(crate) async fn enqueue(state:&State,tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,node_id:Uuid,actor:Option<Uuid>)->Result<i64>{
    lock_profile(tx,node_id).await?;
    let row=sqlx::query("SELECT p.id AS profile_id,p.domain,p.published_revision,v.document_cipher,c.revision AS certificate_revision,c.material_cipher,n.document,n.token_cipher,n.probe_cipher,m.overrides FROM managed_nodes m JOIN profiles p ON p.id=m.profile_id JOIN profile_versions v ON v.profile_id=p.id AND v.revision=p.published_revision JOIN certificates c ON c.id=p.certificate_id JOIN nodes n ON n.id=m.node_id WHERE m.node_id=$1 AND NOT m.revoked FOR UPDATE OF m").bind(node_id).fetch_one(&mut **tx).await?;
    let profile_id:Uuid=row.get("profile_id");let revision:i64=row.get("published_revision");
    let doc=state.vault.open(&row.get::<String,_>("document_cipher"),&format!("profile:{profile_id}:{revision}"))?;
    let doc=model::validate(serde_json::from_str(&doc).map_err(|_|bad())?)?;
    let cert_id:Uuid=sqlx::query_scalar("SELECT certificate_id FROM profiles WHERE id=$1").bind(profile_id).fetch_one(&mut **tx).await?;
    let cert_revision:i64=row.get("certificate_revision");
    let cert=state.vault.open(&row.get::<Option<String>,_>("material_cipher").ok_or_else(bad)?,&format!("certificate:{cert_id}:{cert_revision}"))?;
    let cert:Value=serde_json::from_str(&cert).map_err(|_|bad())?;let domain:String=row.get("domain");
    telemt_managed::certificate::validate(cert["fullchain"].as_str().ok_or_else(bad)?,cert["private_key"].as_str().ok_or_else(bad)?,&domain)?;
    let document:Value=row.get("document");let ip=document["ipv4"].as_str().or_else(||document["ipv6"].as_str()).ok_or_else(bad)?;
    let token=state.vault.open(&row.get::<String,_>("token_cipher"),&format!("node:{node_id}"))?;
    let probe=state.vault.open(&row.get::<String,_>("probe_cipher"),&format!("probe:{node_id}"))?;
    let (config,nginx)=model::render(&doc,&domain,ip,&token,&probe,&row.get("overrides"))?;
    sqlx::query("DELETE FROM probe_jobs WHERE node_id=$1").bind(node_id).execute(&mut **tx).await?;
    let id:i64=sqlx::query_scalar("INSERT INTO deployments(node_id,profile_revision,certificate_revision,bundle_cipher,digest,actor) VALUES($1,$2,$3,'','',$4) RETURNING id").bind(node_id).bind(revision).bind(cert_revision).bind(actor).fetch_one(&mut **tx).await?;
    let bundle=telemt_managed::Bundle{revision:id,profile_revision:revision,certificate_revision:cert_revision,node_id:node_id.to_string(),domain:domain.clone(),public_ip:ip.into(),config,nginx,fullchain:cert["fullchain"].as_str().ok_or_else(bad)?.into(),private_key:cert["private_key"].as_str().ok_or_else(bad)?.into(),api_token:token.to_string()};
    let digest=bundle.digest()?;let raw=serde_json::to_string(&bundle).map_err(|_|bad())?;
    // Reserve the vault nonce and authentication tag within its decoded-record limit.
    if raw.len()>telemt_managed::MAX_BUNDLE_BYTES-28{return Err(bad());}
    let cipher=state.vault.seal(&raw,&format!("deployment:{node_id}:{id}"))?;
    sqlx::query("UPDATE deployments SET bundle_cipher=$2,digest=$3 WHERE id=$1").bind(id).bind(cipher).bind(digest).execute(&mut **tx).await?;
    sqlx::query("UPDATE deployments SET state='superseded',updated_at=now() WHERE node_id=$1 AND id<>$2 AND state='pending'").bind(node_id).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE managed_nodes SET desired_revision=$2,status='pending' WHERE node_id=$1").bind(node_id).bind(id).execute(&mut **tx).await?;
    sqlx::query("UPDATE nodes SET document=document||$2::jsonb WHERE id=$1").bind(node_id).bind(json!({"port":doc.native_port,"sni":domain,"web_base_path":doc.base_path,"managed":true})).execute(&mut **tx).await?;Ok(id)
}

/// Reconcile certificate renewals without assuming that an offline node applied a bundle.
pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    sqlx::query("DELETE FROM node_commands WHERE expires_at<now()-interval '1 day'").execute(&state.db).await?;
    sqlx::query("DELETE FROM deployments WHERE id IN (SELECT id FROM (SELECT id,node_id,row_number() OVER(PARTITION BY node_id ORDER BY id DESC) AS rank FROM deployments) d JOIN managed_nodes m USING(node_id) WHERE rank>100 AND id<>m.desired_revision AND id<>m.applied_revision)").execute(&state.db).await?;
    sqlx::query("DELETE FROM profile_versions WHERE (profile_id,revision) IN (SELECT profile_id,revision FROM (SELECT profile_id,revision,row_number() OVER(PARTITION BY profile_id ORDER BY revision DESC) AS rank FROM profile_versions) v WHERE rank>100)").execute(&state.db).await?;
    sqlx::query("DELETE FROM certificate_versions WHERE (certificate_id,revision) IN (SELECT certificate_id,revision FROM (SELECT certificate_id,revision,row_number() OVER(PARTITION BY certificate_id ORDER BY revision DESC) AS rank FROM certificate_versions) v WHERE rank>10)").execute(&state.db).await?;
    let mut tx=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(78213420)").fetch_one(&mut *tx).await?;
    if !locked{return Ok(());}
    let ids:Vec<Uuid>=sqlx::query_scalar("SELECT m.node_id FROM managed_nodes m JOIN nodes n ON n.id=m.node_id JOIN profiles p ON p.id=m.profile_id JOIN certificates c ON c.id=p.certificate_id LEFT JOIN deployments d ON d.id=m.desired_revision WHERE NOT m.revoked AND n.enabled AND p.published_revision>0 AND c.revision>0 AND (d.id IS NULL OR d.profile_revision<>p.published_revision OR d.certificate_revision<>c.revision) LIMIT 20").fetch_all(&mut *tx).await?;
    for id in ids{enqueue(state,&mut tx,id,None).await.map_err(|e|anyhow::anyhow!(e.1))?;}
    tx.commit().await?;
    crate::managed::admission::tick(state).await?;
    Ok(())
}

/// Serialize all per-node renders with profile publication before taking the managed-node lock.
pub(crate) async fn lock_profile(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,node:Uuid)->Result<()> {
    sqlx::query("SELECT p.id FROM profiles p JOIN managed_nodes m ON m.profile_id=p.id WHERE m.node_id=$1 FOR UPDATE OF p").bind(node).fetch_one(&mut **tx).await?;Ok(())
}
