use crate::state::State;
use serde_json::{Value,json};
use anyhow::{Context,ensure};

async fn request(state:&State,issuer:&Value,method:reqwest::Method,suffix:&str,body:Option<Value>)->anyhow::Result<Value>{
    let zone=issuer["zone_id"].as_str().context("zone missing")?;
    ensure!(zone.len()==32&&hex::decode(zone).is_ok(),"invalid zone");
    let mut request=state.client.request(method,format!("https://api.cloudflare.com/client/v4/zones/{zone}/dns_records{suffix}")).bearer_auth(issuer["token"].as_str().context("token missing")?);
    if let Some(body)=body{request=request.json(&body);}
    let mut response=request.send().await?.error_for_status()?;let mut bytes=Vec::new();
    while let Some(chunk)=response.chunk().await?{ensure!(bytes.len()+chunk.len()<=262144,"DNS response exceeds limit");bytes.extend(chunk);}
    let result:Value=serde_json::from_slice(&bytes)?;ensure!(result["success"]==true,"DNS operation rejected");Ok(result["result"].clone())
}

// Reconcile by exact name and value before creating after an ambiguous provider response.
pub(super) async fn present(state:&State,issuer:&Value,name:&str,value:&str)->anyhow::Result<String>{
    let query=url::form_urlencoded::Serializer::new(String::new()).append_pair("type","TXT").append_pair("name",name).append_pair("per_page","100").finish();
    let records=request(state,issuer,reqwest::Method::GET,&format!("?{query}"),None).await?;
    let records=records.as_array().context("invalid DNS list")?;ensure!(records.len()<100,"ambiguous DNS list");
    if let Some(record)=records.iter().find(|r|r["name"]==name&&matches_value(&r["content"],value)){return Ok(record["id"].as_str().context("record ID missing")?.into());}
    let record=request(state,issuer,reqwest::Method::POST,"",Some(json!({"type":"TXT","name":name,"content":value,"ttl":60}))).await?;
    Ok(record["id"].as_str().context("record ID missing")?.into())
}
pub(super) async fn cleanup(state:&State,issuer:&Value,name:&str,value:&str)->anyhow::Result<()> {
    let query=url::form_urlencoded::Serializer::new(String::new()).append_pair("type","TXT").append_pair("name",name).append_pair("per_page","100").finish();
    let records=request(state,issuer,reqwest::Method::GET,&format!("?{query}"),None).await?;
    let records=records.as_array().context("invalid DNS list")?;ensure!(records.len()<100,"ambiguous DNS list");
    for record in records {if record["name"]==name&&matches_value(&record["content"],value) {let id=record["id"].as_str().context("record ID missing")?;ensure!(id.len()==32&&hex::decode(id).is_ok(),"invalid record ID");request(state,issuer,reqwest::Method::DELETE,&format!("/{id}"),None).await?;}}
    Ok(())
}

// Cloudflare may return TXT content with its DNS presentation quotes.
fn matches_value(content:&Value,expected:&str)->bool {
    content.as_str().is_some_and(|value|value==expected||value.strip_prefix('"').and_then(|v|v.strip_suffix('"'))==Some(expected))
}
