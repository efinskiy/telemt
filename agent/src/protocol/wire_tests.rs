use super::*;

#[tokio::test]
async fn socket_probe_authenticates_flight_and_fragmented_res_pq() {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let secret=[7;16];
    let server=tokio::spawn(async move {
        let (mut stream,_)=listener.accept().await.unwrap();let client=record(&mut stream).await.unwrap();let digest=&client[11..43];
        let mut hello=vec![22,3,3,0,44,2,0,0,40,3,3];hello.extend([0;32]);hello.extend([0;6]);
        let ccs=[20,3,3,0,1,1];let tail=[23,3,3,0,4,9,8,7,6];let mut flight=hello.clone();flight.extend(ccs);flight.extend(tail);let mut input=digest.to_vec();input.extend(flight);hello[11..43].copy_from_slice(&hmac(&secret,&input).unwrap());
        stream.write_all(&hello).await.unwrap();stream.write_all(&ccs).await.unwrap();stream.write_all(&tail).await.unwrap();
        let nonce=record(&mut stream).await.unwrap();let nonce=&nonce[5..];assert_eq!(nonce.len(),64);
        let mut key=nonce[8..40].to_vec();key.extend(secret);let key:[u8;32]=Sha256::digest(key).into();let mut rx=Cipher::new((&key).into(),nonce[40..56].into());let mut header=nonce.to_vec();rx.apply_keystream(&mut header);assert_eq!(&header[56..60],&PADDED_INTERMEDIATE.to_le_bytes());
        let mut query=record(&mut stream).await.unwrap();rx.apply_keystream(&mut query[5..]);let query=&query[5..];assert_eq!(u32::from_le_bytes(query[24..28].try_into().unwrap()),REQ_PQ_MULTI);
        let mut frame=vec![0;76];frame[8]=1;frame[16..20].copy_from_slice(&56u32.to_le_bytes());frame[20..24].copy_from_slice(&RES_PQ.to_le_bytes());frame[24..40].copy_from_slice(&query[28..44]);frame[40..56].fill(3);frame[56]=1;frame[57]=7;frame[60..64].copy_from_slice(&0x1cb5c415u32.to_le_bytes());frame[64..68].copy_from_slice(&1u32.to_le_bytes());frame[68..76].fill(9);
        let reverse:Vec<_>=nonce[8..56].iter().rev().copied().collect();let mut key=reverse[..32].to_vec();key.extend(secret);let key:[u8;32]=Sha256::digest(key).into();let mut tx=Cipher::new((&key).into(),reverse[32..48].into());let mut response=(frame.len() as u32).to_le_bytes().to_vec();response.extend(frame);tx.apply_keystream(&mut response);
        write_record(&mut stream,&response[..13]).await.unwrap();write_record(&mut stream,&response[13..]).await.unwrap();
    });
    let job=Job{web_base_path:None,id:"test".into(),node_id:"test".into(),round:1,family:4,ip:"127.0.0.1".into(),port,sni:"example.org".into(),dc:2,secret:hex::encode(secret),expires_at:chrono::Utc::now()+chrono::Duration::seconds(5)};
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5),probe(&job)).await.unwrap(),Ok(()));server.await.unwrap();
}

#[tokio::test]
#[ignore="requires an isolated running Telemt node on loopback"]
async fn isolated_telemt_round_trip() {
    let ip=std::env::var("TELEMT_PROBE_TEST_IP").unwrap_or_else(|_|"127.0.0.1".into());
    assert!(ip.parse::<std::net::IpAddr>().unwrap().is_loopback());
    let job=Job{web_base_path:None,id:"integration".into(),node_id:"integration".into(),round:1,family:4,ip,port:std::env::var("TELEMT_PROBE_TEST_PORT").unwrap().parse().unwrap(),sni:std::env::var("TELEMT_PROBE_TEST_SNI").unwrap(),dc:2,secret:std::env::var("TELEMT_PROBE_TEST_SECRET").unwrap(),expires_at:chrono::Utc::now()+chrono::Duration::seconds(5)};
    assert_eq!(tokio::time::timeout(std::time::Duration::from_secs(5),probe(&job)).await.unwrap(),Ok(()));
}
