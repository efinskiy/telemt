//! Owner settings and a transactional outbox for confirmed DNS changes.
//! Delivery runs independently so Telegram outages cannot delay DNS writes.
mod delivery;
pub(crate) use delivery::tick;
use axum::{http::{Method,StatusCode},Json};
use serde_json::{Value,json};
use sqlx::{Row,Postgres,Transaction};
use uuid::Uuid;
use crate::{auth::Session,error::{ApiError,Result,bad},state::{State,ok}};

// Separate from the DNS coordinator; serialize credential updates and outbound delivery.
const DELIVERY_LOCK:i64=78_213_422;
const TOKEN_PURPOSE:&str="telegram-notifications";

fn invalid(code:&'static str)->ApiError {ApiError(StatusCode::BAD_REQUEST,code)}
fn valid_token(token:&str)->bool {
    token.len()<=256 && token.split_once(':').is_some_and(|(id,key)| !id.is_empty() && id.bytes().all(|c|c.is_ascii_digit()) && key.len()>=20 && key.bytes().all(|c|c.is_ascii_alphanumeric()||b"_-".contains(&c)))
}
fn valid_chat(chat:&str)->bool {
    chat.len()<=128 && (chat.parse::<i64>().is_ok_and(|n|n!=0) || chat.strip_prefix('@').is_some_and(|name|name.len()>=5 && name.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'_')))
}
async fn read(state:&State)->Result<Value> {
    let r=sqlx::query("SELECT * FROM notification_settings WHERE singleton").fetch_one(&state.db).await?;
    Ok(json!({"revision":r.get::<i64,_>("revision"),"enabled":r.get::<bool,_>("enabled"),"chat_id":r.get::<String,_>("chat_id"),"token_configured":r.get::<Option<String>,_>("token_cipher").is_some(),"ip_rotation":r.get::<bool,_>("ip_rotation"),"server_switch":r.get::<bool,_>("server_switch")}))
}

/// All settings and delivery history are restricted to owners, including reads.
pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>> {
    user.require("owner")?;
    match parts {
        ["notifications"] if method==Method::GET=>Ok(Json(ok(read(state).await?))),
        ["notifications","history"] if method==Method::GET=>{
            let rows=sqlx::query("SELECT id,kind,status,attempts,error,created_at,sent_at,message FROM notification_deliveries ORDER BY created_at DESC LIMIT 50").fetch_all(&state.db).await?;
            Ok(Json(ok(json!(rows.iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"kind":r.get::<String,_>("kind"),"status":r.get::<String,_>("status"),"attempts":r.get::<i32,_>("attempts"),"error":r.get::<Option<String>,_>("error"),"created_at":r.get::<chrono::DateTime<chrono::Utc>,_>("created_at"),"sent_at":r.get::<Option<chrono::DateTime<chrono::Utc>>,_>("sent_at"),"message":r.get::<String,_>("message")})).collect::<Vec<_>>()))))
        }
        ["notifications"] if method==Method::PUT=>{
            let revision=body["revision"].as_i64().ok_or_else(bad)?;
            let enabled=body["enabled"].as_bool().ok_or_else(bad)?;
            let ip=body["ip_rotation"].as_bool().ok_or_else(bad)?;
            let server=body["server_switch"].as_bool().ok_or_else(bad)?;
            let chat=body["chat_id"].as_str().ok_or_else(bad)?.trim();
            if !chat.is_empty()&&!valid_chat(chat){return Err(invalid("notification_chat_invalid"));}
            let token=body.get("bot_token").and_then(Value::as_str).unwrap_or("").trim();
            if !token.is_empty()&&!valid_token(token){return Err(invalid("notification_token_invalid"));}
            let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(DELIVERY_LOCK).execute(&mut *tx).await?;
            let current=sqlx::query("SELECT revision,token_cipher FROM notification_settings WHERE singleton FOR UPDATE").fetch_one(&mut *tx).await?;
            if current.get::<i64,_>("revision")!=revision{return Err(ApiError(StatusCode::CONFLICT,"notification_revision_conflict"));}
            let cipher=if body["clear_token"]==true {None}else if token.is_empty(){current.get::<Option<String>,_>("token_cipher")}else{Some(state.vault.seal(token,TOKEN_PURPOSE)?)};
            if enabled && (chat.is_empty()||cipher.is_none()){return Err(invalid("notification_not_configured"));}
            sqlx::query("UPDATE notification_settings SET revision=revision+1,enabled=$1,chat_id=$2,token_cipher=$3,ip_rotation=$4,server_switch=$5 WHERE singleton").bind(enabled).bind(chat).bind(cipher).bind(ip).bind(server).execute(&mut *tx).await?;
            // Never redirect old queued events to a newly configured recipient or bot.
            sqlx::query("UPDATE notification_deliveries SET status='cancelled',error='notification_settings_changed' WHERE status='queued'").execute(&mut *tx).await?;
            sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'notifications.update','telegram',$2)").bind(user.id).bind(json!({"enabled":enabled,"ip_rotation":ip,"server_switch":server})).execute(&mut *tx).await?;
            tx.commit().await?;Ok(Json(ok(read(state).await?)))
        }
        ["notifications","test"] if method==Method::POST=>{
            let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(DELIVERY_LOCK).execute(&mut *tx).await?;
            let r=sqlx::query("SELECT * FROM notification_settings WHERE singleton").fetch_one(&mut *tx).await?;
            if r.get::<String,_>("chat_id").is_empty()||r.get::<Option<String>,_>("token_cipher").is_none(){return Err(invalid("notification_not_configured"));}
            let recent:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notification_deliveries WHERE kind='test' AND created_at>now()-interval '30 seconds')").fetch_one(&mut *tx).await?;
            if recent{return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,"notification_test_rate_limit"));}
            let id=Uuid::new_v4();
            sqlx::query("INSERT INTO notification_deliveries(id,revision,kind,message) VALUES($1,$2,'test',$3)").bind(id).bind(r.get::<i64,_>("revision")).bind("Telemt Fleet: проверочное уведомление. Доставка в Telegram работает.").execute(&mut *tx).await?;
            sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'notifications.test',$2,'{}')").bind(user.id).bind(id.to_string()).execute(&mut *tx).await?;
            tx.commit().await?;Ok(Json(ok(json!({"id":id}))))
        }
        _=>Err(bad()),
    }
}

