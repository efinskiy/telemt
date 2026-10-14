//! Durable mutation intents. A lost response is unknown, never an instruction to retry the write.
use crate::{state::{State,node_request,audit,redact},error::{Result,bad}};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    poll_accepted(state).await?;
    let mut tx=state.db.begin().await?;
    let row=sqlx::query("SELECT * FROM operations WHERE state='queued' ORDER BY created_at FOR UPDATE SKIP LOCKED LIMIT 1").fetch_optional(&mut *tx).await?;
    let Some(row)=row else{return Ok(());};let id:Uuid=row.get("id");let actor:Uuid=row.get("actor");
    sqlx::query("UPDATE operations SET state='running',updated_at=now() WHERE id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;
    let raw=state.vault.open(&row.get::<String,_>("request_cipher"),&format!("operation:{id}"))?;
    let request:Value=serde_json::from_str(&raw)?;let method=reqwest::Method::from_bytes(request["method"].as_str().ok_or_else(||anyhow::anyhow!("invalid method"))?.as_bytes())?;
    let path=request["path"].as_str().ok_or_else(||anyhow::anyhow!("invalid path"))?;
    let targets=request["node_ids"].as_array().ok_or_else(||anyhow::anyhow!("invalid nodes"))?;
    let mut results:Vec<Value>=targets.iter().map(|node|json!({"node_id":node,"state":"not_sent"})).collect();
    persist(state,id,"running",&results).await?;
    for (position,node) in targets.iter().enumerate() {
        let node:Uuid=node.as_str().ok_or_else(||anyhow::anyhow!("invalid node"))?.parse()?;
        if !telemt_managed::allowed_command(method.as_str(),path){
            results[position]=json!({"node_id":node,"state":"failed","reason":"unsupported_legacy_operation"});continue;
        }
        results[position]=json!({"node_id":node,"state":"unknown","reason":"request_may_be_sent"});
        persist(state,id,"running",&results).await?;
        let revision=request["revisions"][node.to_string()].as_str();
        let result=node_request(state,node,method.clone(),path,request.get("body"),revision).await;
        let entry=match result {Ok((status,response))=>{
            let web=response.pointer("/data/operation_id").and_then(Value::as_str);
            let drain=response.pointer("/data/drain/operation_id").and_then(Value::as_str);
            let poll=if let Some(web)=web{Some(format!("/v1/runtime/web/operations/{web}"))}else if drain.is_some(){Some("/v1/runtime/web/status".into())}else{None};
            json!({"node_id":node,"state":if !(200..300).contains(&status){"failed"}else if status==202||poll.is_some(){"accepted"}else{"succeeded"},"status":status,"poll_path":poll,"accepted_at":chrono::Utc::now().timestamp(),"drain_id":drain,"response":redact(response)})
        },Err(_)=>{
            let _=read_back(state,node,&request).await;
            json!({"node_id":node,"state":"unknown","reason":"response_lost"})
        }};
        results[position]=entry;persist(state,id,"running",&results).await?;
    }
    let status=completion_state(&results);
    persist(state,id,status,&results).await?;
    audit(state,Some(actor),"operation.complete",&id.to_string(),json!({"state":status,"results":results})).await.map_err(|_|anyhow::anyhow!("audit unavailable"))?;Ok(())
}
async fn poll_accepted(state:&State)->anyhow::Result<()> {
    let rows=sqlx::query("SELECT id,results FROM operations WHERE state IN ('running','unknown') AND results @> '[{\"state\":\"accepted\"}]'::jsonb ORDER BY updated_at LIMIT 20").fetch_all(&state.db).await?;
    for row in rows {
        let id:Uuid=row.get("id");let mut results:Vec<Value>=serde_json::from_value(row.get("results"))?;
        for result in &mut results {
            if result["state"]!="accepted"{continue;}
            let node:Uuid=result["node_id"].as_str().ok_or_else(||anyhow::anyhow!("missing node"))?.parse()?;
            let Some(path)=result["poll_path"].as_str().map(str::to_owned)else{result["state"]=json!("unknown");continue;};
            match node_request(state,node,reqwest::Method::GET,&path,None,None).await {
                Ok((200,response))=>{
                    let data=if result["drain_id"].is_string(){response.pointer("/data/operator_lifecycle/drain").cloned().unwrap_or(Value::Null)}else{response["data"].clone()};
                    if result["drain_id"].is_string()&&data["operation_id"]!=result["drain_id"] {result["state"]=json!("unknown");continue;}
                    match data["state"].as_str(){Some("succeeded"|"completed")=>result["state"]=json!("succeeded"),Some("failed"|"rolled_back"|"cancelled")=>result["state"]=json!("failed"),_=>{}}
                    result["completion"]=redact(data);
                }
                Ok((404|410|409,_))=>result["state"]=json!("unknown"),
                _=>result["poll_error"]=json!("status_unavailable"),
            }
            if result["state"]=="accepted" && chrono::Utc::now().timestamp()-result["accepted_at"].as_i64().unwrap_or(0)>600 { result["state"]=json!("unknown");result["reason"]=json!("observation_deadline"); }
        }
        let status=completion_state(&results);
        persist(state,id,status,&results).await?;
    }Ok(())
}
pub(crate) fn completion_state(results:&[Value])->&'static str {
    if results.is_empty()||results.iter().any(|r|matches!(r["state"].as_str(),Some("unknown"|"not_sent"))){"unknown"}
    else if results.iter().any(|r|r["state"]=="accepted"){"running"}
    else if results.iter().all(|r|r["state"]=="succeeded"){"succeeded"}
    else if results.iter().any(|r|r["state"]=="succeeded"){"partial"}else{"failed"}
}
pub(crate) async fn recover(state:&State)->anyhow::Result<()> {
    let rows=sqlx::query("SELECT id,request_cipher,results FROM operations WHERE state='running'").fetch_all(&state.db).await?;
    for row in rows {
        let id:Uuid=row.get("id");let raw=state.vault.open(&row.get::<String,_>("request_cipher"),&format!("operation:{id}"))?;
        let request:Value=serde_json::from_str(&raw)?;let mut results:Vec<Value>=serde_json::from_value(row.get("results"))?;
        for target in request["node_ids"].as_array().ok_or_else(||anyhow::anyhow!("invalid nodes"))? {
            if !results.iter().any(|result|&result["node_id"]==target){results.push(json!({"node_id":target,"state":"not_sent"}));}
        }
        persist(state,id,completion_state(&results),&results).await?;
    }Ok(())
}
async fn persist(state:&State,id:Uuid,status:&str,results:&[Value])->anyhow::Result<()> {
    let mut tx=state.db.begin().await?;
    let row=sqlx::query("SELECT request_cipher FROM operations WHERE id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
    let Some(row)=row else{return Ok(());};
    let raw=state.vault.open(&row.get::<String,_>("request_cipher"),&format!("operation:{id}"))?;
    let request:Value=serde_json::from_str(&raw)?;
    let targets=request["node_ids"].as_array().ok_or_else(||anyhow::anyhow!("invalid operation targets"))?;
    // A deletion can prune targets while a worker is waiting for a receipt. Never resurrect them.
    let results:Vec<_>=results.iter().filter(|result|targets.contains(&result["node_id"])).cloned().collect();
    let status=if status=="running" && (results.len()<targets.len() || results.iter().any(|result|matches!(result["state"].as_str(),Some("unknown"|"not_sent")))){"running"}else{completion_state(&results)};
    sqlx::query("UPDATE operations SET state=$2,results=$3,updated_at=now() WHERE id=$1").bind(id).bind(status).bind(json!(results)).execute(&mut *tx).await?;
    tx.commit().await?;Ok(())
}
async fn read_back(state:&State,node:Uuid,request:&Value)->anyhow::Result<Value> {
    let path=request["path"].as_str().unwrap_or("").split('?').next().unwrap_or("");
    let read=if path.starts_with("/v1/users/"){path.split('/').take(4).collect::<Vec<_>>().join("/")}else{"/v1/runtime/web/status".to_owned()};
    let (_,response)=node_request(state,node,reqwest::Method::GET,&read,None,None).await?;Ok(response)
}
pub(crate) async fn reconcile(state:&State,id:Uuid,actor:Uuid)->Result<()> {
    let row=sqlx::query("SELECT * FROM operations WHERE id=$1 AND state='unknown'").bind(id).fetch_optional(&state.db).await?.ok_or_else(bad)?;
    let raw=state.vault.open(&row.get::<String,_>("request_cipher"),&format!("operation:{id}"))?;let request:Value=serde_json::from_str(&raw).map_err(|_|bad())?;
    let mut results:Vec<Value>=serde_json::from_value(row.get("results")).map_err(|_|bad())?;
    for result in &mut results {if result["state"]!="unknown"{continue;}let node:Uuid=result["node_id"].as_str().ok_or_else(bad)?.parse().map_err(|_|bad())?;
        match read_back(state,node,&request).await {Ok(response)=>{
            // Runtime actions cannot be proven by matching configuration fields or replayed safely.
            result["readback"]=redact(response);
        },Err(_)=>{result["reason"]=json!("readback_unavailable");}}
    }
    let status=completion_state(&results);
    persist(state,id,status,&results).await?;audit(state,Some(actor),"operation.reconcile",&id.to_string(),json!({"state":status})).await?;Ok(())
}

#[cfg(test)]mod tests{use super::*;#[test]fn incomplete_and_mixed_targets_never_report_success(){assert_eq!(completion_state(&[json!({"state":"succeeded"}),json!({"state":"not_sent"})]),"unknown");assert_eq!(completion_state(&[json!({"state":"succeeded"}),json!({"state":"failed"})]),"partial");assert_eq!(completion_state(&[json!({"state":"accepted"})]),"running");}}
