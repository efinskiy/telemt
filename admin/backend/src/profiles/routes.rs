use super::{model,rollout};
use crate::{state::{State,field,parse_id,ok,audit,bounded_name},auth::Session,error::{Result,ApiError,bad}};
use axum::{http::{Method,StatusCode,HeaderMap},Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
fn conflict()->ApiError{ApiError(StatusCode::CONFLICT,"profile_revision_conflict")}

pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,headers:&HeaderMap,body:Value)->Result<Json<Value>> {
    match parts {
        ["profiles",id,"import"] if *method==Method::POST=>super::import::import(state,user,parse_id(id)?,headers,body).await,
        ["profiles"] if *method==Method::GET=>{
            let rows=sqlx::query("SELECT p.*,c.status AS certificate_status,c.info AS certificate_info FROM profiles p JOIN certificates c ON c.id=p.certificate_id ORDER BY p.name LIMIT 100").fetch_all(&state.db).await?;
            let mut data=Vec::new();
            for row in rows{
                let id:Uuid=row.get("id");let raw=state.vault.open(&row.get::<String,_>("draft_cipher"),&format!("profile-draft:{id}"))?;
                let mut doc:Value=serde_json::from_str(&raw).map_err(|_|bad())?;
                // Keys are disclosed only through an explicit operator action, never the profile listing.
                if let Some(users)=doc["users"].as_array_mut(){for entry in users{entry["secret"]=json!("");}}
                if user.role=="viewer"{doc=crate::state::redact(doc);}
                let nodes=sqlx::query("SELECT m.node_id,n.name,m.desired_revision,m.applied_revision,m.status,m.report,m.last_seen,m.overrides FROM managed_nodes m JOIN nodes n ON n.id=m.node_id WHERE m.profile_id=$1 ORDER BY n.name LIMIT 20").bind(id).fetch_all(&state.db).await?;
                data.push(json!({"id":id,"name":row.get::<String,_>("name"),"domain":row.get::<String,_>("domain"),"certificate_id":row.get::<Uuid,_>("certificate_id"),"group_id":row.get::<Option<Uuid>,_>("group_id"),"draft_revision":row.get::<i64,_>("draft_revision"),"published_revision":row.get::<i64,_>("published_revision"),"document":doc,"certificate_status":row.get::<String,_>("certificate_status"),"certificate_info":row.get::<Value,_>("certificate_info"),"nodes":nodes.into_iter().map(|n|json!({"id":n.get::<Uuid,_>("node_id"),"name":n.get::<String,_>("name"),"desired_revision":n.get::<i64,_>("desired_revision"),"applied_revision":n.get::<i64,_>("applied_revision"),"status":n.get::<String,_>("status"),"report":n.get::<Value,_>("report"),"last_seen":n.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_seen"),"overrides":n.get::<Value,_>("overrides")})).collect::<Vec<_>>()}));
            }Ok(Json(ok(json!(data))))
        }
        ["profiles"] if *method==Method::POST=>{
            user.require("owner")?;let id=Uuid::new_v4();let name=bounded_name(&body,"name")?;let domain=model::domain(field(&body,"domain")?)?;let cert=parse_id(field(&body,"certificate_id")?)?;
            let cert_domain:String=sqlx::query_scalar("SELECT domain FROM certificates WHERE id=$1").bind(cert).fetch_one(&state.db).await?;if cert_domain!=domain{return Err(bad());}
            let document=body.get("document").cloned().unwrap_or(json!({"native_port":8443,"base_path":"","settings":{},"users":[]}));
            let doc=prepare(document,None)?;let cipher=state.vault.seal(&serde_json::to_string(&doc).map_err(|_|bad())?,&format!("profile-draft:{id}"))?;
            let probe=state.vault.seal(&crate::crypto::random()[..32],&format!("profile-probe:{id}"))?;
            sqlx::query("INSERT INTO profiles(id,name,domain,certificate_id,draft_cipher,probe_cipher) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(name).bind(domain).bind(cert).bind(cipher).bind(probe).execute(&state.db).await?;
            audit(state,Some(user.id),"profile.create",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({"id":id,"draft_revision":1}))))
        }
        ["profiles",id] if *method==Method::PATCH=>{
            user.require("operator")?;let id=parse_id(id)?;let expected=headers.get("if-match").and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<i64>().ok()).ok_or_else(conflict)?;
            let mut tx=state.db.begin().await?;let row=sqlx::query("SELECT draft_revision,draft_cipher,group_id FROM profiles WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
            if row.get::<i64,_>("draft_revision")!=expected{return Err(conflict());}
            let old=state.vault.open(&row.get::<String,_>("draft_cipher"),&format!("profile-draft:{id}"))?;let old:Value=serde_json::from_str(&old).map_err(|_|bad())?;
            let doc=prepare(body["document"].clone(),Some(&old))?;
            if user.role!="owner" && (doc.native_port!=old["native_port"].as_u64().unwrap_or(0) as u16 || doc.base_path!=old["base_path"].as_str().unwrap_or("")){return Err(ApiError(StatusCode::FORBIDDEN,"owner_required"));}
            let group=if let Some(value)=body.get("group_id") {user.require("owner")?;if value.is_null(){None}else{Some(parse_id(value.as_str().ok_or_else(bad)?)?)}}else{row.get::<Option<Uuid>,_>("group_id")};
            if let Some(group)=group{let domain:String=sqlx::query_scalar("SELECT domain FROM profiles WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;let enrolled:Value=sqlx::query_scalar("SELECT document FROM failover_groups WHERE id=$1").bind(group).fetch_one(&mut *tx).await?;if domain!=enrolled["domain"]{return Err(bad());}
                if enrolled["profile_id"]!=id.to_string(){return Err(ApiError(StatusCode::CONFLICT,"reserve_requires_same_profile"));}}
            let cipher=state.vault.seal(&serde_json::to_string(&doc).map_err(|_|bad())?,&format!("profile-draft:{id}"))?;
            sqlx::query("UPDATE profiles SET draft_cipher=$2,draft_revision=draft_revision+1,group_id=$3 WHERE id=$1").bind(id).bind(cipher).bind(group).execute(&mut *tx).await?;tx.commit().await?;
            audit(state,Some(user.id),"profile.edit",&id.to_string(),json!({"revision":expected+1})).await?;Ok(Json(ok(json!({"draft_revision":expected+1}))))
        }
        ["profiles",id,"publish"] | ["profiles",id,"rollback"] if *method==Method::POST=>{
            user.require("operator")?;let id=parse_id(id)?;let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).execute(&mut *tx).await?;
            let row=sqlx::query("SELECT * FROM profiles WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
            let expected=headers.get("if-match").and_then(|v|v.to_str().ok()).and_then(|s|s.parse::<i64>().ok()).ok_or_else(conflict)?;
            if expected!=row.get::<i64,_>("draft_revision"){return Err(conflict());}
            let revision=row.get::<i64,_>("published_revision")+1;
            let raw=if parts[2]=="rollback" {let from=body["revision"].as_i64().ok_or_else(bad)?;let cipher:String=sqlx::query_scalar("SELECT document_cipher FROM profile_versions WHERE profile_id=$1 AND revision=$2").bind(id).bind(from).fetch_one(&mut *tx).await?;state.vault.open(&cipher,&format!("profile:{id}:{from}"))?}else{state.vault.open(&row.get::<String,_>("draft_cipher"),&format!("profile-draft:{id}"))?};
            let doc=model::validate(serde_json::from_str(&raw).map_err(|_|bad())?)?;
            if doc.users.is_empty(){return Err(ApiError(StatusCode::BAD_REQUEST,"profile_requires_user"));}
            super::validation::validate(&doc,&row.get::<String,_>("domain")).await.map_err(|_|ApiError(StatusCode::BAD_REQUEST,"profile_validation_failed"))?;
            let material:Option<String>=sqlx::query_scalar("SELECT material_cipher FROM certificates WHERE id=$1").bind(row.get::<Uuid,_>("certificate_id")).fetch_one(&mut *tx).await?;if material.is_none(){return Err(ApiError(StatusCode::CONFLICT,"certificate_not_ready"));}
            let cipher=state.vault.seal(&raw,&format!("profile:{id}:{revision}"))?;
            sqlx::query("INSERT INTO profile_versions(profile_id,revision,document_cipher,actor) VALUES($1,$2,$3,$4)").bind(id).bind(revision).bind(cipher).bind(user.id).execute(&mut *tx).await?;
            sqlx::query("UPDATE profiles SET published_revision=$2,draft_cipher=$3,draft_revision=draft_revision+1 WHERE id=$1").bind(id).bind(revision).bind(state.vault.seal(&raw,&format!("profile-draft:{id}"))?).execute(&mut *tx).await?;
            if let Some(group)=row.get::<Option<Uuid>,_>("group_id"){
                let group_doc:Value=sqlx::query_scalar("SELECT document FROM failover_groups WHERE id=$1 FOR UPDATE").bind(group).fetch_one(&mut *tx).await?;
                if group_doc["port"]!=json!(doc.native_port){return Err(ApiError(StatusCode::CONFLICT,"profile_dns_port_mismatch"));}
                sqlx::query("UPDATE profiles SET resume_automatic=resume_automatic OR (SELECT automatic FROM failover_groups WHERE id=$2) WHERE id=$1").bind(id).bind(group).execute(&mut *tx).await?;
                sqlx::query("UPDATE failover_groups SET automatic=false,status=CASE WHEN status IN('external_dns_conflict','dns_outcome_unknown','dns_write_pending','configuration_drift','node_revoked') THEN status ELSE 'profile_rollout' END,document=jsonb_set(document,'{configuration_verified}','false') WHERE id=$1").bind(group).execute(&mut *tx).await?;
            }
            let nodes:Vec<Uuid>=sqlx::query_scalar("SELECT node_id FROM managed_nodes WHERE profile_id=$1 AND NOT revoked ORDER BY node_id LIMIT 20").bind(id).fetch_all(&mut *tx).await?;
            let mut deployments=Vec::new();for node in nodes{deployments.push(rollout::enqueue(state,&mut tx,node,Some(user.id)).await?);}
            tx.commit().await?;audit(state,Some(user.id),"profile.publish",&id.to_string(),json!({"revision":revision,"deployments":deployments})).await?;
            Ok(Json(ok(json!({"revision":revision,"deployments":deployments}))))
        }
        ["profiles",id,"versions"] if *method==Method::GET=>{
            let rows=sqlx::query("SELECT revision,created_at,actor FROM profile_versions WHERE profile_id=$1 ORDER BY revision DESC LIMIT 100").bind(parse_id(id)?).fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"revision":r.get::<i64,_>("revision"),"created_at":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"actor":r.get::<Uuid,_>("actor")})).collect::<Vec<_>>()))))
        }
        ["profiles",id,"links"] if *method==Method::POST=>{
            user.require("operator")?;let id=parse_id(id)?;let row=sqlx::query("SELECT p.domain,v.document_cipher,p.published_revision FROM profiles p JOIN profile_versions v ON v.profile_id=p.id AND v.revision=p.published_revision WHERE p.id=$1").bind(id).fetch_one(&state.db).await?;
            let raw=state.vault.open(&row.get::<String,_>("document_cipher"),&format!("profile:{id}:{}",row.get::<i64,_>("published_revision")))?;let doc=model::validate(serde_json::from_str(&raw).map_err(|_|bad())?)?;let domain:String=row.get("domain");
            use base64::Engine;
            let links=doc.users.iter().map(|u|{let server=if doc.base_path.is_empty(){domain.clone()}else{format!("{domain}/{}",doc.base_path)};let mut web=url::form_urlencoded::Serializer::new(String::new());let mut secret=vec![0x70,0xdd];secret.extend(hex::decode(&u.secret).unwrap_or_default());let secret=if doc.base_path.is_empty(){format!("dd{}",u.secret)}else{base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(secret)};web.append_pair("server",&server).append_pair("secret",&secret);json!({"username":u.username,"enabled":u.enabled,"web":format!("tg://webproxy?{}",web.finish()),"native":format!("tg://proxy?server={domain}&port={}&secret=ee{}{}",doc.native_port,u.secret,hex::encode(domain.as_bytes()))})}).collect::<Vec<_>>();
            audit(state,Some(user.id),"profile.links",&id.to_string(),json!({})).await?;Ok(Json(ok(json!(links))))
        }
        _=>Err(bad()),
    }
}
fn prepare(mut doc:Value,old:Option<&Value>)->Result<model::ProfileDocument>{
    for entry in doc["users"].as_array_mut().ok_or_else(bad)?{
        if entry["secret"].as_str().is_none_or(str::is_empty){
            let previous=old.and_then(|v|v["users"].as_array()).and_then(|v|v.iter().find(|u|u["username"]==entry["username"])).and_then(|u|u["secret"].as_str());
            entry["secret"]=json!(previous.map(str::to_owned).unwrap_or_else(||crate::crypto::random()[..32].into()));
        }
    }model::validate(doc)
}
