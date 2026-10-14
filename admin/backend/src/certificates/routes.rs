use crate::{state::{State,field,parse_id,ok,audit,bounded_name},auth::Session,error::{Result,ApiError,bad}};
use axum::{http::{Method,StatusCode},Json};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn route(state:&State,user:&Session,parts:&[&str],method:&Method,body:Value)->Result<Json<Value>>{
    if parts==["certificates"] && *method==Method::GET {
        let rows=sqlx::query("SELECT id,name,domain,revision,info,status,last_error,next_attempt,issuer_cipher IS NOT NULL AS automatic FROM certificates ORDER BY name LIMIT 100").fetch_all(&state.db).await?;
        return Ok(Json(ok(json!(rows.into_iter().map(|r|json!({"id":r.get::<Uuid,_>("id"),"name":r.get::<String,_>("name"),"domain":r.get::<String,_>("domain"),"revision":r.get::<i64,_>("revision"),"info":r.get::<Value,_>("info"),"status":r.get::<String,_>("status"),"last_error":r.get::<Option<String>,_>("last_error"),"next_attempt":r.get::<chrono::DateTime<chrono::Utc>,_>("next_attempt"),"automatic":r.get::<bool,_>("automatic")})).collect::<Vec<_>>()))));
    }
    user.require("owner")?;
    match parts {
        ["certificates"] if *method==Method::POST=>{
            let id=Uuid::new_v4();let name=bounded_name(&body,"name")?;let domain=crate::profiles::domain(field(&body,"domain")?)?;
            sqlx::query("INSERT INTO certificates(id,name,domain) VALUES($1,$2,$3)").bind(id).bind(name).bind(domain).execute(&state.db).await?;
            audit(state,Some(user.id),"certificate.create",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({"id":id}))))
        }
        ["certificates",id,"import"] if *method==Method::POST=>{
            let id=parse_id(id)?;let domain:String=sqlx::query_scalar("SELECT domain FROM certificates WHERE id=$1").bind(id).fetch_one(&state.db).await?;
            let chain=field(&body,"fullchain")?;let key=field(&body,"private_key")?;
            telemt_managed::certificate::validate(chain,key,&domain).map_err(|_|ApiError(StatusCode::BAD_REQUEST,"certificate_invalid"))?;
            let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(super::worker::CERTIFICATE_LOCK).execute(&mut *tx).await?;
            let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM certificate_orders WHERE certificate_id=$1)").bind(id).fetch_one(&mut *tx).await?;
            if pending{return Err(ApiError(StatusCode::CONFLICT,"certificate_order_pending"));}
            super::worker::store(state,&mut tx,id,&domain,chain,key).await?;tx.commit().await?;
            audit(state,Some(user.id),"certificate.import",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({}))))
        }
        ["certificates",id,"issuer"] if *method==Method::PUT=>{
            let id=parse_id(id)?;let mut tx=state.db.begin().await?;
            sqlx::query("SELECT pg_advisory_xact_lock($1)").bind(super::worker::CERTIFICATE_LOCK).execute(&mut *tx).await?;
            let old:Option<String>=sqlx::query_scalar("SELECT issuer_cipher FROM certificates WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut *tx).await?;
            if body["enabled"]==false {
                sqlx::query("UPDATE certificates SET issuer_cipher=NULL,status=CASE WHEN revision>0 THEN 'ready' ELSE 'empty' END WHERE id=$1").bind(id).execute(&mut *tx).await?;
            }else{
                let mut issuer=old.as_ref().map(|c|state.vault.open(c,&format!("issuer:{id}"))).transpose()?.map(|s|serde_json::from_str::<Value>(&s)).transpose().map_err(|_|bad())?.unwrap_or(json!({}));
                let email=field(&body,"email")?;let zone=field(&body,"zone_id")?;
                if email.len()>254||!email.contains('@')||zone.len()!=32||hex::decode(zone).is_err(){return Err(bad());}
                if body["staging"]==true && std::env::var_os("MANAGED_CA_FILE").is_none(){return Err(ApiError(StatusCode::BAD_REQUEST,"staging_requires_test_trust"));}
                let directory=if body["staging"]==true {"https://acme-staging-v02.api.letsencrypt.org/directory"}else{"https://acme-v02.api.letsencrypt.org/directory"};
                if issuer["directory"]!=directory{issuer["account"]=Value::Null;}
                if let Some(token)=body["token"].as_str().filter(|s|!s.is_empty()){if token.len()>512||token.contains(['\r','\n']){return Err(bad());}issuer["token"]=json!(token);}
                if issuer["token"].as_str().is_none(){return Err(bad());}
                // A live order retains its original credentials until cleanup completes.
                let pending:Option<String>=sqlx::query_scalar("SELECT document_cipher FROM certificate_orders WHERE certificate_id=$1 FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
                if let Some(cipher)=pending{
                    let mut order:Value=serde_json::from_str(&state.vault.open(&cipher,&format!("certificate-order:{id}"))?).map_err(|_|bad())?;
                    if order["issuer"]["zone_id"]!=zone||order["issuer"]["directory"]!=directory{return Err(ApiError(StatusCode::CONFLICT,"certificate_order_pending"));}
                    // Rotate only DNS credentials; the persisted order must retain its ACME account.
                    order["issuer"]["token"]=issuer["token"].clone();issuer=order["issuer"].clone();
                    let cipher=state.vault.seal(&order.to_string(),&format!("certificate-order:{id}"))?;
                    sqlx::query("UPDATE certificate_orders SET document_cipher=$2,updated_at=now() WHERE certificate_id=$1").bind(id).bind(cipher).execute(&mut *tx).await?;
                }
                issuer["email"]=json!(email);issuer["zone_id"]=json!(zone);issuer["directory"]=json!(directory);
                let cipher=state.vault.seal(&issuer.to_string(),&format!("issuer:{id}"))?;
                sqlx::query("UPDATE certificates SET issuer_cipher=$2,next_attempt=now(),last_error=NULL WHERE id=$1").bind(id).bind(cipher).execute(&mut *tx).await?;
            }tx.commit().await?;audit(state,Some(user.id),"certificate.issuer",&id.to_string(),json!({"enabled":body["enabled"]!=false})).await?;Ok(Json(ok(json!({}))))
        }
        ["certificates",id,"renew"] if *method==Method::POST=>{
            let id=parse_id(id)?;
            sqlx::query("UPDATE certificates SET next_attempt=now(),status='renew_requested' WHERE id=$1 AND issuer_cipher IS NOT NULL").bind(id).execute(&state.db).await?;
            audit(state,Some(user.id),"certificate.renew",&id.to_string(),json!({})).await?;Ok(Json(ok(json!({}))))
        }
        _=>Err(bad())
    }
}
