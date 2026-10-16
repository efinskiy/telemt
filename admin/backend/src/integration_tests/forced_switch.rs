//! Forced selection bypasses health policy while retaining preview and write fences.
use super::*;

pub(super) async fn run(state:&State,app:&Router,mock:&Mock,nodes:&[Uuid],owner:&(String,String),viewer:&(String,String)) {
    let records=mock.records.lock().unwrap().clone();
    mock.records.lock().unwrap().get_mut(&"a".repeat(32)).unwrap()["content"]=json!("192.0.2.1");
    mock.records.lock().unwrap().get_mut(&"b".repeat(32)).unwrap()["content"]=json!("2001:db8::1");
    sqlx::query("UPDATE nodes SET collected_at=now(),snapshot=$1").bind(json!({"ready":true,"compatibility":{"fingerprint":"a".repeat(64)}})).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE managed_nodes SET last_seen=now()").execute(&state.db).await.unwrap();
    sqlx::query("DELETE FROM probe_jobs").execute(&state.db).await.unwrap();
    let (status,response,_)=request(app,"/api/groups","POST",json!({"name":"forced","domain":"proxy.test","zone_id":"0".repeat(32),"record_a_id":"a".repeat(32),"record_aaaa_id":"b".repeat(32),"active_node":nodes[0],"node_ids":nodes,"token":"test-dns-token"}),Some(owner),true).await;
    assert_eq!(status,StatusCode::OK,"{response}");let group=Uuid::parse_str(response["data"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("UPDATE failover_groups SET automatic=true,last_switch=now() WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE profiles SET group_id=$1,resume_automatic=true WHERE id=(SELECT profile_id FROM managed_nodes WHERE node_id=$2)").bind(group).bind(nodes[0]).execute(&state.db).await.unwrap();
    let pool=Uuid::new_v4();
    sqlx::query("INSERT INTO ip_pools(id,node_id,group_id,owner_id,name,addresses,automatic) VALUES($1,$2,$3,(SELECT id FROM administrators WHERE username='owner'),'forced-source','[\"192.0.2.1\",\"192.0.2.3\"]',true)").bind(pool).bind(nodes[0]).bind(group).execute(&state.db).await.unwrap();
    let preview_url=format!("/api/groups/{group}/force-preview");let switch_url=format!("/api/groups/{group}/force-switch");let target=json!({"node_id":nodes[1]});
    assert_eq!(request(app,&preview_url,"POST",target.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,&switch_url,"POST",target.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,&switch_url,"POST",target.clone(),Some(owner),false).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,&preview_url,"POST",json!({"node_id":nodes[0]}),Some(owner),true).await.0,StatusCode::CONFLICT);
    let (status,response,_)=request(app,&preview_url,"POST",target.clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{response}");
    let body=json!({"node_id":nodes[1],"preview":response["data"]});
    assert_eq!(body["preview"]["intent"]["A"]["content"],"192.0.2.2");assert_eq!(body["preview"]["intent"]["AAAA"]["content"],"2001:db8::2");
    let mut stale=body.clone();stale["preview"]["deployment"]=json!(-1);
    assert_eq!(request(app,&switch_url,"POST",stale,Some(owner),true).await.0,StatusCode::CONFLICT);
    // Force ignores changes in runtime synchronization after the owner previews the DNS intent.
    sqlx::query("UPDATE nodes SET enabled=false,collected_at=NULL,snapshot='{}'").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE managed_nodes SET status='pending',last_seen=NULL").execute(&state.db).await.unwrap();
    sqlx::query("UPDATE failover_groups SET document=jsonb_set(document,'{configuration_verified}','false') WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
    let pending=Uuid::new_v4();
    sqlx::query("INSERT INTO dns_changes(id,group_id,new_node,intent,state) VALUES($1,$2,$3,'{}','unknown')").bind(pending).bind(group).bind(nodes[1]).execute(&state.db).await.unwrap();
    assert_eq!(request(app,&switch_url,"POST",body.clone(),Some(owner),true).await.0,StatusCode::CONFLICT);
    sqlx::query("DELETE FROM dns_changes WHERE id=$1").bind(pending).execute(&state.db).await.unwrap();
    mock.records.lock().unwrap().get_mut(&"a".repeat(32)).unwrap()["content"]=json!("192.0.2.99");
    assert_eq!(request(app,&switch_url,"POST",body.clone(),Some(owner),true).await.0,StatusCode::CONFLICT);
    mock.records.lock().unwrap().get_mut(&"a".repeat(32)).unwrap()["content"]=json!("192.0.2.1");
    let (status,response,_)=request(app,&switch_url,"POST",body.clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{response}");
    let row=sqlx::query("SELECT active_node,automatic FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap();
    assert_eq!(row.get::<Uuid,_>("active_node"),nodes[1]);assert!(!row.get::<bool,_>("automatic"));
    assert!(!sqlx::query_scalar::<_,bool>("SELECT resume_automatic FROM profiles WHERE group_id=$1").bind(group).fetch_one(&state.db).await.unwrap());
    assert!(!sqlx::query_scalar::<_,bool>("SELECT automatic FROM ip_pools WHERE id=$1").bind(pool).fetch_one(&state.db).await.unwrap());
    assert_eq!(mock.records.lock().unwrap()[&"a".repeat(32)]["content"],"192.0.2.2");assert_eq!(mock.records.lock().unwrap()[&"b".repeat(32)]["content"],"2001:db8::2");
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM audit WHERE action='dns.force_submit'").fetch_one(&state.db).await.unwrap(),1);
    assert_eq!(request(app,&switch_url,"POST",body,Some(owner),true).await.0,StatusCode::CONFLICT);
    // Replace an offline pool member without touching DNS, credentials or its shared profile.
    let url=format!("/api/nodes/{}/addresses",nodes[0]);
    let edit=json!({"ipv4":"192.0.2.10","ipv6":"2001:db8::1","expected_addresses":{"ipv4":"192.0.2.1","ipv6":"2001:db8::1"}});
    assert_eq!(request(app,&url,"PATCH",edit.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    let (status,response,_)=request(app,&url,"PATCH",edit.clone(),Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{response}");
    assert_eq!(request(app,&url,"PATCH",edit,Some(owner),true).await.0,StatusCode::CONFLICT);
    let ips:Value=sqlx::query_scalar("SELECT addresses FROM ip_pools WHERE id=$1").bind(pool).fetch_one(&state.db).await.unwrap();
    assert!(ips.as_array().unwrap().contains(&json!("192.0.2.10")));assert!(!ips.as_array().unwrap().contains(&json!("192.0.2.1")));
    let did=response["data"]["deployment"].as_i64().unwrap();
    let cipher:String=sqlx::query_scalar("SELECT bundle_cipher FROM deployments WHERE id=$1").bind(did).fetch_one(&state.db).await.unwrap();
    let raw=state.vault.open(&cipher,&format!("deployment:{}:{did}",nodes[0])).unwrap();let bundle:telemt_managed::Bundle=serde_json::from_str(&raw).unwrap();
    assert_eq!(bundle.public_ip,"192.0.2.10");assert!(bundle.nginx.contains("192.0.2.10:443"));assert!(!bundle.public_ips.contains(&"192.0.2.1".into()));
    assert_eq!(mock.records.lock().unwrap()[&"a".repeat(32)]["content"],"192.0.2.2");
    let edit=json!({"ipv4":"192.0.2.20","ipv6":"2001:db8::2","expected_addresses":{"ipv4":"192.0.2.2","ipv6":"2001:db8::2"}});
    assert_eq!(request(app,&format!("/api/nodes/{}/addresses",nodes[1]),"PATCH",edit,Some(owner),true).await.0,StatusCode::OK);
    let (status,response,_)=request(app,&preview_url,"POST",target,Some(owner),true).await;assert_eq!(status,StatusCode::OK,"{response}");
    assert_eq!(request(app,&switch_url,"POST",json!({"node_id":nodes[1],"preview":response["data"]}),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(mock.records.lock().unwrap()[&"a".repeat(32)]["content"],"192.0.2.20");
    for (index,node) in nodes.iter().enumerate(){sqlx::query("UPDATE nodes SET document=jsonb_set(document,'{ipv4}',$2),enabled=true WHERE id=$1").bind(node).bind(json!(format!("192.0.2.{}",index+1))).execute(&state.db).await.unwrap();}
    sqlx::query("UPDATE managed_nodes SET desired_revision=applied_revision,status='applied',last_seen=now()").execute(&state.db).await.unwrap();
    sqlx::query("DELETE FROM ip_pools WHERE id=$1").bind(pool).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE profiles SET group_id=NULL WHERE group_id=$1").bind(group).execute(&state.db).await.unwrap();
    assert_eq!(request(app,&format!("/api/groups/{group}"),"DELETE",json!({}),Some(owner),true).await.0,StatusCode::OK);
    sqlx::query("DELETE FROM dns_record_cooldown").execute(&state.db).await.unwrap();
    *mock.records.lock().unwrap()=records;
}
