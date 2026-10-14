//! Destination selection cannot cross the configured ingress trust or credential quota boundary.
use super::*;
use crate::web::http::request::endpoint_profile;

fn config() -> ProxyConfig {
    let mut config = runtime_config([7; 32], WebCarrier::Https);
    let runtime = Arc::get_mut(config.web.runtime.as_mut().unwrap()).unwrap();
    let vhost = Arc::get_mut(runtime.vhosts.get_mut("proxy.example.com").unwrap()).unwrap();
    let mut variant = (*vhost.profiles[0]).clone();
    variant.public_addr = "203.0.113.11:443".parse().unwrap();
    vhost
        .endpoint_profiles
        .insert(variant.public_addr, vec![Arc::new(variant)]);
    config
}

#[test]
fn endpoint_requires_one_allowlisted_header_from_trusted_peer() {
    let config = config();
    let vhost = &config.web.runtime.as_ref().unwrap().vhosts["proxy.example.com"];
    let profile = Arc::clone(&vhost.profiles[0]);
    let trusted = ["127.0.0.1/32".parse().unwrap()];
    let peer = "127.0.0.1:12345".parse().unwrap();
    let mut request = hyper::Request::builder()
        .header("x-telemt-public-address", "203.0.113.11:443")
        .body(())
        .unwrap();
    let selected = endpoint_profile(&request, peer, &trusted, vhost, Arc::clone(&profile)).unwrap();
    assert_eq!(selected.public_addr, "203.0.113.11:443".parse().unwrap());
    assert_eq!(selected.credential_id, profile.credential_id);
    assert_eq!(selected.capability, profile.capability);
    assert!(
        endpoint_profile(
            &request,
            "192.0.2.1:12345".parse().unwrap(),
            &trusted,
            vhost,
            Arc::clone(&profile)
        )
        .is_none()
    );
    request.headers_mut().append(
        "x-telemt-public-address",
        "203.0.113.11:443".parse().unwrap(),
    );
    assert!(endpoint_profile(&request, peer, &trusted, vhost, Arc::clone(&profile)).is_none());
    request.headers_mut().insert(
        "x-telemt-public-address",
        "203.0.113.99:443".parse().unwrap(),
    );
    assert!(endpoint_profile(&request, peer, &trusted, vhost, Arc::clone(&profile)).is_none());
    request.headers_mut().remove("x-telemt-public-address");
    assert!(endpoint_profile(&request, peer, &trusted, vhost, profile).is_none());
}

#[tokio::test]
async fn endpoint_bootstrap_requires_matching_endpoint_after_reload() {
    for retained in [true, false] {
        let generation = test_runtime_generation(1, config());
        let active = Arc::new(ArcSwap::from(Arc::clone(&generation)));
        let runtime = WebProcessRuntime::start(Arc::clone(&active));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode([7u8; 32]);
        let root=format!("GET /?bridge={encoded} HTTP/1.1\r\nHost: proxy.example.com\r\nX-Forwarded-For: 192.0.2.10\r\nX-Telemt-Public-Address: 203.0.113.11:443\r\nConnection: close\r\n\r\n").into_bytes();
        let response = request(&listener, &runtime, root).await;
        let (_, body) = split_response(&response);
        let body = std::str::from_utf8(body).unwrap();
        let bootstrap = body
            .split_once("bootstrap=\"")
            .unwrap()
            .1
            .split('"')
            .next()
            .unwrap();
        let replacement = test_runtime_generation(
            2,
            if retained {
                config()
            } else {
                runtime_config([7; 32], WebCarrier::Https)
            },
        );
        active.store(Arc::clone(&replacement));
        let hello = frame::encode(FrameType::Hello, 0, &[1]);
        let mut create=format!("POST /api/v1/session HTTP/1.1\r\nHost: proxy.example.com\r\nX-Forwarded-For: 192.0.2.10\r\nAuthorization: Bearer {bootstrap}\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",hello.len()).into_bytes();
        create.extend_from_slice(&hello);
        assert_eq!(
            request(&listener, &runtime, create)
                .await
                .starts_with(b"HTTP/1.1 200"),
            retained
        );
        runtime.shutdown().await;
        generation.stop_sessions().await;
        generation.stop_background_tasks().await;
        replacement.stop_sessions().await;
        replacement.stop_background_tasks().await;
    }
}
