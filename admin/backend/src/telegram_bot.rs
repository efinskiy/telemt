//! Private-chat control plane using the notification bot, current administrator roles and one-use confirmations.
//! Settings own access, transport bounds Telegram calls, analytics reads snapshots, and actions reuse Fleet routes.
mod settings;
mod transport;
mod analytics;
mod conversation;
mod actions;
pub(crate) use settings::route;
use crate::{auth::Session,error::{ApiError,Result,bad},state::State};
use axum::http::StatusCode;
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

/// Independent from DNS and notification delivery; one controller consumes Telegram updates.
pub(crate) const BOT_LOCK:i64=78_213_423;
const TOKEN_PURPOSE:&str="telegram-notifications";

/// Wait briefly for the current poll instead of making configuration changes race a continuous consumer.
pub(crate) async fn lock_settings(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>)->Result<()> {
    sqlx::query("SET LOCAL lock_timeout='8s'").execute(&mut **tx).await?;
    sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(BOT_LOCK).execute(&mut **tx).await.map_err(|error| {
        if error.as_database_error().and_then(|e|e.code()).as_deref()==Some("55P03"){ApiError(StatusCode::CONFLICT,"telegram_bot_busy")}else{error.into()}
    })?;Ok(())
}

/// Token replacement and access changes invalidate all outstanding confirmations.
pub(crate) async fn reset(tx:&mut sqlx::Transaction<'_,sqlx::Postgres>)->Result<()> {
    sqlx::query("UPDATE telegram_bot_settings SET revision=revision+1,update_offset=NULL,username=NULL,last_error=NULL,next_poll_at=now() WHERE singleton").execute(&mut **tx).await?;
    sqlx::query("DELETE FROM telegram_bot_sessions").execute(&mut **tx).await?;
    sqlx::query("UPDATE telegram_bot_actions SET status='cancelled' WHERE status='pending'").execute(&mut **tx).await?;Ok(())
}

/// Reload authorization for every incoming update and again before consuming a mutation.
async fn identity(state:&State,telegram:i64)->Result<Option<Session>> {
    let row=sqlx::query("SELECT a.id,a.username,a.role FROM telegram_bot_access b JOIN administrators a ON a.id=b.administrator_id WHERE b.telegram_id=$1 AND a.enabled").bind(telegram).fetch_optional(&state.db).await?;
    Ok(row.map(|r|Session{id:r.get("id"),username:r.get("username"),role:r.get("role")}))
}

