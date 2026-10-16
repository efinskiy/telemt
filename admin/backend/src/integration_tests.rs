//! Exercises the actual router, migrations, crypto, operations and DNS adapter against PostgreSQL.
use super::*;
use axum::{body::to_bytes,http::{Method,Request,HeaderMap},extract::{Path,State as ExtractState},Json};
use serde_json::{Value,json};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;
use std::collections::HashMap;
mod rotation_cases;
mod connection_maps;
mod node_api_urls;
mod dns_enrollment;
mod forced_switch;
mod managed_fixture;

#[derive(Clone)]struct Mock {config:Arc<std::sync::Mutex<Value>>,revision:Arc<std::sync::Mutex<String>>,records:Arc<std::sync::Mutex<HashMap<String,Value>>>,fail_patch:Arc<std::sync::atomic::AtomicBool>,stall_patch:Arc<std::sync::atomic::AtomicBool>,patch_started:Arc<tokio::sync::Notify>,user_ready:Arc<std::sync::atomic::AtomicBool>}
async fn node(ExtractState(mock):ExtractState<Mock>,headers:HeaderMap,req:axum::extract::Request)->(StatusCode,Json<Value>) {
    let path=req.uri().path().to_owned();let method=req.method().clone();
    let body:Value=serde_json::from_slice(&to_bytes(req.into_body(),1024*1024).await.unwrap()).unwrap_or(json!({}));
    if headers.get("authorization").and_then(|v|v.to_str().ok())!=Some("Bearer test-node-token"){return (StatusCode::UNAUTHORIZED,Json(json!({"ok":false})));}
    let data=match (method.as_str(),path.as_str()) {
        ("GET","/v1/config")=>mock.config.lock().unwrap().clone(),
        ("PATCH","/v1/config")=>{
            let mut revision=mock.revision.lock().unwrap();
            if headers.get("if-match").and_then(|v|v.to_str().ok())!=Some(revision.as_str()){return(StatusCode::CONFLICT,Json(json!({"ok":false,"error":{"code":"revision_conflict"}})));}
            *mock.config.lock().unwrap()=body;*revision="r2".into();
            return(StatusCode::ACCEPTED,Json(json!({"ok":true,"data":{"reload":{"reload_id":1,"config_revision":"r2"}},"revision":"r2"})));
        }
        ("GET","/v1/system/reload/1")=>json!({"reload_id":1,"state":"succeeded","config_revision":"r2"}),
        ("POST","/v1/users")=>{return(StatusCode::ACCEPTED,Json(json!({"ok":true,"data":{"user":{"username":"new-user","enabled":true,"in_runtime":false},"secret":body["secret"]},"revision":"r2"})));}
        ("GET","/v1/users/new-user")=>json!({"username":"new-user","enabled":true,"in_runtime":mock.user_ready.load(std::sync::atomic::Ordering::Relaxed)}),
        ("GET","/v1/users")=>json!([{"username":"alice","links":{"tls":["tg://proxy?secret=hidden"]},"enabled":true}]),
        ("GET","/v1/runtime/clients")=>json!({"snapshot":{"ips":[{"ip":"1.1.1.1","connections":3,"users":2},{"ip":"2606:4700:4700::1111","connections":2,"users":1}],"total_ips":2,"connections":5,"active_users":2,"cleanup_pending":0,"truncated":false}}),
        ("GET","/v1/fleet/compatibility")=>json!({"fingerprint":"a".repeat(64)}),
        ("GET","/v1/health/ready")=>json!({"ready":true}),
        ("GET","/v1/runtime/interfaces")=>json!({"addresses":[{"ip":"192.0.2.1","usable":true,"interface":"eth0"},{"ip":"192.0.2.3","usable":true,"interface":"eth0"},{"ip":"192.0.2.4","usable":true,"interface":"eth0"},{"ip":"2001:db8::1","usable":true,"interface":"eth0"}],"active_listeners":mock.config.lock().unwrap()["server"]["listeners"],"runtime_generation":1}),
        ("GET","/v1/system/info")=>json!({"process_started_at_epoch_secs":1}),
        _=>json!({}),
    };
    let revision=mock.revision.lock().unwrap().clone();(StatusCode::OK,Json(json!({"ok":true,"data":data,"revision":revision})))
}
async fn cloudflare(ExtractState(mock):ExtractState<Mock>,Path(path):Path<String>,headers:HeaderMap,req:axum::extract::Request)->(StatusCode,Json<Value>) {
    assert_eq!(headers.get("authorization").unwrap(),"Bearer test-dns-token");
    let method=req.method().clone();let body:Value=serde_json::from_slice(&to_bytes(req.into_body(),1024*1024).await.unwrap()).unwrap_or(json!({}));
    if method==Method::PATCH && mock.stall_patch.swap(false,std::sync::atomic::Ordering::Relaxed) {mock.patch_started.notify_one();tokio::time::sleep(Duration::from_millis(500)).await;}
    let id=path.rsplit('/').next().unwrap_or("");let mut records=mock.records.lock().unwrap();
    let Some(record)=records.get_mut(id)else{return(StatusCode::NOT_FOUND,Json(json!({"success":false})));};
    if method==Method::PATCH {record["content"]=body["content"].clone();record["ttl"]=body["ttl"].clone();record["proxied"]=body["proxied"].clone();record["modified_on"]=json!("changed");if mock.fail_patch.swap(false,std::sync::atomic::Ordering::Relaxed){return(StatusCode::BAD_GATEWAY,Json(json!({"success":false})));}}
    (StatusCode::OK,Json(json!({"success":true,"result":record})))
}
async fn request(app:&Router,path:&str,method:&str,body:Value,session:Option<&(String,String)>,csrf:bool)->(StatusCode,Value,HeaderMap) {
    let mut request=Request::builder().uri(path).method(method).header("origin","https://admin.test").header("content-type","application/json");
    if let Some((cookie,token))=session {request=request.header("cookie",cookie);if csrf{request=request.header("x-csrf-token",token);}}
    let response=app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
    let status=response.status();let headers=response.headers().clone();let body=to_bytes(response.into_body(),2*1024*1024).await.unwrap();(status,serde_json::from_slice(&body).unwrap(),headers)
}
async fn login(state:&State,app:&Router,username:&str)->(String,String) {
    let row=sqlx::query("SELECT id,totp_cipher FROM administrators WHERE username=$1").bind(username).fetch_one(&state.db).await.unwrap();let id:Uuid=row.get("id");
    let secret=state.vault.open(&row.get::<String,_>("totp_cipher"),&format!("admin:{id}")).unwrap();let code=crypto::totp(&secret,(chrono::Utc::now().timestamp()/30)as u64).unwrap();
    let (status,response,headers)=request(app,"/api/login","POST",json!({"username":username,"password":"long-test-password-only","totp":code}),None,false).await;
    assert_eq!(status,StatusCode::OK);let cookie=headers.get("set-cookie").unwrap().to_str().unwrap();assert!(cookie.contains("Secure")&&cookie.contains("HttpOnly")&&cookie.contains("SameSite=Strict"));
    (cookie.split(';').next().unwrap().into(),response["data"]["csrf"].as_str().unwrap().into())
}

