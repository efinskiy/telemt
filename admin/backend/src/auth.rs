use argon2::{Argon2, PasswordHasher, PasswordVerifier, password_hash::{SaltString, PasswordHash, rand_core::OsRng}};
use axum::{extract::State as ExtractState, http::{HeaderMap, StatusCode}, Json};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;
use crate::{crypto, state::{State,audit,ok}, error::{ApiError,Result,bad}};

#[derive(Clone,Serialize)]
pub(crate) struct Session { pub id: Uuid, pub username: String, pub role: String }
impl Session {
    pub(crate) fn require(&self, role: &str) -> Result<()> {
        let granted = match role { "owner" => self.role=="owner", "operator" => self.role=="owner" || self.role=="operator", _ => true };
        if granted { Ok(()) } else { Err(ApiError(StatusCode::FORBIDDEN,"forbidden")) }
    }
}

fn cookie(headers: &HeaderMap) -> Option<&str> { headers.get("cookie")?.to_str().ok()?.split(';').find_map(|part| part.trim().strip_prefix("telemt_session=")) }
pub(crate) fn origin(state: &State, headers: &HeaderMap) -> Result<()> {
    if headers.get("origin").and_then(|v|v.to_str().ok()) != Some(state.origin.as_ref()) { return Err(ApiError(StatusCode::FORBIDDEN,"origin_denied")); } Ok(())
}
pub(crate) async fn session(state: &State, headers: &HeaderMap, mutate: bool) -> Result<Session> {
    let token=cookie(headers).ok_or(ApiError(StatusCode::UNAUTHORIZED,"login_required"))?;
    if token.len()!=64 { return Err(ApiError(StatusCode::UNAUTHORIZED,"login_required")); }
    let row=sqlx::query("SELECT a.id,a.username,a.role,s.csrf_hash FROM sessions s JOIN administrators a ON a.id=s.administrator_id WHERE s.token_hash=$1 AND s.expires_at>now() AND a.enabled")
        .bind(crypto::hash(token)).fetch_optional(&state.db).await?.ok_or(ApiError(StatusCode::UNAUTHORIZED,"login_required"))?;
    if mutate { origin(state,headers)?; let csrf=headers.get("x-csrf-token").and_then(|v|v.to_str().ok()).ok_or(ApiError(StatusCode::FORBIDDEN,"csrf_denied"))?;
        if !crypto::equal(&crypto::hash(csrf), &row.get::<String,_>("csrf_hash")) { return Err(ApiError(StatusCode::FORBIDDEN,"csrf_denied")); }
    }
    Ok(Session { id:row.get("id"),username:row.get("username"),role:row.get("role") })
}

