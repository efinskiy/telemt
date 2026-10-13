//! Independent control plane; no dependency on telemt's payload relay or public camouflage listeners.
mod auth;
mod crypto;
mod geoip;
mod connections_map;
mod dns;
mod error;
mod monitor;
mod operations;
mod probes;
mod routes;
mod rotation;
mod state;
use axum::{Router,body::Body,extract::{State as ExtractState,DefaultBodyLimit},http::{Request,StatusCode,header},middleware::{self,Next},response::Response,routing::{get,post,any}};
use state::State;
use std::{sync::Arc,time::Duration};
use sqlx::postgres::PgPoolOptions;
use tokio_util::sync::CancellationToken;

async fn security(req:Request<Body>,next:Next)->Response {
    let mut response=match tokio::time::timeout(Duration::from_secs(15), next.run(req)).await {
        Ok(response) => response,
        Err(_) => Response::builder().status(StatusCode::REQUEST_TIMEOUT).header("content-type","application/json").body(Body::from("{\"ok\":false,\"error\":{\"code\":\"request_timeout\"}}" )).unwrap_or_default(),
    };
    let headers=response.headers_mut();
    for (key,value) in [("cache-control","no-store"),("x-content-type-options","nosniff"),("referrer-policy","no-referrer"),("x-frame-options","DENY"),("content-security-policy","default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'")] {headers.insert(axum::http::HeaderName::from_static(key),axum::http::HeaderValue::from_static(value));}response
}
async fn assets(ExtractState(state):ExtractState<State>,req:Request<Body>)->Response {
    if req.method()!=axum::http::Method::GET{return Response::builder().status(StatusCode::METHOD_NOT_ALLOWED).body(Body::empty()).unwrap_or_default();}
    let path=req.uri().path();
    if path.starts_with("/api/")||path.starts_with("/agent/"){return Response::builder().status(StatusCode::NOT_FOUND).body(Body::empty()).unwrap_or_default();}
    let file=if path.starts_with("/assets/") && path[8..].bytes().all(|c|c.is_ascii_alphanumeric()||b"_.-".contains(&c)){state.assets.join(path.trim_start_matches('/'))}else{state.assets.join("index.html")};
    match bounded_file(&file,4*1024*1024).await {
        Ok(bytes) if bytes.len()<=4*1024*1024=>{
            let content=match file.extension().and_then(|s|s.to_str()){Some("js")=>"application/javascript",Some("css")=>"text/css",Some("svg")=>"image/svg+xml",_=>"text/html; charset=utf-8"};
            Response::builder().header(header::CONTENT_TYPE,content).body(Body::from(bytes)).unwrap_or_default()
        }
        _=>Response::builder().status(StatusCode::SERVICE_UNAVAILABLE).body(Body::from("Administration assets unavailable")).unwrap_or_default(),
    }
}

