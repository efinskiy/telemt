//! Retired direct registration cannot create, edit or adopt a node.
use super::*;
pub(super) async fn run(state:&State,app:&Router,owner:&(String,String),_viewer:&(String,String),port:u16) {
    for origin in [format!("http://127.0.0.1:{port}/"),"https://public.example:9443/".into()] {
        let body=json!({"name":"legacy","api_url":origin,"token":"private","probe_secret":"a".repeat(32),"ipv4":"192.0.2.9","port":443,"sni":"example.org","probe_dc":2});
        assert_eq!(request(app,"/api/nodes","POST",body.clone(),Some(owner),true).await.0,StatusCode::BAD_REQUEST);
        for suffix in ["", "/adopt"] {assert_eq!(request(app,&format!("/api/nodes/{}{suffix}",Uuid::new_v4()),if suffix.is_empty(){"PATCH"}else{"POST"},body.clone(),Some(owner),true).await.0,StatusCode::BAD_REQUEST);}
    }
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM nodes").fetch_one(&state.db).await.unwrap(),0);
}
