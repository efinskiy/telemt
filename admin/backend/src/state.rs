use std::sync::Arc;
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use uuid::Uuid;
use crate::{crypto::Vault, error::{Result, bad}};

/// Shared PostgreSQL fence for fleet DNS, node enrollment and IP rotation.
pub(crate) const DNS_COORDINATOR_LOCK: i64 = 78_213_419;

#[derive(Clone)]
pub(crate) struct State {
    pub db: PgPool, pub vault: Arc<Vault>, pub client: reqwest::Client,
    pub hash_slots: Arc<tokio::sync::Semaphore>,
    #[cfg(test)]
    pub cloudflare_origin: Option<Arc<str>>,
    pub origin: Arc<str>, pub assets: Arc<std::path::PathBuf>,
}

pub(crate) async fn audit(state: &State, actor: Option<Uuid>, action: &str, target: &str, result: Value) -> Result<()> {
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,$2,$3,$4)").bind(actor).bind(action).bind(target).bind(redact(result)).execute(&state.db).await?; Ok(())
}

pub(crate) fn redact(mut value: Value) -> Value {
    match &mut value {
        Value::Object(map) => { map.retain(|key,_| !["secret","probe_secret","token","auth_header","password","totp","totp_secret","links","tls_link","secure_link","classic_link","user_links","credential_hashes"].contains(&key.as_str())); for v in map.values_mut() { *v = redact(v.take()); } }
        Value::Array(items) => { for item in items { *item = redact(item.take()); } }
        Value::String(text) if text.starts_with("ss://") || url::Url::parse(text).is_ok_and(|url| !url.username().is_empty() || url.password().is_some()) => { *text = "[redacted credential URL]".into(); }
        _ => {}
    } value
}

pub(crate) fn parse_id(value: &str) -> Result<Uuid> { value.parse().map_err(|_| bad()) }
pub(crate) fn field<'a>(v: &'a Value, key: &str) -> Result<&'a str> { v.get(key).and_then(Value::as_str).ok_or_else(bad) }
pub(crate) fn bounded_name(v: &Value, key: &str) -> Result<String> {
    let name = field(v,key)?; if name.is_empty() || name.len()>128 || name.chars().any(char::is_control) { return Err(bad()); } Ok(name.to_owned())
}
pub(crate) fn valid_secret(v: &str) -> bool { v.len()==32 && hex::decode(v).is_ok() }

pub(crate) async fn node(state: &State, id: Uuid) -> Result<sqlx::postgres::PgRow> {
    sqlx::query("SELECT * FROM nodes WHERE id=$1").bind(id).fetch_optional(&state.db).await?.ok_or_else(bad)
}

pub(crate) async fn node_request(state: &State, id: Uuid, method: reqwest::Method, path: &str, body: Option<&Value>, revision: Option<&str>) -> anyhow::Result<(u16, Value)> {
    let row = sqlx::query("SELECT api_url,token_cipher FROM nodes WHERE id=$1 AND enabled").bind(id).fetch_one(&state.db).await?;
    let url: String = row.get("api_url"); let cipher: String = row.get("token_cipher");
    let token = state.vault.open(&cipher,&format!("node:{id}"))?;
    let mut req = state.client.request(method, format!("{}{path}",url.trim_end_matches('/'))).header("Authorization",token.as_str());
    if let Some(revision)=revision { req=req.header("If-Match",revision); }
    if let Some(body)=body { req=req.json(body); }
    let mut response=req.send().await?; let status=response.status().as_u16(); let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? { anyhow::ensure!(bytes.len()+chunk.len()<=2*1024*1024,"node response too large"); bytes.extend_from_slice(&chunk); }
    Ok((status,serde_json::from_slice(&bytes)?))
}

pub(crate) fn fresh(at: Option<chrono::DateTime<Utc>>, seconds: i64) -> bool { at.is_some_and(|at| Utc::now().signed_duration_since(at).num_seconds() <= seconds) }
pub(crate) fn ok(data: Value) -> Value { json!({"ok":true,"data":data}) }
