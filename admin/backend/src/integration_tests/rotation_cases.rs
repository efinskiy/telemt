//! Address exhaustion and local-first ordering run against real PostgreSQL/controller routes.
use super::*;

pub(super) async fn seed(state:&State,node:Uuid,agents:&[(Uuid,String)],ip:&str,good:bool) {
    sqlx::query("DELETE FROM probe_jobs WHERE node_id=$1 AND address=$2").bind(node).bind(ip).execute(&state.db).await.unwrap();
    let last=state.probe_round()-1;
    for offset in 0..22 {for (agent,_) in agents {
        sqlx::query("INSERT INTO probe_jobs(id,round,agent_id,node_id,family,address,expires_at,completed_at,result) VALUES($1,$2,$3,$4,$5,$6,now()+interval '1 minute',now()-make_interval(secs => $8),$7)")
            .bind(Uuid::new_v4()).bind(last-offset).bind(agent).bind(node).bind(if ip.contains(':'){6}else{4}).bind(ip)
            .bind(json!({"check":"tcp_tls22","network_ok":true,"reachable":good,"tcp22":{"port":22,"reachable":good,"stage":if good{"wrong_version_number"}else{"timeout"},"phase":"tls","latency_ms":1}})).bind((offset*15) as f64).execute(&state.db).await.unwrap();
    }}
    sqlx::query("UPDATE nodes SET collected_at=now() WHERE id=$1").bind(node).execute(&state.db).await.unwrap();
}
async fn cooldown(state:&State) {
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET last_switch=NULL").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE ip_pools SET last_rotation=NULL").execute(&state.db).await.unwrap();
}

