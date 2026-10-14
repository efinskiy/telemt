//! Atomic removal of a node and its control-plane dependencies.
use crate::{auth::Session, error::Result, state::{State, DNS_COORDINATOR_LOCK}};
use serde_json::{Value, json};
use sqlx::Row;
use uuid::Uuid;

/// Detach shared resources while revoking every credential and queued command for this node.
pub(crate) async fn remove(state: &State, user: &Session, id: Uuid) -> Result<()> {
    user.require("owner")?;
    let mut tx = state.db.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
    // Match rollout's profile -> managed node -> node order before cascading foreign keys.
    sqlx::query("SELECT p.id FROM profiles p JOIN managed_nodes m ON m.profile_id=p.id WHERE m.node_id=$1 FOR UPDATE OF p").bind(id).fetch_optional(&mut *tx).await?;
    sqlx::query("SELECT node_id FROM managed_nodes WHERE node_id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let node = sqlx::query("SELECT name FROM nodes WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let Some(node) = node else { return Ok(()); };
    let groups = sqlx::query("SELECT id,document,active_node,observed FROM failover_groups WHERE active_node=$1 OR document->'node_ids' @> $2::jsonb ORDER BY id FOR UPDATE")
        .bind(id).bind(json!([id])).fetch_all(&mut *tx).await?;
    let mut affected = Vec::new();
    for group in groups {
        let gid: Uuid = group.get("id");
        let mut document: Value = group.get("document");
        if let Some(members) = document["node_ids"].as_array_mut() { members.retain(|member| member != &json!(id)); }
        document["configuration_verified"] = json!(false);
        sqlx::query("UPDATE profiles SET resume_automatic=false WHERE group_id=$1").bind(gid).execute(&mut *tx).await?;
        sqlx::query("UPDATE failover_groups SET document=$2,active_node=CASE WHEN active_node=$3 THEN NULL ELSE active_node END,automatic=false,status='node_removed',updated_at=now() WHERE id=$1")
            .bind(gid).bind(document).bind(id).execute(&mut *tx).await?;
        affected.push(json!({"group_id":gid,"was_active":group.get::<Option<Uuid>,_>("active_node")==Some(id),"observed_dns":group.get::<Value,_>("observed")}));
    }
    sqlx::query("DELETE FROM ip_rotations WHERE pool_id IN (SELECT id FROM ip_pools WHERE node_id=$1)").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM ip_pools WHERE node_id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM dns_changes WHERE old_node=$1 OR new_node=$1").bind(id).execute(&mut *tx).await?;
    prune_operations(state, &mut tx, id).await?;
    sqlx::query("DELETE FROM nodes WHERE id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'node.delete',$2,$3)")
        .bind(user.id).bind(id.to_string()).bind(json!({"name":node.get::<String,_>("name"),"groups":affected,"dns_unchanged":true,"remote_container_unchanged":true})).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}

async fn prune_operations(state: &State, tx: &mut sqlx::Transaction<'_, sqlx::Postgres>, node: Uuid) -> Result<()> {
    // Ciphertext cannot be searched in SQL. Page over a fixed population with bounded memory.
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM operations").fetch_one(&mut **tx).await?;
    let mut cursor: Option<Uuid> = None;
    for _ in 0..(count / 100 + 1) {
        let rows = sqlx::query("SELECT id,request_cipher,results,state FROM operations WHERE ($1::uuid IS NULL OR id>$1) ORDER BY id LIMIT 100 FOR UPDATE")
            .bind(cursor).fetch_all(&mut **tx).await?;
        if rows.is_empty() { break; }
        for row in rows {
            let id: Uuid = row.get("id"); cursor = Some(id);
            let raw = state.vault.open(&row.get::<String,_>("request_cipher"), &format!("operation:{id}"))?;
            let mut request: Value = serde_json::from_str(&raw).map_err(|_|crate::error::bad())?;
            let Some(targets) = request["node_ids"].as_array_mut() else { return Err(crate::error::bad()); };
            let before = targets.len(); targets.retain(|target| target != &json!(node));
            if before == targets.len() { continue; }
            if targets.is_empty() {
                sqlx::query("DELETE FROM operations WHERE id=$1").bind(id).execute(&mut **tx).await?;
                continue;
            }
            if let Some(revisions) = request["revisions"].as_object_mut() { revisions.remove(&node.to_string()); }
            let mut results: Vec<Value> = serde_json::from_value(row.get("results")).map_err(|_|crate::error::bad())?;
            results.retain(|result| result["node_id"] != json!(node));
            let cipher = state.vault.seal(&request.to_string(), &format!("operation:{id}"))?;
            let status = row.get::<String,_>("state");
            let status = if status == "queued" || results.len() < request["node_ids"].as_array().ok_or_else(crate::error::bad)?.len() { status.as_str() } else { crate::operations::completion_state(&results) };
            sqlx::query("UPDATE operations SET request_cipher=$2,results=$3,state=$4,updated_at=now() WHERE id=$1")
                .bind(id).bind(cipher).bind(json!(results)).bind(status).execute(&mut **tx).await?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "node_lifecycle_tests.rs"]
mod tests;
