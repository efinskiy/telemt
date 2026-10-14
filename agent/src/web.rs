//! Verify the public WEB carrier with the same nonce-bound MTProto exchange as native probes.
use crate::{Job,protocol};
use anyhow::{ensure,Context};
use base64::{Engine,engine::general_purpose::URL_SAFE_NO_PAD};
use ctr::cipher::StreamCipher;
use serde_json::Value;
use std::time::Duration;
const LIMIT:usize=65536;

async fn body(mut response:reqwest::Response,limit:usize)->anyhow::Result<Vec<u8>>{
    let mut bytes=Vec::new();while let Some(chunk)=response.chunk().await?{ensure!(bytes.len()+chunk.len()<=limit,"WEB response too large");bytes.extend(chunk);}Ok(bytes)
}
fn frame(kind:u8,payload:&[u8])->Vec<u8>{let mut out=vec![kind,0,0,1];out.extend((payload.len() as u32).to_be_bytes());out.extend(payload);out}
pub(crate) async fn probe(job:&Job)->anyhow::Result<()> {
    let ip=job.ip.parse()?;let secret=hex::decode(&job.secret)?;ensure!(secret.len()==16,"invalid probe key");
    let base=job.web_base_path.as_deref().unwrap_or("");
    ensure!(base.len()<=128 && (base.is_empty()||base.split('/').all(|s|!s.is_empty()&&s.as_bytes()[0].is_ascii_alphanumeric()&&s.bytes().all(|b|b.is_ascii_alphanumeric()||b"_-".contains(&b)))),"invalid WEB base path");
    let mut builder=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).resolve(&job.sni,std::net::SocketAddr::new(ip,443)).timeout(Duration::from_secs(3));
    if let Ok(path)=std::env::var("PROBE_CA_FILE") {use tokio::io::AsyncReadExt;let file=tokio::fs::File::open(path).await?;let mut pem=Vec::new();file.take(131073).read_to_end(&mut pem).await?;ensure!(pem.len()<=131072,"CA file too large");builder=builder.add_root_certificate(reqwest::Certificate::from_pem(&pem)?);}
    let client=builder.build()?;
    let root=if base.is_empty(){format!("https://{}/",job.sni)}else{format!("https://{}/{base}/",job.sni)};
    let mut key=vec![0xdd];key.extend(&secret);
    let message=if base.is_empty(){format!("tdesktop-web-proxy-bridge-v1\n{}",job.sni)}else{format!("tdesktop-web-proxy-bridge-v2\n{}\n{base}",job.sni)};
    let capability=URL_SAFE_NO_PAD.encode(protocol::hmac(&key,message.as_bytes())?);
    let recovery=client.get(format!("{root}?bridge={capability}")).header("accept","application/vnd.telemt.web-recovery+json").send().await?.error_for_status()?;
    ensure!(recovery.headers().get("content-type").and_then(|v|v.to_str().ok()).is_some_and(|v|v.starts_with("application/vnd.telemt.web-recovery+json")),"invalid recovery media type");
    let recovery:Value=serde_json::from_slice(&body(recovery,1024).await?)?;
    let bootstrap=recovery["bootstrap"].as_str().context("bootstrap missing")?;ensure!(recovery["v"]==1 && canonical_token(bootstrap),"invalid bootstrap");
    let endpoint=format!("{root}api/v1");
    let mut session=client.post(format!("{endpoint}/session")).bearer_auth(bootstrap).header("content-type","application/octet-stream").body(vec![0x10,0,0,0,0,0,0,1,1]);
    if recovery["negotiation"]["enabled"]==true{session=session.header("X-Carrier-Capabilities","https").header("X-Carrier-Attempt","1");}
    let response=session.send().await?;
    ensure!(response.status()==200,"session rejected");let token=response.headers().get("x-session-token").and_then(|v|v.to_str().ok()).context("session token missing")?.to_owned();ensure!(canonical_token(&token),"invalid session token");
    let carrier=response.headers().get("x-carrier-mode").and_then(|v|v.to_str().ok()).context("carrier missing")?;ensure!(["https","https-lanes"].contains(&carrier),"unsupported carrier");let lanes=carrier=="https-lanes";
    let result=async {ensure!(body(response,1024).await?==[0x11,0,0,0,0,0,0,0],"session framing invalid");exchange(&client,&endpoint,&token,&secret,job.dc,lanes).await}.await;
    let _=tokio::time::timeout(Duration::from_millis(200),client.delete(format!("{endpoint}/session")).bearer_auth(&token).send()).await;
    result
}
async fn exchange(client:&reqwest::Client,endpoint:&str,token:&str,secret:&[u8],dc:i16,lanes:bool)->anyhow::Result<()> {
    let (header,mut tx,mut rx)=protocol::obfuscation(secret,dc)?;let nonce=protocol::random::<16>();let payload=protocol::request(nonce);
    let mut encrypted=(payload.len() as u32).to_le_bytes().to_vec();encrypted.extend(payload);tx.apply_keystream(&mut encrypted);
    let mut data=header.to_vec();data.extend(encrypted);let mut batch=frame(1,&[]);batch.extend(frame(2,&data));
    let mut up=client.post(format!("{endpoint}/up")).bearer_auth(token).header("content-type","application/octet-stream").header("x-up-seq","1").body(batch);
    if lanes{up=up.header("x-lane-id","1");}
    let response=up.send().await?;ensure!(response.status()==204&&response.headers().get("x-up-ack").is_some_and(|v|v=="1"),"up rejected");
    let mut cursor=0u64;let mut plaintext=Vec::new();
    for _ in 0..16 {
        let mut down=client.post(format!("{endpoint}/down")).bearer_auth(token).header("x-down-cursor",cursor);
        if lanes{down=down.header("x-lane-id","1");}
        let response=down.send().await?;ensure!(response.status()==200||response.status()==204,"down rejected");
        let status=response.status().as_u16();
        let cursor_header=response.headers().get("x-down-cursor").and_then(|v|v.to_str().ok()).context("cursor missing")?.to_owned();
        let next=cursor_header.parse::<u64>()?;ensure!(next.to_string()==cursor_header,"noncanonical cursor");
        let wire=body(response,LIMIT).await?;
        ensure!((status==200&&!wire.is_empty()&&cursor.checked_add(1)==Some(next))||(status==204&&wire.is_empty()&&next==cursor),"invalid down cursor");
        let data=decode_frames(&wire)?;
        for mut bytes in data {rx.apply_keystream(&mut bytes);ensure!(plaintext.len()+bytes.len()<=LIMIT,"MTProto response too large");plaintext.extend(bytes);}
        cursor=next;
        if plaintext.len()>=4{let size=u32::from_le_bytes(plaintext[..4].try_into()?) as usize;ensure!((40..LIMIT-4).contains(&size),"invalid MTProto size");if plaintext.len()>=size+4{return protocol::validate_response(&plaintext[4..size+4],&nonce);}}
    }anyhow::bail!("WEB response exhausted")
}

