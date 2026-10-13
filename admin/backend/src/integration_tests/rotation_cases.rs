use super::*;

pub(super) async fn run(state:&State,app:&Router,mock:&Mock,node_id:Uuid,owner:&(String,String),viewer:&(String,String),agents:&[(Uuid,String)]) {
    let listeners=json!([{"ip":"192.0.2.1","port":443,"transport":"mtproxy","synlimit":"off"},{"ip":"2001:db8::1","port":443,"transport":"mtproxy","synlimit":"off"}]);
    *mock.config.lock().unwrap()=json!({"server":{"listeners":listeners}});
    mock.records.lock().unwrap().get_mut("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()["content"]=json!("192.0.2.1");
    mock.records.lock().unwrap().get_mut("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap()["content"]=json!("2001:db8::1");
    sqlx::query("UPDATE nodes SET enabled=true,collected_at=now(),snapshot=$2 WHERE id=$1").bind(node_id).bind(json!({"ready":true,"compatibility":{"fingerprint":"a".repeat(64)}})).execute(&state.db).await.unwrap();
    let (status,result,_)=request(app,"/api/groups","POST",json!({"name":"single-node","domain":"proxy.test","zone_id":"00000000000000000000000000000000","record_a_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","record_aaaa_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","active_node":node_id,"node_ids":[node_id],"token":"test-dns-token"}),Some(owner),true).await;assert_eq!(status,StatusCode::OK);
    let group=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    let body=json!({"name":"assigned addresses","node_id":node_id,"group_id":group,"addresses":["192.0.2.1","192.0.2.3","192.0.2.4","2001:db8::1"]});
    assert_eq!(request(app,"/api/ip-pools","POST",body.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    let mut invalid=body.clone();invalid["addresses"]=json!(["192.0.2.1","192.0.2.99"]);assert_eq!(request(app,"/api/ip-pools","POST",invalid,Some(owner),true).await.0,StatusCode::CONFLICT);
    let (status,result,_)=request(app,"/api/ip-pools","POST",body,Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{result}");let pool=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    assert!(!sqlx::query_scalar::<_,bool>("SELECT automatic FROM ip_pools WHERE id=$1").bind(pool).fetch_one(&state.db).await.unwrap());
    for action in ["switch","repair"] {assert_eq!(request(app,&format!("/api/groups/{group}/{action}"),"POST",json!({"node_id":node_id,"expected_records":{}}),Some(owner),true).await.0,StatusCode::CONFLICT);}
    let (status,preview,_)=request(app,&format!("/api/ip-pools/{pool}/preview"),"GET",json!({}),Some(owner),false).await;assert_eq!(status,StatusCode::OK);assert_eq!(preview["data"]["listener_patch"]["server"]["listeners"].as_array().unwrap().len(),4);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/prepare"),"POST",json!({"revision":"stale"}),Some(owner),true).await.0,StatusCode::CONFLICT);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/prepare"),"POST",json!({"revision":"r2"}),Some(owner),true).await.0,StatusCode::OK);
    operations::tick(state).await.unwrap();operations::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM ip_pools WHERE id=$1").bind(pool).fetch_one(&state.db).await.unwrap(),"prepared");
    seed(state,node_id,agents,"192.0.2.1",false).await;for ip in ["192.0.2.3","192.0.2.4","2001:db8::1"]{seed(state,node_id,agents,ip,true).await;}
    let (status,preview,_)=request(app,&format!("/api/ip-pools/{pool}/rotation-preview"),"POST",json!({"ip":"192.0.2.3"}),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{preview}");
    let (status,result,_)=request(app,&format!("/api/ip-pools/{pool}/rotate"),"POST",preview["data"].clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK);let rotation=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(request(app,"/api/operations","POST",json!({"node_ids":[node_id],"method":"PATCH","path":"/v1/config","body":{},"revisions":{node_id.to_string():"r2"}}),Some(owner),true).await.0,StatusCode::CONFLICT);
    assert_eq!(request(app,&format!("/api/nodes/{node_id}"),"PATCH",json!({"enabled":false}),Some(owner),true).await.0,StatusCode::CONFLICT);
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.3");
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT document->>'ipv4' FROM nodes WHERE id=$1").bind(node_id).fetch_one(&state.db).await.unwrap(),"192.0.2.3");
    crate::rotation::tick(state).await.unwrap();operations::tick(state).await.unwrap();operations::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM ip_rotations WHERE id=$1").bind(rotation).fetch_one(&state.db).await.unwrap(),"succeeded");
    assert!(!mock.config.lock().unwrap()["server"]["listeners"].as_array().unwrap().iter().any(|listener|listener["ip"]=="192.0.2.1"));
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/rotate"),"POST",preview["data"].clone(),Some(owner),true).await.0,StatusCode::CONFLICT);
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();sqlx::query("UPDATE failover_groups SET last_switch=NULL").execute(&state.db).await.unwrap();sqlx::query("UPDATE ip_pools SET last_rotation=NULL").execute(&state.db).await.unwrap();
    seed(state,node_id,agents,"192.0.2.3",false).await;
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}"),"PATCH",json!({"automatic":true}),Some(owner),true).await.0,StatusCode::OK);
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state='queued'").bind(pool).fetch_one(&state.db).await.unwrap(),1);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}"),"PATCH",json!({"automatic":false}),Some(owner),true).await.0,StatusCode::OK);
    crate::rotation::tick(state).await.unwrap();assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.3");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state='queued'").bind(pool).fetch_one(&state.db).await.unwrap(),0);
    let (_,preview,_)=request(app,&format!("/api/ip-pools/{pool}/rotation-preview"),"POST",json!({"ip":"192.0.2.4"}),Some(owner),true).await;
    let (status,result,_)=request(app,&format!("/api/ip-pools/{pool}/rotate"),"POST",preview["data"].clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK);let rotation=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    mock.fail_patch.store(true,std::sync::atomic::Ordering::Relaxed);crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT document->>'ipv4' FROM nodes WHERE id=$1").bind(node_id).fetch_one(&state.db).await.unwrap(),"192.0.2.3");
    // A changed address with an incomplete companion record is still a repairable partial write.
    mock.records.lock().unwrap().get_mut("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap()["ttl"]=json!(300);
    let (status,repair,_)=request(app,&format!("/api/ip-pools/{pool}/dns-repair-preview"),"GET",json!({}),Some(owner),false).await;assert_eq!(status,StatusCode::OK);
    let mut stale=repair["data"].clone();stale["expected_records"]["A"]["content"]=json!("192.0.2.99");
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/repair-dns"),"POST",stale,Some(owner),true).await.0,StatusCode::CONFLICT);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/repair-dns"),"POST",repair["data"].clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    let (status,result,_)=request(app,&format!("/api/ip-pools/{pool}/repair-dns"),"POST",repair["data"].clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{result}");
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM ip_rotations WHERE id=$1").bind(rotation).fetch_one(&state.db).await.unwrap(),"queued");
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT document->>'ipv4' FROM nodes WHERE id=$1").bind(node_id).fetch_one(&state.db).await.unwrap(),"192.0.2.4");
    crate::rotation::tick(state).await.unwrap();*mock.revision.lock().unwrap()="changed-externally".into();operations::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM ip_rotations WHERE id=$1").bind(rotation).fetch_one(&state.db).await.unwrap(),"unknown");
    let (status,preview,_)=request(app,&format!("/api/ip-pools/{pool}/retirement-preview"),"GET",json!({}),Some(owner),false).await;assert_eq!(status,StatusCode::OK);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/repair-listener"),"POST",preview["data"].clone(),Some(owner),true).await.0,StatusCode::OK);
    operations::tick(state).await.unwrap();operations::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM ip_rotations WHERE id=$1").bind(rotation).fetch_one(&state.db).await.unwrap(),"succeeded");
    assert_eq!(request(app,&format!("/api/groups/{group}"),"PATCH",json!({"automatic":true}),Some(owner),true).await.0,StatusCode::CONFLICT);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}"),"DELETE",json!({}),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.4");
    // An unowned IPv6 endpoint must not prevent an A-only pool from rotating.
    sqlx::query("UPDATE failover_groups SET document=document-'record_aaaa_id',last_switch=NULL WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    sqlx::query("DELETE FROM probe_jobs WHERE node_id=$1 AND family=6").bind(node_id).execute(&state.db).await.unwrap();
    let (status,result,_)=request(app,"/api/ip-pools","POST",json!({"name":"A-only","node_id":node_id,"group_id":group,"addresses":["192.0.2.3","192.0.2.4"]}),Some(owner),true).await;assert_eq!(status,StatusCode::OK);
    let pool=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/prepare"),"POST",json!({"revision":"r2"}),Some(owner),true).await.0,StatusCode::OK);
    operations::tick(state).await.unwrap();operations::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    seed(state,node_id,agents,"192.0.2.4",false).await;seed(state,node_id,agents,"192.0.2.3",true).await;
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}"),"PATCH",json!({"automatic":true}),Some(owner),true).await.0,StatusCode::OK);
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM ip_rotations WHERE pool_id=$1 AND state='queued'").bind(pool).fetch_one(&state.db).await.unwrap(),1);
}
async fn seed(state:&State,node:Uuid,agents:&[(Uuid,String)],ip:&str,good:bool) {
    let last=chrono::Utc::now().timestamp()/15-1;let family=if ip.contains(':'){6}else{4};
    sqlx::query("DELETE FROM probe_jobs WHERE node_id=$1 AND address=$2").bind(node).bind(ip).execute(&state.db).await.unwrap();
    for offset in 0..6 {for (agent,_) in agents {sqlx::query("INSERT INTO probe_jobs(id,round,agent_id,node_id,family,address,expires_at,completed_at,result) VALUES($1,$2,$3,$4,$5,$6,now()+interval '1 minute',now(),$7)").bind(Uuid::new_v4()).bind(last-offset).bind(agent).bind(node).bind(family).bind(ip).bind(json!({"network_ok":true,"reachable":good})).execute(&state.db).await.unwrap();}}
}
