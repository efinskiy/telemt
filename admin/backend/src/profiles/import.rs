//! Explicit TOML import preserves existing client keys; generated infrastructure is reviewed separately.
use super::model;
use crate::{state::{State,field,ok,audit},auth::Session,error::{Result,ApiError,bad}};
use axum::{http::{HeaderMap,StatusCode},Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

pub(super) async fn import(state:&State,user:&Session,id:Uuid,headers:&HeaderMap,body:Value)->Result<Json<Value>> {
    user.require("owner")?;
    let source=field(&body,"toml")?;if source.len()>512*1024{return Err(bad());}
    let config:toml::Value=toml::from_str(source).map_err(|_|bad())?;let config=serde_json::to_value(config).map_err(|_|bad())?;
    let access=config["access"].as_object().ok_or_else(bad)?;let keys=access.get("users").and_then(Value::as_object).ok_or_else(bad)?;
    let mut users=Vec::new();for (name,key) in keys {if name=="fleet_probe"{continue;}users.push(json!({"username":name,"secret":key,"enabled":if config["access"]["user_enabled"][name].is_null(){true}else{config["access"]["user_enabled"][name].as_bool().ok_or_else(bad)?},"quota_bytes":limit(&config["access"]["user_data_quota"][name])?,"up_bps":limit(&config["access"]["user_rate_limits"][name]["up_bps"])?,"down_bps":limit(&config["access"]["user_rate_limits"][name]["down_bps"])?}));}
    let mut settings=json!({});for key in ["general","timeouts","shaping","upstreams","dc_overrides","web","access"]{if let Some(value)=config.get(key){settings[key]=value.clone();}}
    if let Some(web)=settings["web"].as_object_mut(){web.remove("vhosts");web.remove("enabled");web.remove("carrier");web.remove("carrier_method");}
    if let Some(access)=settings["access"].as_object_mut(){for key in ["users","user_enabled","user_data_quota","user_rate_limits"]{access.remove(key);}}
    let vhosts=config["web"]["vhosts"].as_array();if vhosts.is_some_and(|v|v.len()>1){return Err(ApiError(StatusCode::BAD_REQUEST,"import_requires_single_domain"));}
    let doc=model::validate(json!({"native_port":body["native_port"].as_u64().unwrap_or(8443),"base_path":vhosts.and_then(|v|v.first()).and_then(|v|v["base_path"].as_str()).unwrap_or(""),"users":users,"settings":settings}))?;
    let expected=headers.get("if-match").and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<i64>().ok()).ok_or_else(bad)?;
    let mut tx=state.db.begin().await?;let row=sqlx::query("SELECT draft_revision,domain FROM profiles WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    if expected!=row.get::<i64,_>("draft_revision"){return Err(ApiError(StatusCode::CONFLICT,"profile_revision_conflict"));}
    let domain:String=row.get("domain");if vhosts.and_then(|v|v.first()).and_then(|v|v["host"].as_str()).is_some_and(|host|host!=domain){return Err(ApiError(StatusCode::CONFLICT,"import_domain_mismatch"));}
    let cipher=state.vault.seal(&serde_json::to_string(&doc).map_err(|_|bad())?,&format!("profile-draft:{id}"))?;
    sqlx::query("UPDATE profiles SET draft_cipher=$2,draft_revision=draft_revision+1 WHERE id=$1").bind(id).bind(cipher).execute(&mut *tx).await?;
    if let Some(probe)=keys.get("fleet_probe").and_then(Value::as_str){if !crate::state::valid_secret(probe){return Err(bad());}sqlx::query("UPDATE profiles SET probe_cipher=$2 WHERE id=$1 AND published_revision=0").bind(id).bind(state.vault.seal(probe,&format!("profile-probe:{id}"))?).execute(&mut *tx).await?;}
    tx.commit().await?;audit(state,Some(user.id),"profile.import",&id.to_string(),json!({"users":doc.users.len()})).await?;
    Ok(Json(ok(json!({"draft_revision":expected+1}))))
}

fn limit(value:&Value)->Result<u64>{if value.is_null(){Ok(0)}else{value.as_u64().ok_or_else(bad)}}
