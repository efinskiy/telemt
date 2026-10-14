use crate::state::State;
use anyhow::{Context,ensure};
use instant_acme::{Account,NewAccount,NewOrder,Identifier,OrderStatus,ChallengeType,AuthorizationStatus};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

// A dedicated transaction holds this lease; durable order writes use separate transactions.
pub(super) const CERTIFICATE_LOCK:i64=78213421;
const STEP_DEADLINE:std::time::Duration=std::time::Duration::from_secs(40);

pub(crate) async fn tick(state:&State)->anyhow::Result<()> {
    let mut lease=state.db.begin().await?;
    let locked:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(CERTIFICATE_LOCK).fetch_one(&mut *lease).await?;
    if !locked{return Ok(());}
    let id:Option<Uuid>=sqlx::query_scalar("SELECT c.id FROM certificates c WHERE c.next_attempt<=now() AND (c.issuer_cipher IS NOT NULL OR EXISTS(SELECT 1 FROM certificate_orders o WHERE o.certificate_id=c.id)) ORDER BY next_attempt LIMIT 1").fetch_optional(&state.db).await?;
    if let Some(id)=id {
        let outcome=tokio::time::timeout(STEP_DEADLINE,step(state,id)).await;
        if !matches!(outcome,Ok(Ok(()))) {
            // Provider errors may contain challenge or account details; expose a stable category only.
            sqlx::query("UPDATE certificates SET last_error='certificate_provider_failed',next_attempt=now()+interval '5 minutes' WHERE id=$1").bind(id).execute(&state.db).await?;
        }
    }
    lease.rollback().await?;Ok(())
}
async fn save_order(state:&State,id:Uuid,doc:&Value)->anyhow::Result<()> {
    let cipher=state.vault.seal(&doc.to_string(),&format!("certificate-order:{id}"))?;
    sqlx::query("INSERT INTO certificate_orders(certificate_id,document_cipher) VALUES($1,$2) ON CONFLICT(certificate_id) DO UPDATE SET document_cipher=$2,updated_at=now()").bind(id).bind(cipher).execute(&state.db).await?;Ok(())
}
async fn step(state:&State,id:Uuid)->anyhow::Result<()> {
    let row=sqlx::query("SELECT * FROM certificates WHERE id=$1").bind(id).fetch_one(&state.db).await?;
    let domain:String=row.get("domain");let revision:i64=row.get("revision");
    let order:Option<String>=sqlx::query_scalar("SELECT document_cipher FROM certificate_orders WHERE certificate_id=$1").bind(id).fetch_optional(&state.db).await?;
    let mut document=order.map(|c|state.vault.open(&c,&format!("certificate-order:{id}"))).transpose()?.map(|s|serde_json::from_str::<Value>(&s)).transpose()?.unwrap_or(json!({}));
    if document["done"]==true || document["failed"]==true {
        if let Some(value)=document["dns_value"].as_str(){super::provider::cleanup(state,&document["issuer"],&format!("_acme-challenge.{domain}"),value).await?;}
        sqlx::query("DELETE FROM certificate_orders WHERE certificate_id=$1").bind(id).execute(&state.db).await?;
        sqlx::query("UPDATE certificates SET next_attempt=now()+interval '12 hours',status=CASE WHEN revision>0 THEN 'ready' ELSE 'empty' END WHERE id=$1").bind(id).execute(&state.db).await?;return Ok(());
    }
    if row.get::<Option<String>,_>("issuer_cipher").is_none(){document["failed"]=json!(true);save_order(state,id,&document).await?;return Ok(());}
    let mut issuer=if document["issuer"].is_object(){document["issuer"].clone()}else{
        let cipher=row.get::<Option<String>,_>("issuer_cipher").context("issuer disabled")?;
        serde_json::from_str::<Value>(&state.vault.open(&cipher,&format!("issuer:{id}"))?)?
    };
    let account=if issuer["account"].is_object(){Account::builder()?.from_credentials(serde_json::from_value(issuer["account"].clone())?).await?}else{
        let contact=format!("mailto:{}",issuer["email"].as_str().context("email missing")?);
        let (account,credentials)=Account::builder()?.create(&NewAccount{contact:&[&contact],terms_of_service_agreed:true,only_return_existing:false},issuer["directory"].as_str().context("directory missing")?.into(),None).await?;
        issuer["account"]=serde_json::to_value(credentials)?;
        let cipher=state.vault.seal(&issuer.to_string(),&format!("issuer:{id}"))?;
        sqlx::query("UPDATE certificates SET issuer_cipher=$2 WHERE id=$1").bind(id).bind(cipher).execute(&state.db).await?;account
    };
    if document["url"].is_null() {
        let info:Value=row.get("info");let now=chrono::Utc::now().timestamp();
        // Lifetime-relative fallback also works with short-lived certificates.
        let mut due=info["renew_at"].as_i64().unwrap_or(0);
        let mut ari_check=info["ari_check_at"].as_i64().unwrap_or(0);
        if revision>0 && row.get::<String,_>("status")!="renew_requested" {
            if let Some(material)=row.get::<Option<String>,_>("material_cipher").filter(|_|ari_check<=now) {
                let raw=state.vault.open(&material,&format!("certificate:{id}:{revision}"))?;let material:Value=serde_json::from_str(&raw)?;
                let chain=material["fullchain"].as_str().context("chain missing")?;
                let leaf={rustls_pemfile::certs(&mut chain.as_bytes()).next()};
                if let Some(Ok(leaf))=leaf
                    && let Ok(identifier)=instant_acme::CertificateIdentifier::try_from(&leaf)
                    && let Ok((renewal,retry))=account.renewal_info(&identifier).await {
                            ari_check=now.saturating_add(i64::try_from(retry.as_secs()).unwrap_or(i64::MAX).max(60));
                            let start=renewal.suggested_window.start.unix_timestamp();let end=renewal.suggested_window.end.unix_timestamp();
                            if end>start && (info["ari_start"].as_i64()!=Some(start)||info["ari_end"].as_i64()!=Some(end)){
                                let random=u64::from_str_radix(&crate::crypto::random()[..16],16)?;
                                due=if start<=now{now}else{start+(random%((end-start) as u64)) as i64};
                                sqlx::query("UPDATE certificates SET info=info||$2::jsonb WHERE id=$1").bind(id).bind(json!({"renew_at":due,"ari_start":start,"ari_end":end})).execute(&state.db).await?;
                            }
                }
            }
            if ari_check<=now{ari_check=now+43200;}
            sqlx::query("UPDATE certificates SET info=info||$2::jsonb WHERE id=$1").bind(id).bind(json!({"ari_check_at":ari_check})).execute(&state.db).await?;
            if due>now {sqlx::query("UPDATE certificates SET next_attempt=to_timestamp($2),last_error=NULL WHERE id=$1").bind(id).bind((due.min(ari_check)) as f64).execute(&state.db).await?;return Ok(());}
        }
        let identifiers=[Identifier::Dns(domain.clone())];let order=account.new_order(&NewOrder::new(&identifiers)).await?;
        let key=rcgen::KeyPair::generate()?;let mut params=rcgen::CertificateParams::new(vec![domain.clone()])?;params.distinguished_name=rcgen::DistinguishedName::new();
        let csr=params.serialize_request(&key)?;
        document=json!({"url":order.url(),"key":key.serialize_pem(),"csr":hex::encode(csr.der()),"issuer":issuer,"created_at":now});
        save_order(state,id,&document).await?;
        sqlx::query("UPDATE certificates SET status='issuing',last_error=NULL,next_attempt=now()+interval '1 minute' WHERE id=$1").bind(id).execute(&state.db).await?;return Ok(());
    }
    let mut order=account.order(document["url"].as_str().context("order URL missing")?.into()).await?;
    let status=order.state().status;
    if chrono::Utc::now().timestamp()-document["created_at"].as_i64().unwrap_or(0)>86400 && status!=OrderStatus::Valid {document["failed"]=json!(true);save_order(state,id,&document).await?;anyhow::bail!("order expired");}
    match status {
        OrderStatus::Pending=>{
            let mut authorizations=order.authorizations();let mut count=0;
            while let Some(auth)=authorizations.next().await {
                count+=1;ensure!(count<=1,"unexpected authorization count");let mut auth=auth?;
                if auth.status==AuthorizationStatus::Valid{continue;}
                let mut challenge=auth.challenge(ChallengeType::Dns01).context("DNS-01 unavailable")?;
                let value=challenge.key_authorization().dns_value();
                if document["dns_value"].is_null(){document["dns_value"]=json!(value);save_order(state,id,&document).await?;}
                ensure!(document["dns_value"]==value,"challenge changed");
                super::provider::present(state,&issuer,&format!("_acme-challenge.{domain}"),&value).await?;
                if document["presented_at"].is_null(){document["presented_at"]=json!(chrono::Utc::now().timestamp());save_order(state,id,&document).await?;}
                // Give authoritative DNS propagation a full coordinator interval before validation.
                if chrono::Utc::now().timestamp()-document["presented_at"].as_i64().unwrap_or(0)>=60{challenge.set_ready().await?;}
            }
        }
        OrderStatus::Ready=>order.finalize_csr(&hex::decode(document["csr"].as_str().context("CSR missing")?)?).await?,
        OrderStatus::Processing=>{},
        OrderStatus::Valid=>{
            let chain=order.certificate().await?.context("certificate pending")?;let key=document["key"].as_str().context("key missing")?;
            let mut tx=state.db.begin().await?;store(state,&mut tx,id,&domain,&chain,key).await?;
            document["done"]=json!(true);
            let cipher=state.vault.seal(&document.to_string(),&format!("certificate-order:{id}"))?;
            sqlx::query("UPDATE certificate_orders SET document_cipher=$2 WHERE certificate_id=$1").bind(id).bind(cipher).execute(&mut *tx).await?;tx.commit().await?;
        }
        OrderStatus::Invalid=>{document["failed"]=json!(true);save_order(state,id,&document).await?;anyhow::bail!("ACME order rejected");}
    }
    sqlx::query("UPDATE certificates SET next_attempt=now()+interval '1 minute' WHERE id=$1").bind(id).execute(&state.db).await?;Ok(())
}

