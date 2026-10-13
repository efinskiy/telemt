//! Authenticated map requests exercise the real Fleet router and registered-node client.
use super::*;

pub(super) async fn run(state: &State, app: &Router, owner: &(String,String), viewer: &(String,String), id: Uuid) {
    let path = format!("/api/nodes/{id}/connections-map");
    assert_eq!(request(app,&path,"GET",json!({}),None,false).await.0,StatusCode::UNAUTHORIZED);
    auth::create(state,"map-operator","long-test-password-only","operator").await.unwrap();
    let operator = login(state,app,"map-operator").await;
    for session in [owner, viewer, &operator] {
        let (status,response,headers) = request(app,&path,"GET",json!({}),Some(session),false).await;
        assert_eq!(status,StatusCode::OK);
        assert_eq!(headers.get("cache-control").unwrap(),"no-store");
        assert_eq!(response["data"]["available"],true);
        assert_eq!(response["data"]["connections"],5);
        assert_eq!(response["data"]["active_users"],2);
        assert_eq!(response["data"]["database"]["status"],"missing");
        assert!(response["data"]["ips"][0]["location"].is_null());
        let serialized = response.to_string();
        assert!(!serialized.contains("test-node-token"));
        assert!(!serialized.contains("secret"));
    }
    sqlx::query("UPDATE nodes SET enabled=false WHERE id=$1").bind(id).execute(&state.db).await.unwrap();
    let (_,response,_) = request(app,&path,"GET",json!({}),Some(viewer),false).await;
    assert_eq!(response["data"]["reason"],"node_disabled");
    sqlx::query("UPDATE nodes SET enabled=true WHERE id=$1").bind(id).execute(&state.db).await.unwrap();
}
