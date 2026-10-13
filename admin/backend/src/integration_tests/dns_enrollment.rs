//! Enrollment errors must distinguish node state from invalid form fields without writing DNS.
use super::*;

pub(super) async fn run(state:&State,app:&Router,owner:&(String,String),node:Uuid,mock:&Mock) {
    let body=json!({"name":"enrollment-check","domain":"proxy.test","zone_id":"00000000000000000000000000000000","record_a_id":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","record_aaaa_id":"","active_node":node,"node_ids":[node],"token":"test-dns-token"});
    let snapshot:Value=sqlx::query_scalar("SELECT snapshot FROM nodes WHERE id=$1").bind(node).fetch_one(&state.db).await.unwrap();
    let records=mock.records.lock().unwrap().clone();
    sqlx::query("UPDATE nodes SET collected_at=now()-interval '1 minute' WHERE id=$1").bind(node).execute(&state.db).await.unwrap();
    let (status,response,_)=request(app,"/api/groups","POST",body.clone(),Some(owner),true).await;
    assert_eq!(status,StatusCode::CONFLICT);
    assert_eq!(response["error"]["code"],"dns_active_node_stale");

    sqlx::query("UPDATE nodes SET collected_at=now(),snapshot=snapshot-'compatibility' WHERE id=$1").bind(node).execute(&state.db).await.unwrap();
    let (status,response,_)=request(app,"/api/groups","POST",body.clone(),Some(owner),true).await;
    assert_eq!(status,StatusCode::CONFLICT);
    assert_eq!(response["error"]["code"],"dns_active_node_compatibility_unavailable");
    sqlx::query("UPDATE nodes SET snapshot=$2 WHERE id=$1").bind(node).bind(snapshot).execute(&state.db).await.unwrap();

    for (record,code) in [("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","dns_active_ipv4_mismatch"),("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","dns_active_ipv6_mismatch")] {
        mock.records.lock().unwrap().get_mut(record).unwrap()["content"]=json!(if code.contains("ipv4"){"192.0.2.99"}else{"2001:db8::99"});
        let mut both=body.clone();both["record_aaaa_id"]=json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        let (status,response,_)=request(app,"/api/groups","POST",both,Some(owner),true).await;
        assert_eq!(status,StatusCode::CONFLICT);assert_eq!(response["error"]["code"],code);
        *mock.records.lock().unwrap()=records.clone();
    }
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM failover_groups").fetch_one(&state.db).await.unwrap(),0);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM dns_changes").fetch_one(&state.db).await.unwrap(),0);

    // IPv4-only enrollment must remain valid with an explicitly empty AAAA field.
    let (status,response,_)=request(app,"/api/groups","POST",body,Some(owner),true).await;
    assert_eq!(status,StatusCode::OK);
    assert_eq!(*mock.records.lock().unwrap(),records);
    let group=Uuid::parse_str(response["data"]["id"].as_str().unwrap()).unwrap();
    sqlx::query("DELETE FROM failover_groups WHERE id=$1").bind(group).execute(&state.db).await.unwrap();
}
