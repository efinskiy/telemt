//! Authenticated fleet resources. Node traffic is forwarded only through an explicit control API allowlist.
use axum::{body::to_bytes, extract::{Path,State as ExtractState}, http::{Method,HeaderMap}, Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{auth::{self,Session},crypto,error::{Result,bad},state::{State,field,parse_id,bounded_name,valid_secret,node,node_request,ok,redact,audit}};

fn control_path(path:&str,method:&str)->bool {
    if path.len()>512 || !path.starts_with("/v1/") || path.contains(['#','\\']) { return false; }
    let raw=path.split('?').next().unwrap_or("");
    if method=="GET" {
        return ["/v1/config","/v1/users","/v1/fleet/traffic","/v1/health","/v1/health/ready","/v1/system/info","/v1/limits/effective"].contains(&raw)
            || raw.starts_with("/v1/runtime/") || raw.starts_with("/v1/stats/") || user_path(raw,"GET");
    }
    (raw=="/v1/config" && method=="PATCH") || (raw=="/v1/system/reload" && method=="POST")
        || user_path(raw,method) || (method=="POST" && ["/v1/runtime/web/sessions/close","/v1/runtime/web/debug/clear","/v1/runtime/web/carrier-learning/reset","/v1/runtime/web/lifecycle/pause","/v1/runtime/web/lifecycle/drain","/v1/runtime/web/lifecycle/resume"].contains(&raw))
}
fn user_path(path:&str,method:&str)->bool {
    if path=="/v1/users" { return method=="POST" || method=="GET"; }
    let parts:Vec<_>=path.split('/').collect();
    if parts.len()<4 || parts[1]!="v1" || parts[2]!="users" || parts[3].is_empty() || parts[3].len()>64 || [".",".."].contains(&parts[3]) || !parts[3].bytes().all(|c|c.is_ascii_alphanumeric()||b"_.-".contains(&c)) { return false; }
    if parts.len()==4 { return ["GET","PATCH","DELETE"].contains(&method); }
    parts.len()==5 && method=="POST" && ["rotate-secret","enable","disable","reset-quota"].contains(&parts[4])
}

pub(crate) async fn dispatch(ExtractState(state):ExtractState<State>,Path(path):Path<String>,headers:HeaderMap,req:axum::extract::Request)->Result<Json<Value>> {
    let method=req.method().clone(); let mutate=method!=Method::GET;
    let user=auth::session(&state,&headers,mutate).await?;
    let uri=req.uri().clone(); let bytes=to_bytes(req.into_body(),1024*1024).await.map_err(|_|bad())?;
    let body:Value=if bytes.is_empty(){json!({})}else{serde_json::from_slice(&bytes).map_err(|_|bad())?};
    let parts:Vec<_>=path.split('/').collect();
    match parts.as_slice() {
        ["profiles",..]=>crate::profiles::route(&state,&user,&parts,&method,&headers,body).await,
        ["certificates",..]=>crate::certificates::route(&state,&user,&parts,&method,body).await,
        ["managed-nodes"] | ["nodes",_,"enrollment"|"managed"|"revoke"|"redeploy"|"adopt"]=>crate::managed::route(&state,&user,&parts,&method,body).await,
        ["nodes"] if method==Method::GET => {
            let rows=sqlx::query("SELECT id,name,api_url,document,snapshot,collected_at,enabled FROM nodes ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            let data:Vec<_>=rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"api_url":r.get::<String,_>("api_url"),"document":r.get::<Value,_>("document"),"snapshot":redact(r.get("snapshot")),"collected_at":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("collected_at"),"enabled":r.get::<bool,_>("enabled")})).collect(); Ok(Json(ok(json!(data))))
        }
        ["nodes"] if method==Method::POST => { user.require("owner")?; save_node(&state,&user,None,body).await }
        ["nodes",id] if method==Method::PATCH => {
            user.require("owner")?;let id=parse_id(id)?;
            if node(&state,id).await?.get::<Value,_>("document")["managed"]==true{return Err(crate::error::ApiError(axum::http::StatusCode::CONFLICT,"managed_profile_owned"));}
            if body.as_object().is_some_and(|body|body.len()==1&&body.contains_key("enabled")) {
                node(&state,id).await?;let enabled=body["enabled"].as_bool().ok_or_else(bad)?;let mut tx=state.db.begin().await?;
                sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
                crate::rotation::guard_node(&state,id).await?;
                sqlx::query("UPDATE failover_groups SET automatic=false,document=jsonb_set(document,'{configuration_verified}','false'),status='endpoint_change_requires_verification' WHERE document->'node_ids' @> $1::jsonb").bind(json!([id])).execute(&mut *tx).await?;
                sqlx::query("UPDATE nodes SET enabled=$2,collected_at=NULL,snapshot='{}' WHERE id=$1").bind(id).bind(enabled).execute(&mut *tx).await?;
                sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'node.enabled',$2,$3)").bind(user.id).bind(id.to_string()).bind(body).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(ok(json!({}))))
            } else {save_node(&state,&user,Some(id),body).await}
        }
        ["nodes",id] if method==Method::DELETE => {
            user.require("owner")?; let id=parse_id(id)?;
            let references:i64=sqlx::query_scalar("SELECT count(*) FROM failover_groups WHERE active_node=$1 OR document->'node_ids' @> $2::jsonb").bind(id).bind(json!([id])).fetch_one(&state.db).await?;
            if references>0{return Err(bad());}
            sqlx::query("DELETE FROM nodes WHERE id=$1").bind(id).execute(&state.db).await?; audit(&state,Some(user.id),"node.delete",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({}))))
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
            for id in nodes {let id=parse_id(id.as_str().ok_or_else(bad)?)?; if !ids.insert(id){return Err(bad());} let target=node(&state,id).await?;if target.get::<Value,_>("document")["managed"]==true&&!telemt_managed::allowed_command(verb,path){return Err(crate::error::ApiError(axum::http::StatusCode::CONFLICT,"managed_profile_owned"));}
                if body.pointer(&format!("/revisions/{id}")).and_then(Value::as_str).is_none(){return Err(bad());}
            }
            let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            for id in &ids {crate::rotation::guard_node(&state,*id).await?;}
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
        ["probes"] if method==Method::GET => {let rows=sqlx::query("SELECT j.id,j.round,j.node_id,j.family,a.network,j.completed_at,j.result FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.issued_at>now()-interval '10 minutes' ORDER BY j.issued_at DESC LIMIT 1000").fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"round":r.get::<i64,_>("round"),"node_id":r.get::<Uuid,_>("node_id"),"family":r.get::<i32,_>("family"),"network":r.get::<String,_>("network"),"completed_at":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("completed_at"),"result":r.get::<Option<Value>,_>("result")})).collect::<Vec<_>>()))))}
        ["ip-pools",..] => crate::rotation::route(&state,&user,&parts,&method,body).await,
        ["groups",..] => crate::dns::route(&state,&user,&parts,&method,body).await,
        ["audit"] if method==Method::GET => {let rows=sqlx::query("SELECT id,actor,action,target,outcome,at FROM audit ORDER BY id DESC LIMIT 500").fetch_all(&state.db).await?;Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<i64,_>("id"),"actor":r.get::<Option<Uuid>,_>("actor"),"action":r.get::<String,_>("action"),"target":r.get::<String,_>("target"),"outcome":r.get::<Value,_>("outcome"),"at":r.get::<chrono::DateTime<chrono::Utc>,_>("at")})).collect::<Vec<_>>()))))}
        _=>Err(bad()),
    }
}

