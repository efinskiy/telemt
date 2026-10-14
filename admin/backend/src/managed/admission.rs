//! DNS admission observes both the desired deployment and public protocol health.
use crate::state::State;
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

/// Only enrolled nodes acknowledging their desired bundle may enter DNS.
pub(crate) async fn ready(state:&State,id:Uuid)->anyhow::Result<bool>{
    let row=sqlx::query("SELECT m.revoked,m.status,m.desired_revision,m.applied_revision,m.last_seen,m.report,c.info FROM managed_nodes m JOIN profiles p ON p.id=m.profile_id JOIN certificates c ON c.id=p.certificate_id WHERE m.node_id=$1").bind(id).fetch_optional(&state.db).await?;
    let Some(row)=row else{return Ok(false);};
    let info:Value=row.get("info");let report:Value=row.get("report");
    Ok(!row.get::<bool,_>("revoked")&&row.get::<String,_>("status")=="applied"&&row.get::<i64,_>("desired_revision")==row.get::<i64,_>("applied_revision")&&crate::state::fresh(row.get("last_seen"),15)&&report["certificate_fingerprint"]==info["fingerprint"]&&info["not_after"].as_i64().is_some_and(|at|at>chrono::Utc::now().timestamp()+3600))
}

pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    let profiles=sqlx::query("SELECT id,group_id,resume_automatic FROM profiles WHERE group_id IS NOT NULL ORDER BY id LIMIT 100").fetch_all(&state.db).await?;
    for profile in profiles{
        let pid:Uuid=profile.get("id");let gid:Uuid=profile.get("group_id");
        let mut tx=state.db.begin().await?;
        let lock:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *tx).await?;if !lock{continue;}
        let group=sqlx::query("SELECT active_node,document,status FROM failover_groups WHERE id=$1 FOR UPDATE").bind(gid).fetch_one(&mut *tx).await?;
        if ["external_dns_conflict","dns_outcome_unknown","dns_write_pending","configuration_drift","node_revoked","node_removed"].contains(&group.get::<String,_>("status").as_str()){continue;}
        let Some(active)=group.get::<Option<Uuid>,_>("active_node")else{continue;};
        let active_ready=ready(state,active).await?;
        let snapshot:Value=sqlx::query_scalar("SELECT snapshot FROM nodes WHERE id=$1 AND collected_at>now()-interval '15 seconds'").bind(active).fetch_optional(&mut *tx).await?.unwrap_or(Value::Null);
        let mut doc:Value=group.get("document");
        // A verified group contract remains usable when its active server is offline.
        let fingerprint=if active_ready {snapshot.pointer("/compatibility/fingerprint").and_then(Value::as_str)}else if doc["configuration_verified"]==true{doc["fingerprint"].as_str()}else{None};
        let Some(fingerprint)=fingerprint.map(str::to_owned)else{continue;};
        let mut members=doc["node_ids"].as_array().cloned().unwrap_or_default();let mut eligible=0;
        let nodes=sqlx::query("SELECT n.id,n.document,n.snapshot,n.collected_at FROM managed_nodes m JOIN nodes n ON n.id=m.node_id WHERE m.profile_id=$1 AND n.enabled AND NOT m.revoked ORDER BY n.name LIMIT 20").bind(pid).fetch_all(&mut *tx).await?;
        for node in nodes{
            let id:Uuid=node.get("id");let current:Value=node.get("snapshot");
            if !ready(state,id).await?||!crate::state::fresh(node.get("collected_at"),15)||current["ready"]!=true||current["compatibility"]["fingerprint"]!=fingerprint{continue;}
            let public:Value=node.get("document");
            if public["port"]!=doc["port"]||public["sni"]!=doc["sni"]{continue;}
            let mut healthy=true;
            for (key,family,record) in [("ipv4",4,"record_a_id"),("ipv6",6,"record_aaaa_id")]{if doc[record].as_str().is_some_and(|v|!v.is_empty()){
                healthy &= public[key].as_str().is_some_and(|v|!v.is_empty()) && matches!(crate::probes::health(state,id,family).await?,crate::probes::Health::Healthy|crate::probes::Health::Recovered);
            }}
            if healthy{eligible+=1;if !members.contains(&json!(id))&&members.len()<20{members.push(json!(id));}}
        }
        doc["node_ids"]=json!(members);doc["fingerprint"]=json!(fingerprint);
        if eligible>0{doc["configuration_verified"]=json!(true);}
        let resume=profile.get::<bool,_>("resume_automatic")&&eligible>=2&&group.get::<String,_>("status")=="profile_rollout";
        sqlx::query("UPDATE failover_groups SET document=$2,automatic=CASE WHEN $3 THEN true ELSE automatic END,status=CASE WHEN $3 THEN 'waiting_for_quorum' ELSE status END WHERE id=$1").bind(gid).bind(doc).bind(resume).execute(&mut *tx).await?;
        if resume{sqlx::query("UPDATE profiles SET resume_automatic=false WHERE id=$1").bind(pid).execute(&mut *tx).await?;}
        tx.commit().await?;
    }Ok(())
}