/// A dedicated bounded worker preserves notification delivery and data-plane independence.
pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    let mut fence=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(BOT_LOCK).fetch_one(&mut *fence).await?;
    if !locked{return Ok(());}
    sqlx::query("DELETE FROM telegram_bot_sessions WHERE expires_at<now()").execute(&state.db).await?;
    sqlx::query("UPDATE telegram_bot_actions SET status='cancelled',result='{\"code\":\"telegram_confirmation_expired\"}' WHERE status='pending' AND expires_at<now()").execute(&state.db).await?;
    sqlx::query("DELETE FROM telegram_bot_actions WHERE created_at<now()-interval '7 days'").execute(&state.db).await?;
    sqlx::query("UPDATE telegram_bot_actions SET artifact_cipher=NULL WHERE artifact_cipher IS NOT NULL AND created_at<now()-interval '24 hours'").execute(&state.db).await?;
    // A prior process may have stopped after executing a mutation but before storing its response.
    sqlx::query("UPDATE telegram_bot_actions SET status='unknown',result='{\"code\":\"telegram_action_interrupted\"}' WHERE status='running'").execute(&state.db).await?;
    let row=sqlx::query("SELECT b.*,n.token_cipher FROM telegram_bot_settings b CROSS JOIN notification_settings n WHERE b.enabled AND b.next_poll_at<=now()").fetch_optional(&state.db).await?;
    let Some(row)=row else{return Ok(());};
    let outcome=tokio::time::timeout(std::time::Duration::from_secs(35),async {
        let cipher:Option<String>=row.get("token_cipher");
        let token=state.vault.open(cipher.as_deref().ok_or(ApiError(StatusCode::CONFLICT,"notification_not_configured"))?,TOKEN_PURPOSE)?;
        let offset:Option<i64>=row.get("update_offset");
        if offset.is_none() {
            let me=transport::call(state,&token,"getMe",json!({})).await?;
            let webhook=transport::call(state,&token,"getWebhookInfo",json!({})).await?;
            if webhook["url"].as_str().is_none_or(|url|!url.is_empty()){return Err(ApiError(StatusCode::CONFLICT,"telegram_webhook_active"));}
            // Enabling management must never execute commands sent while access was disabled.
            let pending=transport::call(state,&token,"getUpdates",json!({"offset":-1,"limit":1,"timeout":0,"allowed_updates":["message","callback_query"]})).await?;
            let next=pending.as_array().ok_or_else(bad)?.last().and_then(|u|u["update_id"].as_i64()).unwrap_or(-1).saturating_add(1);
            transport::call(state,&token,"setMyCommands",json!({"scope":{"type":"all_private_chats"},"commands":[{"command":"start","description":"Главное меню Fleet"},{"command":"status","description":"Сводка по серверам"},{"command":"nodes","description":"Серверы и аналитика"},{"command":"groups","description":"Группы DNS"},{"command":"operations","description":"Мои операции"},{"command":"add","description":"Добавить сервер"},{"command":"cancel","description":"Отменить ввод"},{"command":"id","description":"Мой Telegram ID"}]})).await?;
            sqlx::query("UPDATE telegram_bot_settings SET update_offset=$1,username=$2,last_error=NULL,last_poll_at=now() WHERE singleton").bind(next).bind(me["username"].as_str()).execute(&state.db).await?;
            return Ok::<_,ApiError>(());
        }
        let updates=transport::call(state,&token,"getUpdates",json!({"offset":offset,"limit":1,"timeout":2,"allowed_updates":["message","callback_query"]})).await?;
        let updates=updates.as_array().filter(|v|v.len()<=1).ok_or_else(bad)?;
        if let Some(update)=updates.first() {
            let update_id=update["update_id"].as_i64().filter(|id|*id>=offset.unwrap_or(0)&&*id<i64::MAX).ok_or_else(bad)?;
            // Claim before any side effect. A lost process never blindly repeats a DNS or enrollment command.
            sqlx::query("UPDATE telegram_bot_settings SET update_offset=$1 WHERE singleton").bind(update_id+1).execute(&state.db).await?;
            handle(state,&token,row.get("revision"),update).await?;
        }
        sqlx::query("UPDATE telegram_bot_settings SET last_poll_at=now(),last_error=NULL,next_poll_at=greatest(next_poll_at,now()) WHERE singleton").execute(&state.db).await?;Ok(())
    }).await;
    let error=match outcome {Ok(Ok(()))=>None,Ok(Err(e))=>Some(e.1),Err(_)=>Some("telegram_action_timeout")};
    if let Some(code)=error {sqlx::query("UPDATE telegram_bot_settings SET last_error=$1,next_poll_at=greatest(next_poll_at,now()+interval '30 seconds') WHERE singleton").bind(code).execute(&state.db).await?;}
    fence.rollback().await?;Ok(())
}

async fn handle(state:&State,token:&str,revision:i64,update:&Value)->Result<()> {
    let callback=update.get("callback_query");
    let message=callback.map(|q|&q["message"]).unwrap_or(&update["message"]);
    let from=callback.map(|q|&q["from"]).unwrap_or(&message["from"]);
    let Some(telegram)=from["id"].as_i64().filter(|id|*id>0)else{return Ok(());};
    if from["is_bot"]==true||message["chat"]["type"]!="private"||message["chat"]["id"].as_i64()!=Some(telegram){return Ok(());}
    let input=if let Some(callback)=callback {callback["data"].as_str().unwrap_or("")}else{message["text"].as_str().unwrap_or("").trim()};
    if input.len()>1024{return Ok(());}
    if callback.is_none() && message["date"].as_i64().is_none_or(|date|chrono::Utc::now().timestamp().abs_diff(date)>300){return Ok(());}
    let Some(user)=identity(state,telegram).await?else{
        if ["/start","/id"].contains(&input){transport::message(state,token,telegram,&format!("Ваш Telegram ID: {telegram}. Доступ к Fleet не настроен. Попросите владельца добавить этот ID в «Система → Telegram-бот»."),vec![]).await?;}
        return Ok(());
    };
    if let Some(id)=callback.and_then(|c|c["id"].as_str()).filter(|id|id.len()<=256) {
        let _=transport::call(state,token,"answerCallbackQuery",json!({"callback_query_id":id})).await;
    }
    let result=conversation::handle(state,token,&user,telegram,revision,input,callback.is_some()).await;
    if let Err(error)=result {
        if error.1=="telegram_rate_limited"{return Err(error);}
        transport::message(state,token,telegram,&format!("{}\nКод: {}",transport::explain(error.1),error.1),vec![vec![transport::button("Главное меню","menu")]]).await?;
    }
    Ok(())
}
