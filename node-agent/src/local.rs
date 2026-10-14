//! Loopback API transport never accepts remote URLs or arbitrary request headers.
use anyhow::{ensure,Context};
use sha2::{Digest,Sha256};
use serde_json::{Value,json};
use telemt_managed::{Bundle,Command,Receipt};
const RESPONSE_LIMIT:usize=512*1024;
pub(crate) const READS:[&str;9]=["/v1/health/ready","/v1/system/info","/v1/stats/summary","/v1/fleet/traffic","/v1/runtime/shaping?limit=1","/v1/fleet/compatibility","/v1/runtime/system-resources","/v1/stats/dcs","/v1/runtime/gates"];

pub(crate) async fn response(mut response:reqwest::Response,limit:usize)->anyhow::Result<Value>{
    ensure!(response.content_length().is_none_or(|n|n<=limit as u64),"response exceeds limit");let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await? {ensure!(bytes.len()+chunk.len()<=limit,"response exceeds limit");bytes.extend(chunk);}
    Ok(serde_json::from_slice(&bytes)?)
}
pub(crate) async fn request(client:&reqwest::Client,bundle:&Bundle,command:&Command)->anyhow::Result<Receipt>{
    ensure!(telemt_managed::allowed_command(&command.method,&command.path),"command rejected");
    let method=reqwest::Method::from_bytes(command.method.as_bytes())?;
    let mut request=client.request(method,format!("http://{}{}",telemt_managed::API_ADDR,command.path)).header("authorization",&bundle.api_token);
    if let Some(revision)=&command.revision{request=request.header("if-match",revision);}
    if let Some(body)=&command.body{request=request.json(body);}
    let reply=request.send().await?;let status=reply.status().as_u16();
    Ok(Receipt{id:command.id.clone(),status,response:response(reply,RESPONSE_LIMIT).await?})
}
pub(crate) async fn reads(client:&reqwest::Client,bundle:&Bundle)->Value{
    let mut result=json!({});let mut tasks=tokio::task::JoinSet::new();
    for path in READS {let client=client.clone();let bundle=bundle.clone();tasks.spawn(async move{let command=Command{id:String::new(),method:"GET".into(),path:path.into(),body:None,revision:None};(path,request(&client,&bundle,&command).await)});}
    let mut size=0;
    while let Some(joined)=tasks.join_next().await {let Ok((path,Ok(receipt)))=joined else {continue;};let value=json!({"status":receipt.status,"response":receipt.response});size+=value.to_string().len();if size<256*1024{result[path]=value;}}
    result
}
pub(crate) async fn ready(client:&reqwest::Client,bundle:&Bundle)->anyhow::Result<()> {
    let command=Command{id:String::new(),method:"GET".into(),path:"/v1/health/ready".into(),body:None,revision:None};
    let result=request(client,bundle,&command).await?;ensure!(result.status==200,"telemt not ready");Ok(())
}
pub(crate) async fn public_tls(bundle:&Bundle)->anyhow::Result<()> {
    ensure!(!bundle.public_ips.is_empty()&&bundle.public_ips.len()<=8&&bundle.public_ips.contains(&bundle.public_ip),"invalid endpoint set");
    for ip in &bundle.public_ips { public_endpoint(bundle,ip).await?; }
    Ok(())
}
async fn public_endpoint(bundle:&Bundle,ip:&str)->anyhow::Result<()> {
    let ip=ip.parse().context("invalid public IP")?;
    let mut builder=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(std::time::Duration::from_secs(5)).tls_info(true).resolve(&bundle.domain,std::net::SocketAddr::new(ip,443));
    if let Ok(path)=std::env::var("MANAGED_CA_FILE"){builder=builder.add_root_certificate(reqwest::Certificate::from_pem(&crate::storage::read(std::path::Path::new(&path),128*1024).await?)?);}
    let reply=builder.build()?.get(format!("https://{}/",bundle.domain)).send().await?;
    let expected=telemt_managed::certificate::validate(&bundle.fullchain,&bundle.private_key,&bundle.domain)?;
    let served=reply.extensions().get::<reqwest::tls::TlsInfo>().and_then(|info|info.peer_certificate()).context("peer certificate missing")?;
    ensure!(hex::encode(Sha256::digest(served))==expected.fingerprint,"certificate reload pending");
    ensure!(reply.status().is_success(),"HTTPS endpoint unavailable");Ok(())
}
