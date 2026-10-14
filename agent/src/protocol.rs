//! Bounded FakeTLS and obfuscated padded-intermediate client for a real MTProto round trip.
use aes::Aes256;
use ctr::{Ctr128BE,cipher::{KeyIvInit,StreamCipher}};
use hmac::{Hmac,Mac};
use rand::RngCore;
use sha2::{Digest,Sha256};
use subtle::ConstantTimeEq;
use tokio::{net::TcpStream,io::{AsyncReadExt,AsyncWriteExt}};
use crate::Job;

const HELLO_TARGET:usize=600;
const RECORD_LIMIT:usize=16_640;
const RESPONSE_LIMIT:usize=65_536;
const RESPONSE_RECORDS:usize=32;
const HANDSHAKE_OFFSET:usize=11;
const REQ_PQ_MULTI:u32=0xbe7e8ef1;
const RES_PQ:u32=0x05162463;
const PADDED_INTERMEDIATE:u32=0xdddddddd;
pub(crate) type Cipher=Ctr128BE<Aes256>;

pub(crate) fn random<const N:usize>()->[u8;N]{let mut bytes=[0;N];rand::rng().fill_bytes(&mut bytes);bytes}
pub(crate) fn hmac(secret:&[u8],bytes:&[u8])->anyhow::Result<[u8;32]>{let mut mac=<Hmac<Sha256> as Mac>::new_from_slice(secret)?;mac.update(bytes);Ok(mac.finalize().into_bytes().into())}
fn extension(output:&mut Vec<u8>,kind:u16,bytes:&[u8])->anyhow::Result<()>{output.extend(kind.to_be_bytes());output.extend(u16::try_from(bytes.len())?.to_be_bytes());output.extend(bytes);Ok(())}