#[tokio::main]
async fn main()->anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).init();
    let origin=std::env::var("ADMIN_ORIGIN")?;let parsed=url::Url::parse(&origin)?;
    anyhow::ensure!(parsed.scheme()=="https"&&parsed.host_str().is_some()&&parsed.path()=="/"&&parsed.query().is_none()&&parsed.fragment().is_none(),"ADMIN_ORIGIN must be an HTTPS origin");
    let mut key=zeroize::Zeroizing::new(bounded_file(std::path::Path::new(&std::env::var("ADMIN_MASTER_KEY_FILE")?),66).await?);
    if key.len()!=32{*key=hex::decode(std::str::from_utf8(&key)?.trim())?;}
    let vault=Arc::new(crypto::Vault::new(&key)?);
    let db=PgPoolOptions::new().max_connections(16).acquire_timeout(Duration::from_secs(5)).connect(&database_url().await?).await?;
    sqlx::migrate!().run(&db).await?;
    let client=reqwest::Client::builder().timeout(Duration::from_secs(5)).connect_timeout(Duration::from_secs(3)).redirect(reqwest::redirect::Policy::none()).no_proxy().build()?;
    let geoip=Arc::new(geoip::GeoIp::load(std::env::var_os("ADMIN_GEOIP_DATABASE").map(Into::into)).await);
    let state=State{db,vault,client,geoip,hash_slots:Arc::new(tokio::sync::Semaphore::new(4)),
        #[cfg(test)] cloudflare_origin:None,
        origin:parsed.origin().ascii_serialization().into(),assets:Arc::new(std::env::var("ADMIN_ASSETS_DIR").unwrap_or_else(|_|"../frontend/dist".into()).into())};
    let args:Vec<_>=std::env::args().collect();
    if args.get(1).map(String::as_str)==Some("reset-owner") {
        let username=args.get(2).ok_or_else(||anyhow::anyhow!("usage: telemt-admin reset-owner USER PASSWORD_FILE"))?;
        let password=zeroize::Zeroizing::new(String::from_utf8(bounded_file(std::path::Path::new(args.get(3).ok_or_else(||anyhow::anyhow!("missing password file"))?),1025).await?)?);
        println!("{}",auth::reset_owner(&state,username,password.trim_end()).await?);return Ok(());
    }
    if args.get(1).map(String::as_str)==Some("create-owner"){
        let username=args.get(2).ok_or_else(||anyhow::anyhow!("usage: telemt-admin create-owner USER PASSWORD_FILE"))?;
        let password=zeroize::Zeroizing::new(String::from_utf8(bounded_file(std::path::Path::new(args.get(3).ok_or_else(||anyhow::anyhow!("missing password file"))?),1025).await?)?);
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM administrators WHERE role='owner' AND enabled").fetch_one(&state.db).await?;
        anyhow::ensure!(count==0,"an enabled owner already exists; use the cabinet to create accounts");
        println!("{}",auth::create(&state,username,password.trim_end(),"owner").await?);return Ok(());
    }
    operations::recover(&state).await?;
    let cancel=CancellationToken::new();let mut tasks=tokio::task::JoinSet::new();
    for (name,period) in [("monitor",5),("operations",1),("probes",15),("dns",15),("rotation",5),("retention",60)] {
        let state=state.clone();let cancel=cancel.clone();tasks.spawn(async move{
            let mut interval=tokio::time::interval(Duration::from_secs(period));interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop{tokio::select!{_ = cancel.cancelled()=>break,_ = interval.tick()=>{}}
                let result=match name{"monitor"=>monitor::collect(&state).await,"operations"=>operations::tick(&state).await,"probes"=>probes::schedule(&state).await,"dns"=>dns::tick(&state).await,"rotation"=>rotation::tick(&state).await,_=>monitor::retention(&state).await};
                if result.is_err(){tracing::warn!(component=name,"control task failed; retrying next bounded interval");}
            }
        });
    }
    let app=application(state.clone());
    let listener=tokio::net::TcpListener::bind(std::env::var("ADMIN_LISTEN").unwrap_or_else(|_|"127.0.0.1:8080".into())).await?;
    axum::serve(listener,app).with_graceful_shutdown(shutdown_signal()).await?;
    cancel.cancel();
    if tokio::time::timeout(Duration::from_secs(45),async{while tasks.join_next().await.is_some(){}}).await.is_err() {
        tasks.abort_all(); while tasks.join_next().await.is_some() {}
        tracing::warn!("control tasks exceeded shutdown deadline; durable intents require reconciliation");
    }
    state.db.close().await;Ok(())
}

fn application(state:State)->Router {
    Router::new().route("/api/login",post(auth::login)).route("/api/session",get(auth::current)).route("/api/logout",post(auth::logout))
        .route("/api/{*path}",any(routes::dispatch)).route("/agent/jobs",get(probes::jobs)).route("/agent/results",post(probes::result)).fallback(assets)
        .layer(DefaultBodyLimit::max(1024*1024)).layer(middleware::from_fn(security)).with_state(state)

}

// Check size before allocation, then cap the read again in case the file changes concurrently.
async fn bounded_file(path:&std::path::Path,limit:usize)->anyhow::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let metadata=tokio::fs::metadata(path).await?;
    anyhow::ensure!(metadata.is_file()&&metadata.len()<=limit as u64,"invalid file type or size");
    let mut bytes=Vec::with_capacity(metadata.len() as usize);let file=tokio::fs::File::open(path).await?;
    file.take(limit as u64+1).read_to_end(&mut bytes).await?;
    anyhow::ensure!(bytes.len()<=limit,"file changed beyond size limit");Ok(bytes)
}

#[cfg(test)]
mod integration_tests;

async fn shutdown_signal() {
    #[cfg(unix)]
    if let Ok(mut terminate) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
        return;
    }
    let _ = tokio::signal::ctrl_c().await;
}

async fn database_url()->anyhow::Result<zeroize::Zeroizing<String>> {
    let value = if let Ok(path) = std::env::var("DATABASE_URL_FILE") { String::from_utf8(bounded_file(std::path::Path::new(&path),4096).await?)? } else { std::env::var("DATABASE_URL")? };
    Ok(zeroize::Zeroizing::new(value.trim().to_owned()))
}