fn validate_node_api_url(raw:&str)->Result<()> {
    let url=url::Url::parse(raw).map_err(|_|bad())?;
    let destination=match url.host(){
        Some(url::Host::Ipv4(ip))=>!ip.is_unspecified()&&!ip.is_multicast()&&!ip.is_broadcast(),
        Some(url::Host::Ipv6(ip))=>!ip.is_unspecified()&&!ip.is_multicast(),
        Some(url::Host::Domain(name))=>!name.is_empty(),
        None=>false,
    };
    // Owners select the management network; credentials remain outside the endpoint URL.
    if !destination||!["http","https"].contains(&url.scheme())||url.path()!="/"||url.query().is_some()||url.fragment().is_some()||!url.username().is_empty()||url.password().is_some(){return Err(bad());}Ok(())
}

async fn save_node(state:&State,user:&Session,id:Option<Uuid>,body:Value)->Result<Json<Value>> {
    let name=bounded_name(&body,"name")?;let raw=field(&body,"api_url")?;validate_node_api_url(raw)?;
    let port=body.get("port").and_then(Value::as_u64).ok_or_else(bad)?;if !(1..=65535).contains(&port){return Err(bad());}
    let sni=bounded_name(&body,"sni")?;if !sni.bytes().all(|c|c.is_ascii_alphanumeric()||b".-".contains(&c)){return Err(bad());}
    for key in ["ipv4","ipv6"] {if let Some(value)=body.get(key).and_then(Value::as_str).filter(|s|!s.is_empty()){let ip:std::net::IpAddr=value.parse().map_err(|_|bad())?;if ip.is_ipv4()!=(key=="ipv4")||ip.is_unspecified()||ip.is_multicast(){return Err(bad());}}}
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM nodes").fetch_one(&state.db).await?;if id.is_none()&&count>=20{return Err(bad());}
    if let Some(id)=id {node(state,id).await?;}
    let id=id.unwrap_or_else(Uuid::new_v4);let existing=sqlx::query("SELECT token_cipher,probe_cipher FROM nodes WHERE id=$1").bind(id).fetch_optional(&state.db).await?;
    let token=if let Some(value)=body.get("token").and_then(Value::as_str).filter(|s|!s.is_empty()){if value.len()>1024||reqwest::header::HeaderValue::from_str(value).is_err(){return Err(bad());}state.vault.seal(value,&format!("node:{id}"))?}else{existing.as_ref().map(|r|r.get::<String,_>("token_cipher")).ok_or_else(bad)?};
    let probe=if let Some(value)=body.get("probe_secret").and_then(Value::as_str).filter(|s|!s.is_empty()){if !valid_secret(value){return Err(bad());}state.vault.seal(value,&format!("probe:{id}"))?}else{existing.as_ref().map(|r|r.get::<String,_>("probe_cipher")).ok_or_else(bad)?};
    let document=json!({"ipv4":body.get("ipv4"),"ipv6":body.get("ipv6"),"port":port,"sni":sni,"probe_dc":body.get("probe_dc").and_then(Value::as_i64).filter(|dc|(1..=5).contains(dc)).unwrap_or(2)});
    let mut tx=state.db.begin().await?;sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
    crate::rotation::guard_node(state,id).await?;
    sqlx::query("UPDATE failover_groups SET automatic=false,document=jsonb_set(document,'{configuration_verified}','false'),status='endpoint_change_requires_verification' WHERE document->'node_ids' @> $1::jsonb").bind(json!([id])).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO nodes(id,name,api_url,token_cipher,probe_cipher,document) VALUES($1,$2,$3,$4,$5,$6) ON CONFLICT(id) DO UPDATE SET name=$2,api_url=$3,token_cipher=$4,probe_cipher=$5,document=$6,collected_at=NULL,snapshot='{}'").bind(id).bind(name).bind(raw).bind(token).bind(probe).bind(document).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'node.save',$2,'{}')").bind(user.id).bind(id.to_string()).execute(&mut *tx).await?;tx.commit().await?;Ok(Json(ok(json!({"id":id}))))
}

