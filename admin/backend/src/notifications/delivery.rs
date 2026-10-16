//! Bounded retries of persisted events; ambiguous transport failures may duplicate a message.
use super::*;
use std::time::Duration;

struct Failure {code:&'static str,retry:bool,delay:i64}
async fn send(state:&State,token:&str,chat:&str,message:&str)->std::result::Result<(),Failure> {
    let origin="https://api.telegram.org";
    #[cfg(test)]
    let origin=state.telegram_origin.as_deref().unwrap_or(origin);
    // Never propagate reqwest errors: their URL contains the bot credential.
    let response=async {
        let mut response=state.client.post(format!("{origin}/bot{token}/sendMessage"))
            .json(&json!({"chat_id":chat,"text":message,"link_preview_options":{"is_disabled":true}})).send().await.map_err(|_|())?;
        let status=response.status();let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|_|())? {
            if bytes.len()+chunk.len()>64*1024{return Err(());}
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_,()>((status,serde_json::from_slice::<Value>(&bytes).map_err(|_|())?))
    };
    let (status,value)=tokio::time::timeout(Duration::from_secs(5),response).await.ok().and_then(|r|r.ok())
        .ok_or(Failure{code:"telegram_transport_error",retry:true,delay:0})?;
    if status.is_success()&&value["ok"]==true{return Ok(());}
    let code=value["error_code"].as_u64().unwrap_or(u64::from(status.as_u16()));
    Err(match code {
        429=>Failure{code:"telegram_rate_limited",retry:true,delay:value["parameters"]["retry_after"].as_i64().unwrap_or(60).clamp(1,86400)},
        401=>Failure{code:"telegram_invalid_token",retry:false,delay:0},
        403=>Failure{code:"telegram_forbidden",retry:false,delay:0},
        400=>Failure{code:"telegram_chat_error",retry:false,delay:0},
        _=>Failure{code:"telegram_api_error",retry:status.is_server_error()||code>=500,delay:0},
    })
}

/// One request per tick bounds shutdown latency and serializes all Fleet instances.
pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    let mut tx=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(DELIVERY_LOCK).fetch_one(&mut *tx).await?;
    if !locked{return Ok(());}
    sqlx::query("DELETE FROM notification_deliveries WHERE created_at<now()-interval '30 days'").execute(&mut *tx).await?;
    sqlx::query("UPDATE notification_deliveries SET status='failed',error='notification_expired' WHERE status='queued' AND created_at<now()-interval '24 hours'").execute(&mut *tx).await?;
    let settings=sqlx::query("SELECT * FROM notification_settings WHERE singleton").fetch_one(&mut *tx).await?;
    let revision:i64=settings.get("revision");
    sqlx::query("UPDATE notification_deliveries SET status='cancelled',error='notification_settings_changed' WHERE status='queued' AND revision<>$1").bind(revision).execute(&mut *tx).await?;
    let Some(row)=sqlx::query("SELECT id,kind,message,attempts FROM notification_deliveries WHERE status='queued' AND next_attempt<=now() ORDER BY next_attempt,created_at LIMIT 1 FOR UPDATE SKIP LOCKED").fetch_optional(&mut *tx).await? else{tx.commit().await?;return Ok(());};
    let id:Uuid=row.get("id");let kind:String=row.get("kind");
    if kind!="test"&&(!settings.get::<bool,_>("enabled")||!settings.get::<bool,_>(kind.as_str())) {
        sqlx::query("UPDATE notification_deliveries SET status='cancelled' WHERE id=$1").bind(id).execute(&mut *tx).await?;
        tx.commit().await?;return Ok(());
    }
    let cipher:Option<String>=settings.get("token_cipher");
    let token=cipher.as_deref().and_then(|cipher|state.vault.open(cipher,TOKEN_PURPOSE).ok());
    let result=if let Some(token)=token {send(state,&token,&settings.get::<String,_>("chat_id"),&row.get::<String,_>("message")).await}else{Err(Failure{code:"notification_not_configured",retry:false,delay:0})};
    let attempts=row.get::<i32,_>("attempts")+1;
    match result {
        Ok(())=>{sqlx::query("UPDATE notification_deliveries SET status='sent',attempts=$2,error=NULL,sent_at=now() WHERE id=$1").bind(id).bind(attempts).execute(&mut *tx).await?;}
        Err(failure)=>{
            let status=if failure.retry&&attempts<8{"queued"}else{"failed"};
            let delay=failure.delay.max((15_i64*2_i64.pow(attempts.min(8) as u32)).min(3600));
            sqlx::query("UPDATE notification_deliveries SET status=$2,attempts=$3,error=$4,next_attempt=now()+make_interval(secs=>$5) WHERE id=$1").bind(id).bind(status).bind(attempts).bind(failure.code).bind(delay as f64).execute(&mut *tx).await?;
        }
    }
    tx.commit().await?;Ok(())
}
