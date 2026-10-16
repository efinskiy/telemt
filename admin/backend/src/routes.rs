//! Authenticated fleet resources. Node traffic is forwarded only through an explicit control API allowlist.
use axum::{body::to_bytes, extract::{Path,State as ExtractState}, http::{Method,HeaderMap}, Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{auth,crypto,error::{Result,bad},state::{State,field,parse_id,bounded_name,node,node_request,ok,redact,audit}};

fn control_path(path:&str,method:&str)->bool {
    telemt_managed::allowed_command(method,path)
}

pub(crate) async fn dispatch(ExtractState(state):ExtractState<State>,Path(path):Path<String>,headers:HeaderMap,req:axum::extract::Request)->Result<Json<Value>> {
    let method=req.method().clone(); let mutate=method!=Method::GET;
    let user=auth::session(&state,&headers,mutate).await?;
    let uri=req.uri().clone(); let bytes=to_bytes(req.into_body(),1024*1024).await.map_err(|_|bad())?;
    let body:Value=if bytes.is_empty(){json!({})}else{serde_json::from_slice(&bytes).map_err(|_|bad())?};
    let parts:Vec<_>=path.split('/').collect();
    match parts.as_slice() {
        ["notifications",..]=>crate::notifications::route(&state,&user,&parts,&method,body).await,
        ["profiles",..]=>crate::profiles::route(&state,&user,&parts,&method,&headers,body).await,
        ["certificates",..]=>crate::certificates::route(&state,&user,&parts,&method,body).await,
        ["managed-nodes"] | ["nodes",_,"enrollment"|"managed"|"revoke"|"redeploy"|"addresses"]=>crate::managed::route(&state,&user,&parts,&method,body).await,
        ["nodes"] if method==Method::GET => {
            let rows=sqlx::query("SELECT id,name,document,snapshot,collected_at,enabled FROM nodes ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            let data:Vec<_>=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"document":r.get::<Value,_>("document"),"snapshot":redact(r.get("snapshot")),"collected_at":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("collected_at"),"enabled":r.get::<bool,_>("enabled")})).collect(); Ok(Json(ok(json!(data))))
        }
        ["nodes",id] if method==Method::DELETE => {
            crate::node_lifecycle::remove(&state,&user,parse_id(id)?).await?;
            Ok(Json(ok(json!({}))))
        }
        ["nodes",id,"read"] if method==Method::GET => {
            let params:std::collections::HashMap<_,_>=url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes()).into_owned().collect();
            let path=params.get("path").ok_or_else(bad)?; if !control_path(path,"GET"){return Err(bad());}
            let (status,response)=node_request(&state,parse_id(id)?,reqwest::Method::GET,path,None,None).await?;
            let response=if user.role=="viewer"{redact(response)}else{response}; Ok(Json(ok(json!({"status":status,"response":response}))))
        }
        ["nodes",id,"connections-map"] if method==Method::GET => Ok(Json(ok(crate::connections_map::read(&state,parse_id(id)?).await?))),
        ["nodes",id,"history"] if method==Method::GET => {
            let rows=sqlx::query("SELECT minute,snapshot FROM minute_history WHERE node_id=$1 AND minute>now()-interval '30 days' ORDER BY minute DESC LIMIT 1440").bind(parse_id(id)?).fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"at":r.get::<chrono::DateTime<chrono::Utc>,_>("minute"),"snapshot":r.get::<Value,_>("snapshot")})).collect::<Vec<_>>()))))
        }
        ["operations"] if method==Method::POST => {
            user.require("operator")?; let nodes=body.get("node_ids").and_then(Value::as_array).ok_or_else(bad)?;
            let verb=field(&body,"method")?;let path=field(&body,"path")?;
            if nodes.is_empty()||nodes.len()>20||!control_path(path,verb)||verb=="GET"{return Err(bad());}
            let mut ids=std::collections::HashSet::new();
            for id in nodes {let id=parse_id(id.as_str().ok_or_else(bad)?)?; if !ids.insert(id){return Err(bad());} node(&state,id).await?;if !telemt_managed::allowed_command(verb,path){return Err(crate::error::ApiError(axum::http::StatusCode::CONFLICT,"managed_profile_owned"));}
                if body.pointer(&format!("/revisions/{id}")).and_then(Value::as_str).is_none(){return Err(bad());}
            }
            let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            for id in &ids {node(&state,*id).await?;crate::rotation::guard_node(&state,*id).await?;}
            let id=Uuid::new_v4();let cipher=state.vault.seal(&body.to_string(),&format!("operation:{id}"))?;
            sqlx::query("INSERT INTO operations(id,actor,state,request_cipher) VALUES($1,$2,'queued',$3)").bind(id).bind(user.id).bind(cipher).execute(&mut *tx).await?;tx.commit().await?;
            audit(&state,Some(user.id),"operation.submit",&id.to_string(),json!({"method":verb,"path":path,"nodes":nodes})).await?; Ok(Json(ok(json!({"id":id,"state":"queued"}))))
        }
        ["operations"] if method==Method::GET => {
            let rows=sqlx::query("SELECT id,state,results,created_at,updated_at FROM operations ORDER BY created_at DESC LIMIT 100").fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"state":r.get::<String,_>("state"),"results":r.get::<Value,_>("results"),"created_at":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"updated_at":r.get::<chrono::DateTime<chrono::Utc>,_>("updated_at")})).collect::<Vec<_>>()))))
        }
        ["operations",id,"reconcile"] if method==Method::POST => {user.require("operator")?;crate::operations::reconcile(&state,parse_id(id)?,user.id).await?;Ok(Json(ok(json!({}))))}
        ["administrators"] if method==Method::GET => {
            user.require("owner")?; let rows=sqlx::query("SELECT id,username,role,enabled FROM administrators ORDER BY username LIMIT 100").fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"username":r.get::<String,_>("username"),"role":r.get::<String,_>("role"),"enabled":r.get::<bool,_>("enabled")})).collect::<Vec<_>>()))))
        }
        ["administrators"] if method==Method::POST => {user.require("owner")?; let result=auth::create(&state,field(&body,"username")?,field(&body,"password")?,field(&body,"role")?).await?;audit(&state,Some(user.id),"admin.create",result["id"].as_str().unwrap_or(""),json!({})).await?;Ok(Json(ok(result)))}
        ["administrators",id] if method==Method::PATCH => {
            user.require("owner")?;let id=parse_id(id)?;let enabled=body.get("enabled").and_then(Value::as_bool).ok_or_else(bad)?;
            let role=field(&body,"role")?;if !["owner","operator","viewer"].contains(&role){return Err(bad());}
            let mut tx=state.db.begin().await?;
            let owners=sqlx::query("SELECT id FROM administrators WHERE role='owner' AND enabled FOR UPDATE").fetch_all(&mut *tx).await?;
            if (!enabled||role!="owner") && owners.len()<=1 && owners.iter().any(|r|r.get::<Uuid,_>("id")==id){return Err(bad());}
            sqlx::query("UPDATE administrators SET enabled=$2,role=$3 WHERE id=$1").bind(id).bind(enabled).bind(role).execute(&mut *tx).await?;
            sqlx::query("DELETE FROM sessions WHERE administrator_id=$1").bind(id).execute(&mut *tx).await?;tx.commit().await?;
            audit(&state,Some(user.id),"admin.update",&id.to_string(),body).await?;Ok(Json(ok(json!({}))))
        }
        ["agents"] if method==Method::GET => {let rows=sqlx::query("SELECT id,name,network,country,enabled,last_seen FROM agents ORDER BY name LIMIT 100").fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"network":r.get::<String,_>("network"),"country":r.get::<String,_>("country"),"enabled":r.get::<bool,_>("enabled"),"last_seen":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_seen")})).collect::<Vec<_>>()))))}
        ["agents"] if method==Method::POST => {user.require("owner")?;let id=Uuid::new_v4();let token=crypto::random();let name=bounded_name(&body,"name")?;let network=bounded_name(&body,"network")?;if field(&body,"country")?!="RU"{return Err(bad());}
            let count:i64=sqlx::query_scalar("SELECT count(*) FROM agents").fetch_one(&state.db).await?;if count>=100{return Err(bad());}
            sqlx::query("INSERT INTO agents(id,name,network,token_hash,country) VALUES($1,$2,$3,$4,'RU')").bind(id).bind(name).bind(network).bind(crypto::hash(&token)).execute(&state.db).await?;audit(&state,Some(user.id),"agent.create",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({"id":id,"token":token}))))}
        ["agents",id] if method==Method::PATCH => {user.require("owner")?;let id=parse_id(id)?;let enabled=body.get("enabled").and_then(Value::as_bool).ok_or_else(bad)?;sqlx::query("UPDATE agents SET enabled=$2 WHERE id=$1").bind(id).bind(enabled).execute(&state.db).await?;audit(&state,Some(user.id),"agent.update",&id.to_string(),body).await?;Ok(Json(ok(json!({}))))}
        ["probes"] if method==Method::GET => {let rows=sqlx::query("SELECT j.id,j.round,j.node_id,j.family,j.address,a.network,j.completed_at,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.issued_at>now()-interval '10 minutes' ORDER BY j.issued_at DESC LIMIT 1000").fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"round":r.get::<i64,_>("round"),"node_id":r.get::<Uuid,_>("node_id"),"family":r.get::<i32,_>("family"),"ip":r.get::<Option<String>,_>("address"),"network":r.get::<String,_>("network"),"completed_at":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("completed_at"),"result":r.get::<Option<Value>,_>("result")})).collect::<Vec<_>>()))))}
        ["ip-pools",..] => crate::rotation::route(&state,&user,&parts,&method,body).await,
        ["groups",..] => crate::dns::route(&state,&user,&parts,&method,body).await,
        ["audit"] if method==Method::GET => {let rows=sqlx::query("SELECT id,actor,action,target,outcome,at FROM audit ORDER BY id DESC LIMIT 500").fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"actor":r.get::<Option<Uuid>,_>("actor"),"action":r.get::<String,_>("action"),"target":r.get::<String,_>("target"),"outcome":r.get::<Value,_>("outcome"),"at":r.get::<chrono::DateTime<chrono::Utc>,_>("at")})).collect::<Vec<_>>()))))}
        _=>Err(bad()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn node_proxy_only_allows_managed_runtime_commands() {
        assert!(!control_path("https://evil.invalid","POST"));
        assert!(!control_path("/v1/users/a/../../config","PATCH"));
        assert!(!control_path("/v1/config","PATCH"));
        assert!(!control_path("/v1/users/a/disable","POST"));
        assert!(control_path("/v1/users/a/reset-quota","POST"));
        assert!(control_path("/v1/fleet/traffic","GET"));
    }
    #[test]
    fn roles_are_server_enforced() {
        let user=auth::Session{id:Uuid::nil(),username:"x".into(),role:"viewer".into()};
        assert!(user.require("operator").is_err());assert!(user.require("owner").is_err());
    }
}