#[derive(Deserialize)]
pub(crate) struct Login { username: String, password: String, totp: String }
pub(crate) async fn login(ExtractState(state): ExtractState<State>, headers: HeaderMap, Json(input): Json<Login>) -> Result<(HeaderMap,Json<Value>)> {
    origin(&state,&headers)?;
    if input.username.len()>128 || input.password.len()>1024 || input.totp.len()!=6 { return Err(bad()); }
    let global=sqlx::query("INSERT INTO login_limits(key,failures) VALUES('global',1) ON CONFLICT(key) DO UPDATE SET failures=CASE WHEN login_limits.window_start<now()-interval '1 minute' THEN 1 ELSE login_limits.failures+1 END,window_start=CASE WHEN login_limits.window_start<now()-interval '1 minute' THEN now() ELSE login_limits.window_start END RETURNING failures").fetch_one(&state.db).await?;
    if global.get::<i32,_>("failures")>60{return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,"login_rate_limited"));}
    let key=crypto::hash(&input.username);
    // A locked database row makes attempts bounded across workers and controller restarts.
    let mut tx=state.db.begin().await?;
    sqlx::query("INSERT INTO login_limits(key) VALUES($1) ON CONFLICT DO NOTHING").bind(&key).execute(&mut *tx).await?;
    let rate=sqlx::query("SELECT failures,window_start FROM login_limits WHERE key=$1 FOR UPDATE").bind(&key).fetch_one(&mut *tx).await?;
    let expired=Utc::now().signed_duration_since(rate.get::<chrono::DateTime<Utc>,_>("window_start")).num_seconds()>=300;
    if !expired && rate.get::<i32,_>("failures")>=5 { return Err(ApiError(StatusCode::TOO_MANY_REQUESTS,"login_rate_limited")); }
    sqlx::query("UPDATE login_limits SET failures=CASE WHEN $2 THEN 1 ELSE failures+1 END, window_start=CASE WHEN $2 THEN now() ELSE window_start END WHERE key=$1").bind(&key).bind(expired).execute(&mut *tx).await?;
    tx.commit().await?;
    let row=sqlx::query("SELECT * FROM administrators WHERE username=$1 AND enabled").bind(&input.username).fetch_optional(&state.db).await?;
    let denied=ApiError(StatusCode::UNAUTHORIZED,"invalid_credentials");
    let stored: Option<String>=row.as_ref().map(|row|row.get("password_hash")); let password=zeroize::Zeroizing::new(input.password);
    let _hash_slot=state.hash_slots.clone().try_acquire_owned().map_err(|_|ApiError(StatusCode::TOO_MANY_REQUESTS,"login_busy"))?;
    let valid=tokio::task::spawn_blocking(move || match stored {
        Some(stored)=>PasswordHash::new(&stored).is_ok_and(|hash|Argon2::default().verify_password(password.as_bytes(),&hash).is_ok()),
        None=>{let _=Argon2::default().hash_password(password.as_bytes(),&SaltString::generate(&mut OsRng));false}
    }).await.map_err(|_|bad())?;
    if !valid { return Err(denied); }
    let row=row.ok_or(ApiError(StatusCode::UNAUTHORIZED,"invalid_credentials"))?;
    let id:Uuid=row.get("id"); let secret=state.vault.open(&row.get::<String,_>("totp_cipher"),&format!("admin:{id}"))?;
    let step=(Utc::now().timestamp()/30) as u64;
    let matched=[step.saturating_sub(1),step,step+1].into_iter().find(|step|crypto::totp(&secret,*step).is_ok_and(|code|crypto::equal(&code,&input.totp)));
    let Some(matched)=matched else { return Err(denied); };
    let mut tx=state.db.begin().await?;
    let changed=sqlx::query("UPDATE administrators SET last_totp_step=$2 WHERE id=$1 AND last_totp_step<$2 AND enabled AND password_hash=$3 AND totp_cipher=$4").bind(id).bind(matched as i64).bind(row.get::<String,_>("password_hash")).bind(row.get::<String,_>("totp_cipher")).execute(&mut *tx).await?;
    if changed.rows_affected()!=1 { return Err(denied); }
    let token=crypto::random(); let csrf=crypto::random();
    sqlx::query("INSERT INTO sessions(token_hash,administrator_id,csrf_hash,expires_at,csrf_cipher) VALUES($1,$2,$3,$4,$5)").bind(crypto::hash(&token)).bind(id).bind(crypto::hash(&csrf)).bind(Utc::now()+Duration::hours(8)).bind(state.vault.seal(&csrf,&format!("csrf:{}",crypto::hash(&token)))?).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM login_limits WHERE key=$1").bind(&key).execute(&mut *tx).await?; tx.commit().await?;
    audit(&state,Some(id),"auth.login",&id.to_string(),json!({"success":true})).await?;
    let user=Session{id,username:row.get("username"),role:row.get("role")};
    let mut headers=HeaderMap::new(); headers.insert("set-cookie",format!("telemt_session={token}; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=28800").parse().map_err(|_|bad())?);
    headers.insert("cache-control","no-store".parse().map_err(|_|bad())?);
    Ok((headers,Json(ok(json!({"user":user,"csrf":csrf})))))
}