pub(crate) fn hello(secret:&[u8],sni:&str)->anyhow::Result<(Vec<u8>,[u8;32])>{
    anyhow::ensure!(secret.len()==16&&!sni.is_empty()&&sni.len()<=253&&sni.bytes().all(|c|c.is_ascii_alphanumeric()||b".-".contains(&c)),"invalid probe credential or SNI");
    let mut extensions=Vec::new();let mut name=Vec::new();name.extend(u16::try_from(sni.len()+3)?.to_be_bytes());name.push(0);name.extend(u16::try_from(sni.len())?.to_be_bytes());name.extend(sni.as_bytes());extension(&mut extensions,0,&name)?;
    extension(&mut extensions,0x002b,&[2,3,4])?;extension(&mut extensions,0x000a,&[0,2,0,29])?;
    extension(&mut extensions,0x000d,&[0,4,4,3,8,4])?;
    let mut key=vec![0,36,0,29,0,32];key.extend(x25519_dalek::x25519(random::<32>(),x25519_dalek::X25519_BASEPOINT_BYTES));extension(&mut extensions,0x0033,&key)?;
    extension(&mut extensions,0x0010,b"\x00\x0c\x02h2\x08http/1.1")?;
    let mut message=vec![3,3];message.extend([0;32]);message.push(32);message.extend(random::<32>());message.extend([0,6,0x13,1,0x13,2,0x13,3,1,0]);
    let padding=HELLO_TARGET.checked_sub(5+4+message.len()+2+extensions.len()+4).ok_or_else(||anyhow::anyhow!("ClientHello too large"))?;extension(&mut extensions,0x0015,&vec![0;padding])?;
    message.extend(u16::try_from(extensions.len())?.to_be_bytes());message.extend(extensions);
    let mut handshake=vec![1];let length=u32::try_from(message.len())?.to_be_bytes();handshake.extend(&length[1..]);handshake.extend(message);
    let mut record=vec![22,3,1];record.extend(u16::try_from(handshake.len())?.to_be_bytes());record.extend(handshake);
    let mut digest=hmac(secret,&record)?;let timestamp=(chrono::Utc::now().timestamp() as u32).to_le_bytes();for i in 0..4{digest[28+i]^=timestamp[i];}
    record[HANDSHAKE_OFFSET..HANDSHAKE_OFFSET+32].copy_from_slice(&digest);Ok((record,digest))
}
async fn record(stream:&mut TcpStream)->anyhow::Result<Vec<u8>> {
    let mut header=[0;5];stream.read_exact(&mut header).await?;let size=u16::from_be_bytes([header[3],header[4]]) as usize;
    anyhow::ensure!(size>0&&size<=RECORD_LIMIT&&header[1]==3&&[1,3].contains(&header[2]),"invalid TLS record");
    let mut record=header.to_vec();record.resize(5+size,0);stream.read_exact(&mut record[5..]).await?;Ok(record)
}
async fn authenticate(stream:&mut TcpStream,secret:&[u8],sni:&str)->anyhow::Result<()> {
    let (hello,digest)=hello(secret,sni)?;stream.write_all(&hello).await?;
    let mut response=record(stream).await?;anyhow::ensure!(response[0]==22&&response.len()>HANDSHAKE_OFFSET+32&&response[5]==2,"invalid ServerHello");
    let received:[u8;32]=response[HANDSHAKE_OFFSET..HANDSHAKE_OFFSET+32].try_into()?;response[HANDSHAKE_OFFSET..HANDSHAKE_OFFSET+32].fill(0);
    for _ in 0..6 {let next=record(stream).await?;anyhow::ensure!([20,23].contains(&next[0])&&response.len()+next.len()<=RESPONSE_LIMIT,"invalid server flight");let application=next[0]==23;response.extend(next);
        if application{let mut input=digest.to_vec();input.extend(&response);if bool::from(hmac(secret,&input)?.ct_eq(&received)){return Ok(());}}
    }anyhow::bail!("FakeTLS response authentication failed")
}
pub(crate) fn obfuscation(secret:&[u8],dc:i16)->anyhow::Result<([u8;64],Cipher,Cipher)> {
    let mut nonce=random::<64>();let mut valid=false;
    for _ in 0..32 {if ![0xef,0x16,0x17].contains(&nonce[0])&&!nonce[..4].starts_with(b"GET ")&&!nonce[..4].starts_with(b"POST")&&!nonce[..4].starts_with(b"HEAD")&&!nonce[..4].starts_with(b"OPTI")&&nonce[4..8]!=[0;4]{valid=true;break;}nonce=random();}
    anyhow::ensure!(valid,"nonce generation exhausted");
    nonce[56..60].copy_from_slice(&PADDED_INTERMEDIATE.to_le_bytes());nonce[60..62].copy_from_slice(&dc.to_le_bytes());
    let mut key=nonce[8..40].to_vec();key.extend(secret);let tx_key: [u8;32]=Sha256::digest(key).into();
    let reverse:Vec<_>=nonce[8..56].iter().rev().copied().collect();let mut key=reverse[..32].to_vec();key.extend(secret);let rx_key:[u8;32]=Sha256::digest(key).into();
    let mut tx=Cipher::new((&tx_key).into(),nonce[40..56].into());let rx=Cipher::new((&rx_key).into(),reverse[32..48].into());
    let mut encrypted=nonce;tx.apply_keystream(&mut encrypted);nonce[56..64].copy_from_slice(&encrypted[56..64]);Ok((nonce,tx,rx))
}
async fn write_record(stream:&mut TcpStream,bytes:&[u8])->anyhow::Result<()> {anyhow::ensure!(bytes.len()<=RECORD_LIMIT,"request too large");let mut header=vec![23,3,3];header.extend(u16::try_from(bytes.len())?.to_be_bytes());stream.write_all(&header).await?;stream.write_all(bytes).await?;Ok(())}
pub(crate) fn request(nonce:[u8;16])->Vec<u8>{let mut body=vec![0;8];let id=((chrono::Utc::now().timestamp() as u64)<<32)|4;body.extend(id.to_le_bytes());body.extend(20u32.to_le_bytes());body.extend(REQ_PQ_MULTI.to_le_bytes());body.extend(nonce);body}
pub(crate) fn validate_response(frame:&[u8],nonce:&[u8;16])->anyhow::Result<()> {
    anyhow::ensure!(frame.len()>=76&&frame[..8]==[0;8]&&frame[8]&1==1,"invalid MTProto envelope");
    let length=u32::from_le_bytes(frame[16..20].try_into()?) as usize;
    anyhow::ensure!(length>=56&&length.is_multiple_of(4)&&length<=frame.len()-20&&u32::from_le_bytes(frame[20..24].try_into()?)==RES_PQ&&bool::from(frame[24..40].ct_eq(nonce)),"invalid resPQ or nonce");
    let body=&frame[20..20+length];let pq=usize::from(body[36]);
    anyhow::ensure!((1..=8).contains(&pq),"invalid resPQ factor length");
    let vector=(37+pq+3)&!3;
    anyhow::ensure!(vector+8<=body.len()&&u32::from_le_bytes(body[vector..vector+4].try_into()?)==0x1cb5c415,"missing fingerprint vector");
    let count=u32::from_le_bytes(body[vector+4..vector+8].try_into()?) as usize;
    anyhow::ensure!((1..=16).contains(&count)&&vector+8+count*8==body.len(),"invalid fingerprint count");Ok(())
}

