//! Measures real loopback socket throughput with the production disabled wrapper.
use crate::traffic_limiter::{TrafficLimiter,AdaptiveIo};
use tokio::io::{AsyncWriteExt,AsyncReadExt};
async fn transfer(shaped:bool)->f64 {
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let reader=tokio::spawn(async move{let (mut stream,_)=listener.accept().await.unwrap();let mut buffer=vec![0;256*1024];let mut received=0u64;loop{let n=stream.read(&mut buffer).await.unwrap();if n==0{break;}received+=n as u64;}received});
    let mut stream=tokio::net::TcpStream::connect(address).await.unwrap();stream.set_nodelay(true).unwrap();let payload=vec![7;64*1024];let blocks=32768;let limiter=TrafficLimiter::new();let lease=limiter.acquire_lease("benchmark","192.0.2.1".parse().unwrap()).unwrap();let started=std::time::Instant::now();
    if shaped {let mut stream=AdaptiveIo::new(stream,Some(lease));for _ in 0..blocks{stream.write_all(&payload).await.unwrap();}stream.shutdown().await.unwrap();}
    else {for _ in 0..blocks{stream.write_all(&payload).await.unwrap();}stream.shutdown().await.unwrap();}
    assert_eq!(reader.await.unwrap(),blocks*payload.len() as u64);started.elapsed().as_secs_f64()
}
#[tokio::test(flavor="multi_thread",worker_threads=4)]
#[ignore="reproducible disabled shaping loopback comparison"]
async fn disabled_loopback_throughput() {
    transfer(false).await;transfer(true).await;let mut baseline=Vec::new();let mut disabled=Vec::new();
    for round in 0..7 {if round%2==0{baseline.push(transfer(false).await);disabled.push(transfer(true).await);}else{disabled.push(transfer(true).await);baseline.push(transfer(false).await);}}
    baseline.sort_by(f64::total_cmp);disabled.sort_by(f64::total_cmp);let regression=(1.0-baseline[3]/disabled[3])*100.0;
    println!("bytes_per_run=2147483648 baseline_seconds={:.6} disabled_seconds={:.6} throughput_regression_percent={regression:.3}",baseline[3],disabled[3]);
    assert!(regression<=3.0,"loopback regression {regression:.3}%");
}
