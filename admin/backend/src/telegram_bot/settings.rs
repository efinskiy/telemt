//! Owner-managed Telegram identity bindings; administrator roles are never copied into persistent grants.
use super::*;
use axum::{Json,http::Method};

async fn read(state:&State)->Result<Value> {
    let row=sqlx::query("SELECT b.*,n.token_cipher IS NOT NULL AS token_configured FROM telegram_bot_settings b CROSS JOIN notification_settings n").fetch_one(&state.db).await?;
    let bindings=sqlx::query("SELECT b.telegram_id,b.administrator_id,a.username,a.role,a.enabled FROM telegram_bot_access b JOIN administrators a ON a.id=b.administrator_id ORDER BY b.telegram_id LIMIT 100").fetch_all(&state.db).await?;
    Ok(json!({"enabled":row.get::<bool,_>("enabled"),"revision":row.get::<i64,_>("revision"),"token_configured":row.get::<bool,_>("token_configured"),"username":row.get::<Option<String>,_>("username"),"last_poll_at":row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("last_poll_at"),"last_error":row.get::<Option<String>,_>("last_error"),"bindings":bindings.iter().map(|b|json!({"telegram_id":b.get::<i64,_>("telegram_id").to_string(),"administrator_id":b.get::<Uuid,_>("administrator_id"),"username":b.get::<String,_>("username"),"role":b.get::<String,_>("role"),"enabled":b.get::<bool,_>("enabled")})).collect::<Vec<_>>()}))
}

/// Cookie authentication and CSRF are enforced by the existing dispatcher before owner settings are reached.
pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>> {
    user.require("owner")?;
    match parts {
        ["telegram-bot"] if method==Method::GET=>Ok(Json(crate::state::ok(read(state).await?))),
        ["telegram-bot"] if method==Method::PUT=>{
            let enabled=body["enabled"].as_bool().ok_or_else(bad)?;
            let revision=body["revision"].as_i64().ok_or_else(bad)?;
            let bindings=body["bindings"].as_array().filter(|v|v.len()<=100&&(!enabled||!v.is_empty())).ok_or_else(bad)?;
            let mut seen=std::collections::HashSet::new();let mut parsed=Vec::new();
            for binding in bindings {
                let id=binding["telegram_id"].as_str().and_then(|v|v.parse::<i64>().ok()).filter(|id|*id>0&&*id<=4_503_599_627_370_495).ok_or_else(bad)?;
                let admin=crate::state::parse_id(binding["administrator_id"].as_str().ok_or_else(bad)?)?;
                if !seen.insert(id){return Err(bad());}parsed.push((id,admin));
            }
            let mut tx=state.db.begin().await?;
            lock_settings(&mut tx).await?;
            let current:i64=sqlx::query_scalar("SELECT revision FROM telegram_bot_settings WHERE singleton FOR UPDATE").fetch_one(&mut *tx).await?;
            if current!=revision{return Err(ApiError(StatusCode::CONFLICT,"telegram_bot_revision_conflict"));}
            let configured:bool=sqlx::query_scalar("SELECT token_cipher IS NOT NULL FROM notification_settings").fetch_one(&mut *tx).await?;
            if enabled&&!configured{return Err(ApiError(StatusCode::BAD_REQUEST,"notification_not_configured"));}
            for (_,id) in &parsed {let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM administrators WHERE id=$1 AND enabled)").bind(id).fetch_one(&mut *tx).await?;if !exists{return Err(ApiError(StatusCode::BAD_REQUEST,"telegram_admin_disabled"));}}
            sqlx::query("DELETE FROM telegram_bot_access").execute(&mut *tx).await?;
            for (id,admin) in &parsed {sqlx::query("INSERT INTO telegram_bot_access(telegram_id,administrator_id) VALUES($1,$2)").bind(id).bind(admin).execute(&mut *tx).await?;}
            sqlx::query("UPDATE telegram_bot_settings SET enabled=$1 WHERE singleton").bind(enabled).execute(&mut *tx).await?;
            reset(&mut tx).await?;
            sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'telegram.access.update','bot',$2)").bind(user.id).bind(json!({"enabled":enabled,"bindings":parsed})).execute(&mut *tx).await?;
            tx.commit().await?;Ok(Json(crate::state::ok(read(state).await?)))
        }
        _=>Err(bad()),
    }
}
