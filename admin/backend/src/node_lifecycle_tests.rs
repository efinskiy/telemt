//! PostgreSQL regressions for deletion of active, shared and legacy dependencies.
use super::*;
use crate::{crypto::Vault, state::State};
use std::sync::Arc;
use sqlx::postgres::PgPoolOptions;

#[tokio::test]
#[ignore = "requires TELEMT_TEST_DATABASE_URL pointing to disposable PostgreSQL"]
async fn cascades_dependencies_without_deleting_shared_resources() {
    let url=std::env::var("TELEMT_TEST_DATABASE_URL").unwrap();
    let root=PgPoolOptions::new().connect(&url).await.unwrap();
    let schema=format!("cascade_{}",Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&root).await.unwrap();
    let search=schema.clone();
    let db=PgPoolOptions::new().max_connections(8).after_connect(move|conn,_|{
        let search=search.clone();Box::pin(async move {sqlx::query(&format!("SET search_path TO {search}")).execute(conn).await?;Ok(())})
    }).connect(&url).await.unwrap();
    sqlx::migrate!().run(&db).await.unwrap();
    let state=State{db, vault:Arc::new(Vault::new(&[17;32]).unwrap()), client:reqwest::Client::new(), geoip:Arc::new(crate::geoip::GeoIp::unavailable("missing")), hash_slots:Arc::new(tokio::sync::Semaphore::new(1)),cloudflare_origin:None,probe_round_override:None,origin:"https://fleet.test".into(),assets:Arc::new("/tmp/missing".into())};
    let owner=Session{id:Uuid::new_v4(),username:"owner".into(),role:"owner".into()};
    sqlx::query("INSERT INTO administrators(id,username,password_hash,totp_cipher,role) VALUES($1,'owner','','','owner')").bind(owner.id).execute(&state.db).await.unwrap();
    let cert=Uuid::new_v4();let profile=Uuid::new_v4();let a=Uuid::new_v4();let b=Uuid::new_v4();let group=Uuid::new_v4();let pool=Uuid::new_v4();
    sqlx::query("INSERT INTO certificates(id,name,domain) VALUES($1,'shared','proxy.test')").bind(cert).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO profiles(id,name,domain,certificate_id,draft_cipher,probe_cipher) VALUES($1,'shared','proxy.test',$2,'','')").bind(profile).bind(cert).execute(&state.db).await.unwrap();
    for (id,name) in [(a,"a"),(b,"b")] {
        sqlx::query("INSERT INTO nodes(id,name,token_cipher,probe_cipher,document) VALUES($1,$2,'','',$3)").bind(id).bind(name).bind(json!({"managed":true,"profile_id":profile})).execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO managed_nodes(node_id,profile_id,credential_hash) VALUES($1,$2,'credential')").bind(id).bind(profile).execute(&state.db).await.unwrap();
    }
    sqlx::query("INSERT INTO failover_groups(id,name,document,token_cipher,active_node,automatic) VALUES($1,'shared',$2,'',$3,true)").bind(group).bind(json!({"node_ids":[a,b],"configuration_verified":true,"profile_id":profile})).bind(a).execute(&state.db).await.unwrap();
    sqlx::query("UPDATE profiles SET group_id=$1,resume_automatic=true WHERE id=$2").bind(group).bind(profile).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO ip_pools(id,node_id,group_id,owner_id,name,addresses) VALUES($1,$2,$3,$4,'pool','[\"192.0.2.1\",\"192.0.2.2\"]')").bind(pool).bind(a).bind(group).bind(owner.id).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO ip_rotations(id,pool_id,old_ip,new_ip,family,state,reason) VALUES($1,$2,'192.0.2.1','192.0.2.2',4,'unknown','test')").bind(Uuid::new_v4()).bind(pool).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO dns_changes(id,group_id,old_node,new_node,intent,state) VALUES($1,$2,$3,$4,'{}','unknown'),($5,$2,$4,$3,'{}','verified')").bind(Uuid::new_v4()).bind(group).bind(a).bind(b).bind(Uuid::new_v4()).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO node_commands(id,node_id,request_cipher,expires_at) VALUES($1,$2,'',now()+interval '1 minute')").bind(Uuid::new_v4()).bind(a).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO deployments(node_id,profile_revision,certificate_revision,bundle_cipher,digest) VALUES($1,1,1,'','')").bind(a).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO node_history(node_id,snapshot) VALUES($1,'{}')").bind(a).execute(&state.db).await.unwrap();
    sqlx::query("INSERT INTO minute_history(node_id,minute,snapshot) VALUES($1,now(),'{}')").bind(a).execute(&state.db).await.unwrap();
    let mixed=Uuid::new_v4();let single=Uuid::new_v4();
    for (id,targets) in [(mixed,json!([a,b])),(single,json!([a]))] {
        let intent=json!({"node_ids":targets,"method":"POST","path":"/v1/runtime/web/lifecycle/pause","revisions":{a.to_string():"a",b.to_string():"b"}});
        let cipher=state.vault.seal(&intent.to_string(),&format!("operation:{id}")).unwrap();
        sqlx::query("INSERT INTO operations(id,actor,state,request_cipher,results) VALUES($1,$2,'running',$3,$4)").bind(id).bind(owner.id).bind(cipher).bind(json!([{"node_id":a,"state":"unknown"},{"node_id":b,"state":"succeeded"}])).execute(&state.db).await.unwrap();
    }
    let viewer=Session{role:"viewer".into(),..owner.clone()};
    assert_eq!(remove(&state,&viewer,a).await.unwrap_err().0,axum::http::StatusCode::FORBIDDEN);
    remove(&state,&owner,a).await.unwrap();remove(&state,&owner,a).await.unwrap();
    for table in ["node_history","minute_history","managed_nodes","deployments","node_commands"] {
        assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT count(*) FROM {table} WHERE node_id=$1")).bind(a).fetch_one(&state.db).await.unwrap(),0);
    }
    for table in ["ip_pools","ip_rotations","dns_changes"] {assert_eq!(sqlx::query_scalar::<_,i64>(&format!("SELECT count(*) FROM {table}")).fetch_one(&state.db).await.unwrap(),0);}
    let row=sqlx::query("SELECT active_node,automatic,document,status FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await.unwrap();
    assert!(row.get::<Option<Uuid>,_>("active_node").is_none());assert!(!row.get::<bool,_>("automatic"));assert_eq!(row.get::<Value,_>("document")["node_ids"],json!([b]));assert_eq!(row.get::<String,_>("status"),"node_removed");
    assert!(!sqlx::query_scalar::<_,bool>("SELECT resume_automatic FROM profiles WHERE id=$1").bind(profile).fetch_one(&state.db).await.unwrap());
    let cipher:String=sqlx::query_scalar("SELECT request_cipher FROM operations WHERE id=$1").bind(mixed).fetch_one(&state.db).await.unwrap();
    let intent:Value=serde_json::from_str(&state.vault.open(&cipher,&format!("operation:{mixed}")).unwrap()).unwrap();
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT state FROM operations WHERE id=$1").bind(mixed).fetch_one(&state.db).await.unwrap(),"succeeded");
    assert_eq!(intent["node_ids"],json!([b]));assert!(intent["revisions"].get(a.to_string()).is_none());
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM operations WHERE id=$1").bind(single).fetch_one(&state.db).await.unwrap(),0);
    remove(&state,&owner,b).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM nodes").fetch_one(&state.db).await.unwrap(),0);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM profiles").fetch_one(&state.db).await.unwrap(),1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM certificates").fetch_one(&state.db).await.unwrap(),1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM audit WHERE action='node.delete'").fetch_one(&state.db).await.unwrap(),2);
    state.db.close().await;sqlx::query(&format!("DROP SCHEMA {schema} CASCADE")).execute(&root).await.unwrap();root.close().await;
}