#[tokio::test]
#[ignore="requires TELEMT_TEST_DATABASE_URL pointing to a disposable PostgreSQL database"]
async fn postgres_router_auth_operations_probes_and_dns() {
    if managed_fixture::isolated_pki().await{return;}
    let url=std::env::var("TELEMT_TEST_DATABASE_URL").expect("disposable test database required");
    let root=PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
    let schema=format!("test_{}",Uuid::new_v4().simple());sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&root).await.unwrap();
    let search=schema.clone();let db=PgPoolOptions::new().max_connections(16).after_connect(move|connection,_|{let search=search.clone();Box::pin(async move{sqlx::query(&format!("SET search_path TO {search}")).execute(connection).await?;Ok(())})}).connect(&url).await.unwrap();
    sqlx::migrate!().run(&db).await.unwrap();
    let mock=Mock{config:Arc::new(std::sync::Mutex::new(json!({"shaping":{"enabled":false}}))),revision:Arc::new(std::sync::Mutex::new("r1".into())),records:Arc::new(std::sync::Mutex::new(HashMap::from([("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),json!({"name":"proxy.test","type":"A","content":"192.0.2.1","ttl":60,"proxied":false,"modified_on":"original"})),("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),json!({"name":"proxy.test","type":"AAAA","content":"2001:db8::1","ttl":60,"proxied":false,"modified_on":"original"}))]))),fail_patch:Arc::new(std::sync::atomic::AtomicBool::new(false)),stall_patch:Arc::new(std::sync::atomic::AtomicBool::new(false)),patch_started:Arc::new(tokio::sync::Notify::new()),user_ready:Arc::new(std::sync::atomic::AtomicBool::new(false))};
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let port=listener.local_addr().unwrap().port();let mock_app=Router::new().route("/v1/{*path}",any(node)).route("/zones/{*path}",any(cloudflare)).with_state(mock.clone());
    let server=tokio::spawn(async move{axum::serve(listener,mock_app).await.unwrap();});
    let state=State{db,probe_round_override:Some(chrono::Utc::now().timestamp()/15),geoip:Arc::new(crate::geoip::GeoIp::unavailable("missing")),vault:Arc::new(crypto::Vault::new(&[17;32]).unwrap()),client:reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).no_proxy().timeout(Duration::from_secs(3)).build().unwrap(),hash_slots:Arc::new(tokio::sync::Semaphore::new(4)),cloudflare_origin:Some(format!("http://127.0.0.1:{port}").into()),origin:"https://admin.test".into(),assets:Arc::new("/private/tmp/unavailable-admin-assets".into())};
    auth::create(&state,"owner","long-test-password-only","owner").await.unwrap();auth::create(&state,"viewer","long-test-password-only","viewer").await.unwrap();
    let app=application(state.clone());let owner=login(&state,&app,"owner").await;let viewer=login(&state,&app,"viewer").await;
    assert_eq!(request(&app,"/api/nodes","GET",json!({}),None,false).await.0,StatusCode::UNAUTHORIZED);
    assert_eq!(request(&app,"/api/nodes","POST",json!({}),Some(&owner),false).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(&app,"/api/nodes","POST",json!({}),Some(&viewer),true).await.0,StatusCode::BAD_REQUEST);
    node_api_urls::run(&state,&app,&owner,&viewer,port).await;
    let secret="0123456789abcdef0123456789abcdef";
    let nodes=vec![Uuid::new_v4(),Uuid::new_v4()];
    managed_fixture::seed(&state,&nodes,secret).await;
    let managed_agent=managed_fixture::start(state.clone(),app.clone(),port,nodes.clone());
    connection_maps::run(&state,&app,&owner,&viewer,nodes[0]).await;
    let cipher:String=sqlx::query_scalar("SELECT probe_cipher FROM nodes WHERE id=$1").bind(nodes[0]).fetch_one(&state.db).await.unwrap();assert!(!cipher.contains(secret));
    let (_,response,_)=request(&app,&format!("/api/nodes/{}/read?path=%2Fv1%2Fusers",nodes[0]),"GET",json!({}),Some(&viewer),false).await;
    assert!(response["data"]["response"]["data"][0].get("links").is_none());
    for (method,path) in [("PATCH","/v1/config"),("POST","/v1/users"),("DELETE","/v1/users/alice")] {
        assert_eq!(request(&app,"/api/operations","POST",json!({"node_ids":[nodes[0]],"method":method,"path":path,"body":{},"revisions":{nodes[0].to_string():"r1"}}),Some(&owner),true).await.0,StatusCode::BAD_REQUEST);
    }
    let (status,response,_)=request(&app,"/api/operations","POST",json!({"node_ids":[nodes[0]],"method":"POST","path":"/v1/runtime/web/lifecycle/pause","body":{},"revisions":{nodes[0].to_string():"r1"}}),Some(&owner),true).await;
    assert_eq!(status,StatusCode::OK);let operation=Uuid::parse_str(response["data"]["id"].as_str().unwrap()).unwrap();operations::tick(&state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM operations WHERE id=$1").bind(operation).fetch_one(&state.db).await.unwrap(),"succeeded");
    // A crash after only one target must preserve every unattempted target and never report success.
    let interrupted=Uuid::new_v4();let intent=json!({"node_ids":nodes,"method":"PATCH","path":"/v1/config","body":{"shaping":{"enabled":true}}});
    let cipher=state.vault.seal(&intent.to_string(),&format!("operation:{interrupted}")).unwrap();
    sqlx::query("INSERT INTO operations(id,actor,state,request_cipher,results) VALUES($1,$2,'running',$3,$4)").bind(interrupted).bind(sqlx::query_scalar::<_,Uuid>("SELECT id FROM administrators WHERE username='owner'").fetch_one(&state.db).await.unwrap()).bind(cipher).bind(json!([{"node_id":nodes[0],"state":"succeeded"}])).execute(&state.db).await.unwrap();
    operations::recover(&state).await.unwrap();let row=sqlx::query("SELECT state,results FROM operations WHERE id=$1").bind(interrupted).fetch_one(&state.db).await.unwrap();assert_eq!(row.get::<String,_>("state"),"unknown");assert_eq!(row.get::<Value,_>("results").as_array().unwrap().len(),2);
    let mut agents=Vec::new();
    for network in ["independent-a"] {let (_,result,_)=request(&app,"/api/agents","POST",json!({"name":network,"network":network,"country":"RU"}),Some(&owner),true).await;agents.push((Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap(),result["data"]["token"].as_str().unwrap().to_owned()));}
    probes::schedule(&state).await.unwrap();
    let response=app.clone().oneshot(Request::builder().uri("/agent/jobs").header("authorization",format!("Bearer {}",agents[0].1)).body(Body::empty()).unwrap()).await.unwrap();let response:Value=serde_json::from_slice(&to_bytes(response.into_body(),1024*1024).await.unwrap()).unwrap();let job=response["data"][0]["id"].clone();
    for expected in [StatusCode::OK,StatusCode::CONFLICT] {let response=app.clone().oneshot(Request::builder().method("POST").uri("/agent/results").header("authorization",format!("Bearer {}",agents[0].1)).header("content-type","application/json").body(Body::from(json!({"id":job,"network_ok":true,"reachable":true,"stage":"ok","latency_ms":1,"native":{"reachable":true,"stage":"ok","latency_ms":1},"web":{"reachable":true,"stage":"ok","latency_ms":1}}).to_string())).unwrap()).await.unwrap();assert_eq!(response.status(),expected);}
    for node in &nodes {sqlx::query("UPDATE nodes SET collected_at=now(),snapshot=$2 WHERE id=$1").bind(node).bind(json!({"ready":true,"compatibility":{"fingerprint":"a".repeat(64)}})).execute(&state.db).await.unwrap();}
    dns_enrollment::run(&state,&app,&owner,nodes[0],&mock).await;
    for (index,target) in nodes.iter().enumerate(){for ip in [format!("192.0.2.{}",index+1),format!("2001:db8::{}",index+1)]{rotation_cases::seed(&state,*target,&agents,&ip,true).await;}}
    assert_eq!(probes::health(&state,nodes[0],4).await.unwrap(),probes::Health::Recovered);
    sqlx::query("UPDATE probe_jobs SET completed_at=now()-interval '301 seconds' WHERE node_id=$1 AND completed_at IS NOT NULL").bind(nodes[0]).execute(&state.db).await.unwrap();
    assert_eq!(probes::health(&state,nodes[0],4).await.unwrap(),probes::Health::Unknown);
    sqlx::query("UPDATE probe_jobs SET completed_at=now() WHERE node_id=$1 AND completed_at IS NOT NULL").bind(nodes[0]).execute(&state.db).await.unwrap();
    let mut next_round=state.clone();next_round.probe_round_override=Some(state.probe_round()+1);
    assert_eq!(probes::health(&next_round,nodes[0],4).await.unwrap(),probes::Health::Unknown);
    let (status,response,_)=request(&app,"/api/groups","POST",json!({"name":"proxy","domain":"proxy.test","zone_id":"00000000000000000000000000000000","record_a_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","record_aaaa_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","active_node":nodes[0],"node_ids":nodes,"token":"test-dns-token"}),Some(&owner),true).await;
    assert_eq!(status,StatusCode::OK);let group=Uuid::parse_str(response["data"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(request(&app,&format!("/api/groups/{group}/preview"),"POST",json!({"node_id":nodes[1]}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(request(&app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":nodes[1]}),Some(&owner),true).await.0,StatusCode::CONFLICT);
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,false).await;}
    assert_eq!(request(&app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":nodes[1]}),Some(&owner),true).await.0,StatusCode::OK);
    let active:Uuid=sqlx::query_scalar("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap();assert_eq!(active,nodes[1]);
    let changes:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE state='verified'").fetch_one(&state.db).await.unwrap();assert_eq!(changes,1);
    assert_eq!(request(&app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":nodes[1]}),Some(&owner),true).await.0,StatusCode::CONFLICT);
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET last_switch=NULL WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,true).await;}
    for ip in ["192.0.2.2","2001:db8::2"]{rotation_cases::seed(&state,nodes[1],&agents,ip,false).await;}
    mock.fail_patch.store(true,std::sync::atomic::Ordering::Relaxed);
    assert_eq!(request(&app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":nodes[0]}),Some(&owner),true).await.0,StatusCode::BAD_GATEWAY);
    assert_eq!(request(&app,&format!("/api/groups/{group}/reconcile"),"POST",json!({}),Some(&owner),true).await.0,StatusCode::OK);
    let unknown:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE state='unknown'").fetch_one(&state.db).await.unwrap();assert_eq!(unknown,1);
    let (_,records,_)=request(&app,&format!("/api/groups/{group}/records"),"GET",json!({}),Some(&owner),false).await;
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,true).await;}
    let (status,response,_)=request(&app,&format!("/api/groups/{group}/repair"),"POST",json!({"node_id":nodes[0],"expected_records":records["data"]}),Some(&owner),true).await;
    assert_eq!(status,StatusCode::OK,"{response}");
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM dns_changes WHERE state IN ('writing','unknown')").fetch_one(&state.db).await.unwrap();assert_eq!(pending,0);
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET last_switch=NULL,automatic=true WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,false).await;}
    for ip in ["192.0.2.2","2001:db8::2"]{rotation_cases::seed(&state,nodes[1],&agents,ip,true).await;}
    mock.stall_patch.store(true,std::sync::atomic::Ordering::Relaxed);
    let pending_app=app.clone();let pending_owner=owner.clone();let reserve=nodes[1];
    let pending=tokio::spawn(async move{request(&pending_app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":reserve}),Some(&pending_owner),true).await});
    tokio::time::timeout(Duration::from_secs(2),mock.patch_started.notified()).await.unwrap();pending.abort();let _=pending.await;
    let mut lock=state.db.begin().await.unwrap();
    let acquired:bool=sqlx::query_scalar("SELECT pg_try_advisory_xact_lock($1)").bind(crate::state::DNS_COORDINATOR_LOCK).fetch_one(&mut *lock).await.unwrap();assert!(acquired,"cancelled DNS request leaked its coordinator lock");lock.rollback().await.unwrap();
    tokio::time::sleep(Duration::from_millis(550)).await;
    assert_eq!(request(&app,&format!("/api/groups/{group}/reconcile"),"POST",json!({}),Some(&owner),true).await.0,StatusCode::OK);
    let (_,records,_)=request(&app,&format!("/api/groups/{group}/records"),"GET",json!({}),Some(&owner),false).await;
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,true).await;}
    let (status,response,_)=request(&app,&format!("/api/groups/{group}/repair"),"POST",json!({"node_id":nodes[0],"expected_records":records["data"]}),Some(&owner),true).await;
    assert_eq!(status,StatusCode::OK,"{response}");
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET last_switch=NULL,automatic=true WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    for ip in ["192.0.2.1","2001:db8::1"]{rotation_cases::seed(&state,nodes[0],&agents,ip,false).await;}
    mock.records.lock().unwrap().get_mut("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()["content"]=json!("192.0.2.99");
    assert_eq!(request(&app,&format!("/api/groups/{group}/switch"),"POST",json!({"node_id":nodes[1]}),Some(&owner),true).await.0,StatusCode::CONFLICT);
    assert!(!sqlx::query_scalar::<_,bool>("SELECT automatic FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap());
    assert_eq!(request(&app,&format!("/api/groups/{group}"),"PATCH",json!({"node_ids":[nodes[1],nodes[0]],"token":"test-dns-token"}),Some(&owner),true).await.0,StatusCode::OK);
    let order:Value=sqlx::query_scalar("SELECT document->'node_ids' FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap();assert_eq!(order,json!([nodes[1],nodes[0]]));
    assert_eq!(request(&app,&format!("/api/nodes/{}",nodes[1]),"PATCH",json!({"enabled":false}),Some(&owner),true).await.0,StatusCode::BAD_REQUEST);
    assert_eq!(request(&app,&format!("/api/groups/{group}"),"PATCH",json!({"automatic":false}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(request(&app,&format!("/api/groups/{group}"),"PATCH",json!({"automatic":true}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(request(&app,&format!("/api/groups/{group}/verify-configuration"),"POST",json!({}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(request(&app,&format!("/api/groups/{group}"),"DELETE",json!({}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.99");
    // Exercise forced recovery with agents genuinely offline, then resume command processing.
    managed_agent.abort();let _=managed_agent.await;
    forced_switch::run(&state,&app,&mock,&nodes,&owner,&viewer).await;
    let managed_agent=managed_fixture::start(state.clone(),app.clone(),port,nodes.clone());
    rotation_cases::run(&state,&app,&mock,&nodes,&owner,&viewer,&agents).await;
    let (_,session,_)=request(&app,"/api/session","GET",json!({}),Some(&owner),false).await;assert_eq!(session["data"]["csrf"],owner.1);
    assert_eq!(request(&app,"/api/logout","POST",json!({}),Some(&owner),true).await.0,StatusCode::OK);
    assert_eq!(request(&app,"/api/nodes","GET",json!({}),Some(&owner),false).await.0,StatusCode::UNAUTHORIZED);
    let audits:i64=sqlx::query_scalar("SELECT count(*) FROM audit").fetch_one(&state.db).await.unwrap();assert!(audits>=10);
    managed_agent.abort();let _=managed_agent.await;
    server.abort();let _=server.await;state.db.close().await;sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&root).await.unwrap();root.close().await;
}
