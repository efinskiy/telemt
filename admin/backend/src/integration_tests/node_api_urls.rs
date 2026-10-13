use super::*;

pub(super) async fn run(state:&State,app:&Router,owner:&(String,String),viewer:&(String,String),port:u16) {
    for origin in ["http://203.0.113.10:9091/","https://[2001:db8::10]:9443/","https://node-api.example.test/"] {
        let body=json!({"name":"public management","api_url":origin,"token":"Bearer test-node-token","probe_secret":"0123456789abcdef0123456789abcdef","ipv4":"192.0.2.10","port":443,"sni":"example.org"});
        assert_eq!(request(app,"/api/nodes","POST",body.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
        let (status,result,_)=request(app,"/api/nodes","POST",body.clone(),Some(owner),true).await;
        assert_eq!(status,StatusCode::OK,"{result}");let id=Uuid::parse_str(result["data"]["id"].as_str().unwrap()).unwrap();
        let before=sqlx::query("SELECT api_url,token_cipher,probe_cipher FROM nodes WHERE id=$1").bind(id).fetch_one(&state.db).await.unwrap();
        assert_eq!(before.get::<String,_>("api_url"),origin);
        let mut edited=body;edited["api_url"]=json!(format!("http://node-api.example.test:{port}/"));edited["token"]=json!("");edited["probe_secret"]=json!("");
        assert_eq!(request(app,&format!("/api/nodes/{id}"),"PATCH",edited,Some(owner),true).await.0,StatusCode::OK);
        let after=sqlx::query("SELECT token_cipher,probe_cipher FROM nodes WHERE id=$1").bind(id).fetch_one(&state.db).await.unwrap();
        assert_eq!(before.get::<String,_>("token_cipher"),after.get::<String,_>("token_cipher"));
        assert_eq!(before.get::<String,_>("probe_cipher"),after.get::<String,_>("probe_cipher"));
        // Resolve the enrolled hostname to the authenticated local fixture, without external DNS/network.
        let mut connected=state.clone();connected.client=reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).timeout(Duration::from_secs(3)).resolve("node-api.example.test",std::net::SocketAddr::from(([127,0,0,1],port))).build().unwrap();
        let (status,reply)=crate::state::node_request(&connected,id,reqwest::Method::GET,"/v1/health/ready",None,None).await.unwrap();
        assert_eq!(status,200);assert_eq!(reply["data"]["ready"],true);
        assert_eq!(request(app,&format!("/api/nodes/{id}"),"DELETE",json!({}),Some(owner),true).await.0,StatusCode::OK);
    }
}