pub(super) async fn run(state:&State,app:&Router,mock:&Mock,nodes:&[Uuid],owner:&(String,String),viewer:&(String,String),agents:&[(Uuid,String)]) {
    let node=nodes[0];let reserve=nodes[1];
    *mock.revision.lock().unwrap()="r1".into();
    *mock.config.lock().unwrap()=json!({"server":{"listeners":[{"ip":"192.0.2.1","port":8443,"transport":"mtproxy"},{"ip":"2001:db8::1","port":8443,"transport":"mtproxy"}]}});
    mock.records.lock().unwrap().get_mut("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap()["content"]=json!("192.0.2.1");
    mock.records.lock().unwrap().get_mut("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap()["content"]=json!("2001:db8::1");
    sqlx::query("UPDATE nodes SET collected_at=now(),snapshot=$1").bind(json!({"ready":true,"compatibility":{"fingerprint":"a".repeat(64)}})).execute(&state.db).await.unwrap();
    let (status,result,_)=request(app,"/api/groups","POST",json!({"name":"local-first","domain":"proxy.test","zone_id":"0".repeat(32),"record_a_id":"a".repeat(32),"record_aaaa_id":"b".repeat(32),"active_node":node,"node_ids":nodes,"token":"test-dns-token"}),Some(owner),true).await;
    assert_eq!(status,StatusCode::OK,"{result}");let group=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
    let body=json!({"name":"primary-addresses","node_id":node,"group_id":group,"addresses":["192.0.2.1","192.0.2.3","192.0.2.4","2001:db8::1"]});
    assert_eq!(request(app,"/api/ip-pools","POST",body.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    let mut invalid=body.clone();invalid["addresses"]=json!(["192.0.2.1","192.0.2.99"]);
    assert_eq!(request(app,"/api/ip-pools","POST",invalid,Some(owner),true).await.0,StatusCode::CONFLICT);
    let (status,result,_)=request(app,"/api/ip-pools","POST",body,Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{result}");let pool=result["data"]["id"].as_str().unwrap();
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}/prepare"),"POST",json!({"revision":"stale"}),Some(owner),true).await.0,StatusCode::CONFLICT);
    let (status,result,_)=request(app,&format!("/api/ip-pools/{pool}/prepare"),"POST",json!({"revision":"r1"}),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{result}");
    crate::rotation::tick(state).await.unwrap();
    let waiting=sqlx::query("SELECT status,preparation_error,preparation_checked_at IS NOT NULL AS checked FROM ip_pools WHERE id=$1").bind(Uuid::parse_str(pool).unwrap()).fetch_one(&state.db).await.unwrap();
    assert_eq!(waiting.get::<String,_>("status"),"preparing");assert_eq!(waiting.get::<String,_>("preparation_error"),"managed_node_not_ready");assert!(waiting.get::<bool,_>("checked"));
    let deployment=sqlx::query("SELECT d.* FROM deployments d JOIN managed_nodes m ON m.desired_revision=d.id WHERE m.node_id=$1").bind(node).fetch_one(&state.db).await.unwrap();let did:i64=deployment.get("id");
    let raw=state.vault.open(&deployment.get::<String,_>("bundle_cipher"),&format!("deployment:{node}:{did}")).unwrap();let bundle:telemt_managed::Bundle=serde_json::from_str(&raw).unwrap();
    assert_eq!(bundle.public_ips.len(),4);assert!(bundle.nginx.contains("192.0.2.3:443"));assert!(bundle.nginx.contains("X-Telemt-Public-Address"));
    let config:toml::Value=toml::from_str(&bundle.config).unwrap();assert_eq!(config["web"]["vhosts"][0]["public_addrs"].as_array().unwrap().len(),4);
    *mock.config.lock().unwrap()=serde_json::to_value(config).unwrap();
    sqlx::query("UPDATE managed_nodes SET applied_revision=desired_revision,status='applied',last_seen=now() WHERE node_id=$1").bind(node).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{configuration_verified}','false') WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM ip_pools WHERE id=$1").bind(Uuid::parse_str(pool).unwrap()).fetch_one(&state.db).await.unwrap(),"prepared");
    assert!(sqlx::query_scalar::<_,Option<String>>("SELECT preparation_error FROM ip_pools WHERE id=$1").bind(Uuid::parse_str(pool).unwrap()).fetch_one(&state.db).await.unwrap().is_none());
    let (status,result,_)=request(app,&format!("/api/ip-pools/{pool}"),"PATCH",json!({"automatic":true}),Some(owner),true).await;
    assert_eq!(status,StatusCode::CONFLICT);assert_eq!(result["error"]["code"],"rotation_group_or_configuration_not_ready");
    sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{configuration_verified}','true') WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    for ip in ["192.0.2.1","2001:db8::1"]{seed(state,node,agents,ip,false).await;}
    for ip in ["192.0.2.3","192.0.2.4"]{seed(state,node,agents,ip,true).await;}
    for ip in ["192.0.2.2","2001:db8::2"]{seed(state,reserve,agents,ip,true).await;}
    assert_eq!(request(app,&format!("/api/groups/{group}"),"PATCH",json!({"automatic":true}),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(request(app,&format!("/api/ip-pools/{pool}"),"PATCH",json!({"automatic":true}),Some(owner),true).await.0,StatusCode::OK);
    cooldown(state).await;
    crate::dns::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap(),node);
    crate::rotation::tick(state).await.unwrap();crate::rotation::tick(state).await.unwrap();
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.3");
    assert!(mock.config.lock().unwrap()["server"]["listeners"].as_array().unwrap().iter().any(|listener|listener["ip"]=="192.0.2.1"));
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM ip_rotations WHERE pool_id=$1 ORDER BY created_at DESC LIMIT 1").bind(Uuid::parse_str(pool).unwrap()).fetch_one(&state.db).await.unwrap(),"succeeded");
    seed(state,node,agents,"192.0.2.3",false).await;
    sqlx::query("DELETE FROM probe_jobs WHERE node_id=$1 AND address='192.0.2.4'").bind(node).execute(&state.db).await.unwrap();
    cooldown(state).await;
    assert!(!crate::rotation::selection::exhausted(state,node).await.unwrap());
    crate::dns::tick(state).await.unwrap();assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap(),node);
    seed(state,node,agents,"192.0.2.4",false).await;
    assert!(crate::rotation::selection::exhausted(state,node).await.unwrap());
    crate::dns::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap(),reserve);
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.2");
    let (_,status,_)=request(app,&format!("/api/ip-pools/{pool}/status"),"GET",json!({}),Some(owner),false).await;
    assert_eq!(status["data"]["addresses"][0]["probe_port"],22);
    // Recovery of a standby pool must clear the old selection error without moving DNS back.
    for ip in ["192.0.2.1","192.0.2.3","192.0.2.4","2001:db8::1"]{seed(state,node,agents,ip,true).await;}
    sqlx::query("UPDATE ip_pools SET status='no_healthy_ip_reserve' WHERE id=$1").bind(Uuid::parse_str(pool).unwrap()).execute(&state.db).await.unwrap();
    crate::rotation::tick(state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM ip_pools WHERE id=$1").bind(Uuid::parse_str(pool).unwrap()).fetch_one(&state.db).await.unwrap(),"standby");
    assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap(),reserve);
    assert_eq!(mock.records.lock().unwrap()["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"]["content"],"192.0.2.2");
    // Historic protocol reports cannot make port-22 reachability look healthy or failed.
    seed(state,node,agents,"192.0.2.1",true).await;
    sqlx::query("UPDATE probe_jobs SET result=result-'tcp22'-'check' WHERE node_id=$1 AND address='192.0.2.1'").bind(node).execute(&state.db).await.unwrap();
    let (_,status,_)=request(app,&format!("/api/ip-pools/{pool}/status"),"GET",json!({}),Some(owner),false).await;
    assert_eq!(status["data"]["addresses"][0]["health"],"unknown");
    // Deleting the active server preserves the shared group and permits explicit recovery to a prepared pool.
    assert_eq!(request(app,&format!("/api/nodes/{reserve}"),"DELETE",json!({}),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(request(app,&format!("/api/groups/{group}"),"PATCH",json!({"node_ids":[node]}),Some(owner),true).await.0,StatusCode::OK);
    sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{fingerprint}',$2) WHERE id=$1").bind(group).bind(json!("b".repeat(64))).execute(&state.db).await.unwrap();
    assert_eq!(request(app,&format!("/api/groups/{group}/verify-configuration"),"POST",json!({}),Some(owner),true).await.0,StatusCode::CONFLICT);
    assert_eq!(request(app,&format!("/api/groups/{group}/verify-configuration"),"POST",json!({"node_id":node}),Some(owner),true).await.0,StatusCode::OK);
    for ip in ["192.0.2.1","192.0.2.3","192.0.2.4","2001:db8::1"]{seed(state,node,agents,ip,true).await;}
    cooldown(state).await;
    let (_,records,_)=request(app,&format!("/api/groups/{group}/records"),"GET",json!({}),Some(owner),false).await;
    let (status,result,_)=request(app,&format!("/api/groups/{group}/repair"),"POST",json!({"node_id":node,"expected_records":records["data"]}),Some(owner),true).await;
    assert_eq!(status,StatusCode::OK,"{result}");
    assert_eq!(sqlx::query_scalar::<_,Uuid>("SELECT active_node FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap(),node);
    assert!(!sqlx::query_scalar::<_,bool>("SELECT automatic FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap());
}