fn text(value:Option<&str>)->String {
    value.unwrap_or("—").chars().filter(|c|!c.is_control()).take(160).collect()
}

/// Called before replacing the group's observed records, within DNS confirmation's transaction.
pub(crate) async fn enqueue(tx:&mut Transaction<'_,Postgres>,change:Uuid,group:Uuid,target:Uuid,actual:&Value,actor:Option<Uuid>)->Result<()> {
    let r=sqlx::query("SELECT g.name,g.document,g.observed,g.active_node,old.name AS old_name,new.name AS new_name FROM failover_groups g LEFT JOIN nodes old ON old.id=g.active_node LEFT JOIN nodes new ON new.id=$2 WHERE g.id=$1").bind(group).bind(target).fetch_one(&mut **tx).await?;
    let old:Option<Uuid>=r.get("active_node");let observed:Value=r.get("observed");
    let kind=if old!=Some(target){"server_switch"}else if ["A","AAAA"].iter().any(|f|observed[*f]["content"]!=actual[*f]["content"]){"ip_rotation"}else{return Ok(());};
    let settings=sqlx::query("SELECT * FROM notification_settings WHERE singleton").fetch_one(&mut **tx).await?;
    if !settings.get::<bool,_>("enabled")||!settings.get::<bool,_>(kind){return Ok(());}
    let document:Value=r.get("document");
    let mut message=format!("Telemt Fleet: {}\nГруппа: {}\nДомен: {}\nСервер: {} → {}\nРежим: {}\n",if kind=="ip_rotation"{"IP изменён"}else{"активный сервер изменён"},text(Some(&r.get::<String,_>("name"))),text(document["domain"].as_str()),text(r.get::<Option<String>,_>("old_name").as_deref()),text(r.get::<Option<String>,_>("new_name").as_deref()),if actor.is_some(){"ручной"}else{"автоматический"});
    for family in ["A","AAAA"] {if actual[family].is_object(){message.push_str(&format!("{family}: {} → {}\n",text(observed[family]["content"].as_str()),text(actual[family]["content"].as_str())));}}
    message.push_str(&format!("DNS подтверждён: {} UTC\nОперация: {change}",chrono::Utc::now().format("%Y-%m-%d %H:%M:%S")));
    sqlx::query("INSERT INTO notification_deliveries(id,dns_change,revision,kind,message) VALUES($1,$2,$3,$4,$5) ON CONFLICT(dns_change) DO NOTHING").bind(Uuid::new_v4()).bind(change).bind(settings.get::<i64,_>("revision")).bind(kind).bind(message).execute(&mut **tx).await?;Ok(())
}
