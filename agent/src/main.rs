//! External probes poll outbound HTTPS only and never accept arbitrary targets from the public network.
mod protocol;
use serde::{Deserialize,Serialize};
use serde_json::{Value,json};
use std::time::Duration;

#[derive(Deserialize,Serialize)]
struct Job {id:String,node_id:String,round:i64,family:i32,ip:String,port:u16,sni:String,dc:i16,secret:String,expires_at:chrono::DateTime<chrono::Utc>}
impl Drop for Job {fn drop(&mut self){use zeroize::Zeroize;self.secret.zeroize();}}
async fn bounded(response:reqwest::Response,limit:usize)->anyhow::Result<Value>{let mut response=response;anyhow::ensure!(response.status().is_success(),"controller rejected request");let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{anyhow::ensure!(bytes.len()+chunk.len()<=limit,"oversized controller response");bytes.extend(chunk);}Ok(serde_json::from_slice(&bytes)?)}
#[tokio::main]
async fn main()->anyhow::Result<()> {
    let origin=std::env::var("PROBE_CONTROLLER")?;let url=url::Url::parse(&origin)?;anyhow::ensure!(url.scheme()=="https"&&url.host_str().is_some()&&url.path()=="/"&&url.query().is_none(),"HTTPS controller origin required");
    let reference=std::env::var("PROBE_REFERENCE_URL")?;let reference_url=url::Url::parse(&reference)?;anyhow::ensure!(reference_url.scheme()=="https"&&reference_url.host_str().is_some(),"HTTPS network reference required");
    let token=token_file(&std::env::var("PROBE_TOKEN_FILE")?).await?;
    let client=reqwest::Client::builder().timeout(Duration::from_secs(2)).redirect(reqwest::redirect::Policy::none()).no_proxy().build()?;
    let mut interval=tokio::time::interval(Duration::from_secs(2));interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let shutdown = shutdown_signal(); tokio::pin!(shutdown);
    loop {
        tokio::select!{_ = &mut shutdown=>break,_ = interval.tick()=>{}}
        let jobs=async{bounded(client.get(format!("{}/agent/jobs",origin.trim_end_matches('/'))).bearer_auth(token.trim()).send().await?,256*1024).await}.await;
        let Ok(jobs)=jobs else{eprintln!("probe controller unavailable");continue;};let jobs:Vec<Job>=serde_json::from_value(jobs["data"].clone())?;anyhow::ensure!(jobs.len()<=200,"too many jobs");
        let network_ok=client.get(&reference).send().await.is_ok_and(|r|r.status().is_success());
        let semaphore=std::sync::Arc::new(tokio::sync::Semaphore::new(200));let mut tasks=tokio::task::JoinSet::new();
        for job in jobs {let client=client.clone();let token=token.clone();let origin=origin.clone();let semaphore=semaphore.clone();tasks.spawn(async move{
            let permit=semaphore.acquire_owned().await?;let started=tokio::time::Instant::now();
            let result=if chrono::Utc::now()>=job.expires_at{Err("expired")}else if !network_ok{Err("network_unknown")}else{tokio::time::timeout(Duration::from_secs(5),protocol::probe(&job)).await.unwrap_or(Err("timeout"))};drop(permit);
            let body=json!({"id":job.id,"reachable":result.is_ok(),"network_ok":network_ok,"stage":result.err().unwrap_or("ok"),"latency_ms":started.elapsed().as_millis().min(5000) as u64});
            bounded(client.post(format!("{}/agent/results",origin.trim_end_matches('/'))).bearer_auth(token.trim()).json(&body).send().await?,16*1024).await?;Ok::<_,anyhow::Error>(())
        });}
        loop {
            tokio::select! {
                _ = &mut shutdown => { tasks.abort_all(); while tasks.join_next().await.is_some() {} return Ok(()); }
                result = tasks.join_next() => match result { Some(result) => { if result.is_err()||result.is_ok_and(|r|r.is_err()){eprintln!("probe result unavailable or expired");} }, None => break }
            }
        }
    }Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}

async fn token_file(path:&str)->anyhow::Result<zeroize::Zeroizing<String>> {
    use tokio::io::AsyncReadExt;
    let metadata=tokio::fs::metadata(path).await?;
    anyhow::ensure!(metadata.is_file()&&metadata.len()<=66,"invalid probe token file");
    let mut bytes=zeroize::Zeroizing::new(Vec::with_capacity(66));tokio::fs::File::open(path).await?.take(67).read_to_end(&mut bytes).await?;
    let text=std::str::from_utf8(&bytes)?.trim();anyhow::ensure!(text.len()==64&&hex::decode(text).is_ok(),"invalid probe token");
    Ok(zeroize::Zeroizing::new(text.to_owned()))
}