pub(crate) async fn current(ExtractState(state):ExtractState<State>,headers:HeaderMap)->Result<Json<Value>> {
    let user=session(&state,&headers,false).await?;
    let key=crypto::hash(cookie(&headers).ok_or(bad())?);
    let cipher:String=sqlx::query_scalar("SELECT csrf_cipher FROM sessions WHERE token_hash=$1").bind(&key).fetch_one(&state.db).await?;
    let csrf=state.vault.open(&cipher,&format!("csrf:{key}"))?;
    Ok(Json(ok(json!({"user":user,"csrf":csrf.as_str()}))))
}
pub(crate) async fn logout(ExtractState(state):ExtractState<State>,headers:HeaderMap)->Result<(HeaderMap,Json<Value>)> {
    let user=session(&state,&headers,true).await?;
    sqlx::query("DELETE FROM sessions WHERE token_hash=$1").bind(crypto::hash(cookie(&headers).ok_or(bad())?)).execute(&state.db).await?;
    audit(&state,Some(user.id),"auth.logout",&user.id.to_string(),json!({})).await?;
    let mut response=HeaderMap::new(); response.insert("set-cookie","telemt_session=; Path=/; Secure; HttpOnly; SameSite=Strict; Max-Age=0".parse().map_err(|_|bad())?); Ok((response,Json(ok(json!({})))))
}

pub(crate) async fn create(state:&State,username:&str,password:&str,role:&str)->anyhow::Result<Value> {
    anyhow::ensure!(!username.is_empty() && username.len()<=128 && password.len()>=14 && password.len()<=1024 && ["owner","operator","viewer"].contains(&role),"invalid account parameters");
    let username=username.to_owned(); let password=password.to_owned();
    let hash=tokio::task::spawn_blocking(move || Argon2::default().hash_password(password.as_bytes(),&SaltString::generate(&mut OsRng)).map(|hash|hash.to_string()).map_err(|_|anyhow::anyhow!("password hashing failed"))).await??;
    let id=Uuid::new_v4(); let secret=crypto::random(); let cipher=state.vault.seal(&secret,&format!("admin:{id}"))?;
    sqlx::query("INSERT INTO administrators(id,username,password_hash,totp_cipher,role) VALUES($1,$2,$3,$4,$5)").bind(id).bind(&username).bind(hash).bind(cipher).bind(role).execute(&state.db).await?;
    let base32=crypto::base32(&hex::decode(secret)?);
    Ok(json!({"id":id,"totp_secret":base32,"enrollment_uri":format!("otpauth://totp/Telemt:{}?secret={base32}&issuer=Telemt",url::form_urlencoded::byte_serialize(username.as_bytes()).collect::<String>())}))
}

/// Physical recovery requires the database and vault key; it revokes every previous session.
pub(crate) async fn reset_owner(state:&State,username:&str,password:&str)->anyhow::Result<Value> {
    anyhow::ensure!((14..=1024).contains(&password.len()),"password must have 14..1024 bytes");
    let id:Uuid=sqlx::query_scalar("SELECT id FROM administrators WHERE username=$1 AND role='owner'").bind(username).fetch_one(&state.db).await?;
    let password=password.to_owned();
    let hash=tokio::task::spawn_blocking(move || Argon2::default().hash_password(password.as_bytes(),&SaltString::generate(&mut OsRng)).map(|hash|hash.to_string()).map_err(|_|anyhow::anyhow!("password hashing failed"))).await??;
    let secret=crypto::random();let cipher=state.vault.seal(&secret,&format!("admin:{id}"))?;
    let mut tx=state.db.begin().await?;
    sqlx::query("UPDATE administrators SET password_hash=$2,totp_cipher=$3,enabled=true,last_totp_step=-1 WHERE id=$1").bind(id).bind(hash).bind(cipher).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM sessions WHERE administrator_id=$1").bind(id).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'auth.physical_recovery',$2,'{}')").bind(id).bind(id.to_string()).execute(&mut *tx).await?;tx.commit().await?;
    Ok(json!({"id":id,"totp_secret":crypto::base32(&hex::decode(secret)?)}))
}
