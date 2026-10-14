use crate::state::State;
use serde_json::Value;
use sqlx::Row;
use uuid::Uuid;

/// Reads may use authenticated fresh samples; mutations use a durable delivery receipt.
pub(crate) async fn request(state:&State,id:Uuid,method:reqwest::Method,path:&str,body:Option<&Value>,revision:Option<&str>)->anyhow::Result<(u16,Value)>{
    anyhow::ensure!(telemt_managed::allowed_command(method.as_str(),path),"managed profile settings must be changed through the profile");
    if method==reqwest::Method::GET {
        let row=sqlx::query("SELECT cached_reads_cipher FROM managed_nodes WHERE node_id=$1 AND NOT revoked AND cached_at>now()-interval '7 seconds'").bind(id).fetch_optional(&state.db).await?;
        if let Some(cipher)=row.and_then(|r|r.get::<Option<String>,_>("cached_reads_cipher")){
            let raw=state.vault.open(&cipher,&format!("node-cache:{id}"))?;let values:Value=serde_json::from_str(&raw)?;
            if let Some(value)=values.get(path){return Ok((value["status"].as_u64().unwrap_or(502) as u16,value["response"].clone()));}
        }
    }
    let mut tx=state.db.begin().await?;
    sqlx::query("SELECT node_id FROM managed_nodes WHERE node_id=$1 AND NOT revoked FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
    let count:i64=sqlx::query_scalar("SELECT count(*) FROM node_commands WHERE node_id=$1 AND expires_at>now() AND state IN ('queued','claimed')").bind(id).fetch_one(&mut *tx).await?;
    anyhow::ensure!(count<64,"managed command queue full");
    let command=telemt_managed::Command{id:Uuid::new_v4().to_string(),method:method.to_string(),path:path.into(),body:body.cloned(),revision:revision.map(str::to_owned)};
    let cid=Uuid::parse_str(&command.id)?;let cipher=state.vault.seal(&serde_json::to_string(&command)?,&format!("node-command:{id}:{cid}"))?;
    sqlx::query("INSERT INTO node_commands(id,node_id,request_cipher,expires_at) VALUES($1,$2,$3,now()+interval '10 seconds')").bind(cid).bind(id).bind(cipher).execute(&mut *tx).await?;tx.commit().await?;
    let until=tokio::time::Instant::now()+std::time::Duration::from_secs(9);
    while tokio::time::Instant::now()<until{
        let cipher:Option<String>=sqlx::query_scalar("SELECT response_cipher FROM node_commands WHERE id=$1").bind(cid).fetch_one(&state.db).await?;
        if let Some(cipher)=cipher{let raw=state.vault.open(&cipher,&format!("node-receipt:{id}:{cid}"))?;let receipt:telemt_managed::Receipt=serde_json::from_str(&raw)?;return Ok((receipt.status,receipt.response));}
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    anyhow::bail!("managed command response unknown")
}