fn canonical_token(value:&str)->bool {value.len()==43&&URL_SAFE_NO_PAD.decode(value).is_ok_and(|bytes|bytes.len()==32&&URL_SAFE_NO_PAD.encode(bytes)==value)}
fn decode_frames(wire:&[u8])->anyhow::Result<Vec<Vec<u8>>>{
    let mut offset=0;let mut data=Vec::new();
    while offset<wire.len(){
        ensure!(wire.len()-offset>=8,"truncated frame header");
        let kind=wire[offset];let stream=u32::from_be_bytes([0,wire[offset+1],wire[offset+2],wire[offset+3]]);let size=u32::from_be_bytes(wire[offset+4..offset+8].try_into()?) as usize;
        ensure!(size<=LIMIT-8&&wire.len()-offset>=8+size,"truncated frame payload");let payload=&wire[offset+8..offset+8+size];
        match kind {
            2=>{ensure!(stream==1&&!payload.is_empty(),"unexpected data stream");data.push(payload.to_vec());}
            4=>ensure!(stream==1&&size==4&&u32::from_be_bytes(payload.try_into()?)>0,"invalid credit"),
            _=>anyhow::bail!("unexpected or terminal WEB frame"),
        }offset+=8+size;
    }Ok(data)
}
#[cfg(test)] mod tests {
    use super::*;
    #[test] fn malformed_frames_never_advance_cipher(){assert!(decode_frames(&[0x1f,0,0,0,0,0,0,0]).is_err());assert!(decode_frames(&[2,0,0,1,0,0,0,2,1]).is_err());assert!(decode_frames(&frame(2,&[1,2])).is_ok());assert!(decode_frames(&frame(4,&[0,0,0,0])).is_err());}
    #[tokio::test]
    #[ignore="requires an isolated managed-node Compose fixture and TELEMT_PROBE_TEST_JOB"]
    async fn live_managed_web_round_trip(){
        let path=std::env::var("TELEMT_PROBE_TEST_JOB").unwrap();let bytes=std::fs::read(path).unwrap();assert!(bytes.len()<8192);let job:Job=serde_json::from_slice(&bytes).unwrap();
        tokio::time::timeout(Duration::from_secs(5),probe(&job)).await.unwrap().unwrap();
        assert_eq!(tokio::time::timeout(Duration::from_secs(5),protocol::probe(&job)).await.unwrap(),Ok(()));
    }
    #[test] fn token_is_canonical(){let token=URL_SAFE_NO_PAD.encode([255;32]);assert!(canonical_token(&token));assert!(!canonical_token(&format!("{token}=")));}
}