#[cfg(test)]
mod tests {use super::*;#[test]fn node_proxy_does_not_accept_arbitrary_targets(){assert!(!control_path("https://evil.invalid","POST"));assert!(!control_path("/v1/users/a/../../config","PATCH"));assert!(!control_path("/v1/config","DELETE"));assert!(control_path("/v1/users/a/disable","POST"));} #[test]fn roles_are_server_enforced(){let user=Session{id:Uuid::nil(),username:"x".into(),role:"viewer".into()};assert!(user.require("operator").is_err());assert!(user.require("owner").is_err());}}


#[cfg(test)]
mod node_api_url_tests {
    use super::validate_node_api_url;

    #[test]
    fn fleet_traffic_is_read_only_monitoring() {
        assert!(super::control_path("/v1/fleet/traffic", "GET"));
        assert!(!super::control_path("/v1/fleet/traffic", "POST"));
        assert!(!super::control_path("/v1/fleet/traffic/other", "GET"));
    }
    #[test]
    fn accepts_public_private_and_domain_management_origins() {
        for origin in ["http://203.0.113.10:9091/","https://198.51.100.20:9443/","https://[2001:db8::10]:9443/","http://node.example.org:9091/","https://node.example.org","http://10.70.0.2:9091/","http://127.0.0.1:9091/","http://[::1]:9091/","https://[fd00::2]/"] {
            assert!(validate_node_api_url(origin).is_ok(),"{origin}");
        }
    }

    #[test]
    fn rejects_non_origin_urls_and_non_destination_addresses() {
        for origin in ["file:///etc/passwd","ftp://node.example.org/","//node.example.org/","https://","https://user:password@node.example.org/","https://node.example.org/v1/config","https://node.example.org/?token=x","https://node.example.org/#fragment","http://0.0.0.0/","http://[::]/","http://224.0.0.1/","http://[ff02::1]/"] {
            assert!(validate_node_api_url(origin).is_err(),"{origin}");
        }
    }
}
