//! Outbound-only node supervisor. The last committed bundle remains usable without Fleet.
mod storage;
mod local;
mod process;
use anyhow::{Context,ensure};
use serde::{Serialize,Deserialize};
use serde_json::{Value,json};
use telemt_managed::{Bundle,Command,Receipt};
use rand::RngCore;
use std::time::Duration;

#[derive(Serialize,Deserialize)]
struct Identity { credential:String,installation_id:uuid::Uuid,registered:bool }
#[derive(Default,Serialize,Deserialize)]
struct Journal { receipts:Vec<Receipt>,attempted:i64,report:Option<Value> }

#[tokio::main]
async fn main()->anyhow::Result<()> {
    storage::init().await?;
    let client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).build()?;
    if std::env::args().nth(1).as_deref()==Some("healthcheck") {
        let bundle:Bundle=storage::load("active.json").await?.context("node not enrolled")?;
        local::ready(&client,&bundle).await?;local::public_tls(&bundle).await?;return Ok(());
    }
    let mut services=process::Services::new();
    if let Some(bundle)=storage::load::<Bundle>("active.json").await? && services.activate(bundle.clone(),&client).await.is_err(){
        // Retain the committed recovery candidate even when Fleet is also unreachable.
        services.bundle=Some(bundle);eprintln!("last configuration unavailable; retrying committed deployment");}
    let result=run(&mut services,&client).await;services.stop().await;result
}
async fn run(services:&mut process::Services,client:&reqwest::Client)->anyhow::Result<()> {
    let origin=url::Url::parse(&std::env::var("FLEET_URL")?)?;
    ensure!(origin.scheme()=="https"&&origin.host_str().is_some()&&origin.username().is_empty()&&origin.password().is_none()&&origin.path()=="/"&&origin.query().is_none()&&origin.fragment().is_none(),"FLEET_URL must be an HTTPS origin");
    let node=std::env::var("NODE_ID")?;uuid::Uuid::parse_str(&node)?;
    let mut builder=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(12));
    if let Ok(path)=std::env::var("MANAGED_CA_FILE"){builder=builder.add_root_certificate(reqwest::Certificate::from_pem(&storage::read(std::path::Path::new(&path),128*1024).await?)?);}
    let remote=builder.build()?;
    let mut identity=if let Some(identity)=storage::load::<Identity>("identity.json").await?{identity}else{
        let mut bytes=[0u8;32];rand::rng().fill_bytes(&mut bytes);let identity=Identity{credential:hex::encode(bytes),installation_id:uuid::Uuid::new_v4(),registered:false};storage::save("identity.json",&identity).await?;identity
    };
    let mut journal:Journal=storage::load("journal.json").await?.unwrap_or_default();
    if services.bundle.is_none(){journal.attempted=0;journal.report=None;}
    let mut delay=1u64;let shutdown=shutdown();tokio::pin!(shutdown);
    loop {
        tokio::select!{_=&mut shutdown=>break,_=tokio::time::sleep(Duration::from_secs(delay))=>{}}
        let step=async {
            if !services.alive()? && let Some(bundle)=services.bundle.clone(){services.stop().await;if services.activate(bundle.clone(),client).await.is_err(){journal.report=Some(report(&bundle,"failed",Some("service_restart_failed"))?);}}
            if !identity.registered {
                let enrollment=std::env::var("ENROLLMENT_TOKEN")?;
                post(&remote,&origin,"register",&identity.credential,json!({"node_id":node,"enrollment_token":enrollment,"credential":identity.credential,"installation_id":identity.installation_id})).await?;
                identity.registered=true;storage::save("identity.json",&identity).await?;
            }
            if let Some(bundle)=&services.bundle {
                let drift=services.drift().await.unwrap_or(true);
                if drift {journal.report=Some(report(bundle,"drift",Some("local_configuration_changed"))?);}
                else if bundle.revision==journal.attempted && journal.report.as_ref().is_some_and(|r|r["state"]!="applied") && local::ready(client,bundle).await.is_ok() && local::public_tls(bundle).await.is_ok(){
                    journal.report=Some(report(bundle,"applied",None)?);storage::save("journal.json",&journal).await?;
                }
            }
            let reads=if let Some(bundle)=&services.bundle {local::reads(client,bundle).await}else{json!({})};
            let response=post(&remote,&origin,"poll",&identity.credential,json!({"node_id":node,"revision":journal.attempted,"deployment":journal.report,"receipts":journal.receipts,"reads":reads})).await?;
            if response["receipts_accepted"]==true{journal.receipts.clear();storage::save("journal.json",&journal).await?;}
            if !response["bundle"].is_null(){
                let bundle:Bundle=serde_json::from_value(response["bundle"].clone())?;
                ensure!(bundle.revision>=journal.attempted,"stale deployment");
                let result=services.activate(bundle.clone(),client).await;
                // An unavailable first deployment must remain retryable after a transient failure.
                journal.attempted=if result.is_err()&&services.bundle.is_none(){0}else{bundle.revision};
                journal.report=Some(report(&bundle,if result.is_ok(){"applied"}else{"failed"},if result.is_err(){Some("validation_or_activation_failed")}else{None})?);
                storage::save("journal.json",&journal).await?;
                if services.bundle.is_none(){result?;}
            }
            let commands=response["commands"].as_array().context("commands missing")?;ensure!(commands.len()<=16,"command batch exceeds limit");
            for command in commands {
                let command:Command=serde_json::from_value(command.clone())?;
                ensure!(journal.receipts.len()<32,"receipt queue full");
                // A crash after this write reports uncertainty and never repeats a runtime mutation.
                journal.receipts.push(Receipt{id:command.id.clone(),status:504,response:json!({"ok":false,"error":{"code":"execution_unknown"}})});storage::save("journal.json",&journal).await?;
                if let Some(bundle)=&services.bundle && let Ok(receipt)=local::request(client,bundle,&command).await && let Some(last)=journal.receipts.last_mut(){*last=receipt;}
                storage::save("journal.json",&journal).await?;
            }
            Ok::<(),anyhow::Error>(())
        };
        tokio::select!{_=&mut shutdown=>break,result=step=>{if result.is_ok(){delay=1;}else{eprintln!("node synchronization failed; keeping last committed configuration");delay=(delay*2).min(30);}}}
    }Ok(())
}
fn report(bundle:&Bundle,state:&str,error:Option<&str>)->anyhow::Result<Value>{
    let fingerprint=telemt_managed::certificate::validate(&bundle.fullchain,&bundle.private_key,&bundle.domain).ok().map(|info|info.fingerprint);
    Ok(json!({"revision":bundle.revision,"digest":bundle.digest()?,"state":state,"error":error,"certificate_fingerprint":fingerprint}))
}
async fn post(client:&reqwest::Client,origin:&url::Url,action:&str,credential:&str,body:Value)->anyhow::Result<Value>{
    let response=client.post(origin.join(&format!("/node-agent/v1/{action}"))?).bearer_auth(credential).json(&body).send().await?.error_for_status()?;
    let response=local::response(response,telemt_managed::MAX_BUNDLE_BYTES+65536).await?;ensure!(response["ok"]==true,"Fleet rejected request");Ok(response["data"].clone())
}
async fn shutdown(){
    #[cfg(unix)] if let Ok(mut signal)=tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()){tokio::select!{_ = signal.recv()=>{},_ = tokio::signal::ctrl_c()=>{}}return;}
    let _=tokio::signal::ctrl_c().await;
}
