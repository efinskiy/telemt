//! TCP and TLS reachability on SSH's port, independent of the proxy protocol.
use serde_json::{Value,json};
use std::{net::{IpAddr,SocketAddr},sync::Arc,time::Duration};
use tokio::net::TcpStream;
use tokio_rustls::{TlsConnector,rustls::{self,pki_types::ServerName}};

/// The operator-selected reachability port, fixed to prevent arbitrary port probing.
pub(crate) const PORT:u16=22;
// Matches the controller's result deadline and latency bound.
const DEADLINE:Duration=Duration::from_secs(5);

/// Connect directly to the assigned IP; SNI never selects the destination address.
pub(crate) async fn probe(ip:&str,sni:&str)->Value {
    let Ok(ip)=ip.parse::<IpAddr>()else{return json!({"port":PORT,"reachable":false,"stage":"target","phase":"tcp","latency_ms":0});};
    check(SocketAddr::new(ip,PORT),sni,DEADLINE).await
}

async fn check(address:SocketAddr,sni:&str,deadline:Duration)->Value {
    let started=tokio::time::Instant::now();let mut phase="tcp";
    let result=tokio::time::timeout(deadline,async {
        let stream=TcpStream::connect(address).await.map_err(|error|match error.kind(){std::io::ErrorKind::ConnectionRefused=>"refused",std::io::ErrorKind::TimedOut=>"timeout",_=>"tcp"})?;
        phase="tls";
        let name=ServerName::try_from(sni.to_owned()).map_err(|_|"target")?;
        let config=rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions().map_err(|_|"tls")?
            .with_root_certificates(rustls::RootCertStore::empty()).with_no_client_auth();
        match TlsConnector::from(Arc::new(config)).connect(name,stream).await {
            Ok(_)=>Ok("ok"),
            Err(error) if non_tls_response(&error)=>Ok("wrong_version_number"),
            Err(error) if error.kind()==std::io::ErrorKind::TimedOut=>Err("timeout"),
            Err(_)=>Err("tls"),
        }
    }).await.unwrap_or(Err("timeout"));
    let (reachable,stage)=match result{Ok(stage)=>(true,stage),Err(stage)=>(false,stage)};
    json!({"port":PORT,"reachable":reachable,"stage":stage,"phase":phase,"latency_ms":started.elapsed().as_millis().min(DEADLINE.as_millis()) as u64})
}

// Rustls represents OpenSSL's WRONG_VERSION_NUMBER for plaintext SSH as a record parse error.
// Other TLS/certificate errors do not prove the expected SSH response and remain unknown.
fn non_tls_response(error:&std::io::Error)->bool {
    matches!(error.get_ref().and_then(|error|error.downcast_ref::<rustls::Error>()),
        Some(rustls::Error::InvalidMessage(rustls::InvalidMessage::InvalidContentType|rustls::InvalidMessage::UnknownProtocolVersion)))
}

#[cfg(test)]mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    #[tokio::test]async fn ssh_response_is_alive_but_silent_tls_times_out(){
        for respond in [true,false] {
            let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
            let server=async {let (mut stream,_)=listener.accept().await.unwrap();let mut hello=[0u8;2048];let size=stream.read(&mut hello).await.unwrap();assert!(size>5);assert_eq!(hello[0],22);
                if respond{stream.write_all(b"SSH-2.0-OpenSSH_9.6\r\n").await.unwrap();}
                let mut sink=[0u8;2048];while stream.read(&mut sink).await.is_ok_and(|size|size>0){}
            };
            let (result,())=tokio::join!(check(address,"example.org",Duration::from_millis(100)),server);
            assert_eq!(result["reachable"],respond);assert_eq!(result["phase"],"tls");
            assert_eq!(result["stage"],if respond{"wrong_version_number"}else{"timeout"});
        }
        assert!(!non_tls_response(&std::io::Error::new(std::io::ErrorKind::ConnectionReset,"reset")));
    }
}