pub(crate) async fn probe(job:&Job)->std::result::Result<(),&'static str> {
    let ip:std::net::IpAddr=job.ip.parse().map_err(|_|"tcp")?;if ip.is_ipv4()!=(job.family==4)||ip.is_unspecified()||ip.is_multicast(){return Err("tcp");}
    let secret=zeroize::Zeroizing::new(hex::decode(&job.secret).map_err(|_|"fake_tls")?);
    let mut stream=TcpStream::connect((ip,job.port)).await.map_err(|_|"tcp")?;
    authenticate(&mut stream,&secret,&job.sni).await.map_err(|_|"fake_tls")?;
    let (nonce,mut tx,mut rx)=obfuscation(&secret,job.dc).map_err(|_|"mtproto")?;
    write_record(&mut stream,&nonce).await.map_err(|_|"mtproto")?;
    let challenge=random::<16>();let mut payload=request(challenge);let mut frame=(payload.len() as u32).to_le_bytes().to_vec();frame.append(&mut payload);tx.apply_keystream(&mut frame);write_record(&mut stream,&frame).await.map_err(|_|"mtproto")?;
    let mut plaintext=Vec::new();
    for _ in 0..RESPONSE_RECORDS {let mut data=record(&mut stream).await.map_err(|_|"mtproto")?;if data[0]!=23{return Err("mtproto");}rx.apply_keystream(&mut data[5..]);if plaintext.len()+data.len()-5>RESPONSE_LIMIT{return Err("mtproto");}plaintext.extend(&data[5..]);
        if plaintext.len()>=4 {let length=u32::from_le_bytes(plaintext[..4].try_into().map_err(|_|"mtproto")?) as usize;
            if !(40..=RESPONSE_LIMIT-4).contains(&length){return Err("mtproto");}
            if plaintext.len()>=4+length{return validate_response(&plaintext[4..4+length],&challenge).map_err(|_|"mtproto");}}
    }Err("mtproto")
}
#[cfg(test)]mod tests{use super::*;#[test]fn client_hello_has_authenticated_timestamp(){let secret=[7;16];let (mut hello,digest)=hello(&secret,"example.org").unwrap();assert_eq!(hello.len(),HELLO_TARGET);hello[11..43].fill(0);let expected=hmac(&secret,&hello).unwrap();assert_eq!(digest[..28],expected[..28]);}#[test]fn wrong_nonce_never_counts_as_reachable(){let mut bytes=vec![0;76];bytes[8]=1;bytes[16..20].copy_from_slice(&56u32.to_le_bytes());bytes[20..24].copy_from_slice(&RES_PQ.to_le_bytes());bytes[56]=1;bytes[57]=7;bytes[60..64].copy_from_slice(&0x1cb5c415u32.to_le_bytes());bytes[64..68].copy_from_slice(&1u32.to_le_bytes());assert!(validate_response(&bytes,&[1;16]).is_err());assert!(validate_response(&bytes,&[0;16]).is_ok());}#[test]fn header_encryption_preserves_public_key_material(){let secret=[5;16];let (header,_,_)=obfuscation(&secret,2).unwrap();let mut prekey=header[8..40].to_vec();prekey.extend(secret);let key:[u8;32]=Sha256::digest(prekey).into();let mut cipher=Cipher::new((&key).into(),header[40..56].into());let mut decrypted=header;cipher.apply_keystream(&mut decrypted);assert_eq!(&decrypted[56..60],&PADDED_INTERMEDIATE.to_le_bytes());assert_eq!(&decrypted[60..62],&2i16.to_le_bytes());}}

#[cfg(test)]
mod wire_tests;