pub(super) async fn store(state:&State,tx:&mut sqlx::Transaction<'_,sqlx::Postgres>,id:Uuid,domain:&str,chain:&str,key:&str)->anyhow::Result<()> {
    let info=telemt_managed::certificate::validate(chain,key,domain)?;
    let mut public=serde_json::to_value(&info)?;public["renew_at"]=json!(info.not_before+(info.not_after-info.not_before)*2/3);
    let revision:i64=sqlx::query_scalar("SELECT revision+1 FROM certificates WHERE id=$1 FOR UPDATE").bind(id).fetch_one(&mut **tx).await?;
    let material=state.vault.seal(&json!({"fullchain":chain,"private_key":key}).to_string(),&format!("certificate:{id}:{revision}"))?;
    sqlx::query("INSERT INTO certificate_versions(certificate_id,revision,material_cipher,info) VALUES($1,$2,$3,$4)").bind(id).bind(revision).bind(&material).bind(&public).execute(&mut **tx).await?;
    sqlx::query("UPDATE certificates SET revision=$2,material_cipher=$3,info=$4,status='ready',last_error=NULL,next_attempt=now()+interval '12 hours',updated_at=now() WHERE id=$1").bind(id).bind(revision).bind(material).bind(public).execute(&mut **tx).await?;Ok(())
}
