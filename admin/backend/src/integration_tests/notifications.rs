//! Focused settings security and real outbox delivery checks using a local Telegram mock.
use super::*;
const TOKEN:&str="123456:abcdefghijklmnopqrstuvwxyzABCDE";
pub(super) async fn setup(state:&State,app:&Router,owner:&(String,String),viewer:&(String,String)) {
    for path in ["/api/notifications","/api/notifications/history"] {
        assert_eq!(request(app,path,"GET",json!({}),Some(viewer),false).await.0,StatusCode::FORBIDDEN);
    }
    let body=json!({"revision":1,"enabled":true,"chat_id":"-1001234567890","bot_token":TOKEN,"ip_rotation":true,"server_switch":true});
    assert_eq!(request(app,"/api/notifications","PUT",body.clone(),Some(owner),false).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,"/api/notifications","PUT",body.clone(),Some(viewer),true).await.0,StatusCode::FORBIDDEN);
    let (status,response,_)=request(app,"/api/notifications","PUT",body.clone(),Some(owner),true).await;
    assert_eq!(status,StatusCode::OK,"{response}");assert_eq!(response["data"]["token_configured"],true);assert!(!response.to_string().contains(TOKEN));
    assert_eq!(request(app,"/api/notifications","PUT",body.clone(),Some(owner),true).await.0,StatusCode::CONFLICT);
    let mut preserved=body;preserved["revision"]=json!(2);preserved["bot_token"]=json!("");
    assert_eq!(request(app,"/api/notifications","PUT",preserved,Some(owner),true).await.0,StatusCode::OK);
    let cipher:String=sqlx::query_scalar("SELECT token_cipher FROM notification_settings").fetch_one(&state.db).await.unwrap();
    assert!(!cipher.contains(TOKEN));assert_eq!(&*state.vault.open(&cipher,"telegram-notifications").unwrap(),TOKEN);
}
pub(super) async fn confirmed_only(state:&State) {
    let premature:i64=sqlx::query_scalar("SELECT count(*) FROM notification_deliveries d JOIN dns_changes c ON c.id=d.dns_change WHERE c.state<>'verified'").fetch_one(&state.db).await.unwrap();assert_eq!(premature,0);
}
pub(super) async fn delivery(state:&State,app:&Router,owner:&(String,String)) {
    for kind in ["server_switch","ip_rotation"] {
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM notification_deliveries WHERE kind=$1").bind(kind).fetch_one(&state.db).await.unwrap();assert!(count>0,"missing {kind} event");
    }
    let requests=Arc::new(std::sync::atomic::AtomicUsize::new(0));let captured=requests.clone();
    let mock=Router::new().route(&format!("/bot{TOKEN}/sendMessage"),post(move|Json(body):Json<Value>|{let captured=captured.clone();async move{
        assert_eq!(body["chat_id"],"-1001234567890");assert!(body["text"].as_str().unwrap().contains("Telemt Fleet"));assert!(body["text"].as_str().unwrap().encode_utf16().count()<=4096);
        if captured.fetch_add(1,std::sync::atomic::Ordering::SeqCst)==0 {(StatusCode::TOO_MANY_REQUESTS,Json(json!({"ok":false,"error_code":429,"parameters":{"retry_after":90}})))}else{(StatusCode::OK,Json(json!({"ok":true,"result":{"message_id":1}})))}
    }}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();
    let server=tokio::spawn(async move{axum::serve(listener,mock).await.unwrap();});
    let mut sending=state.clone();sending.telegram_origin=Some(format!("http://{addr}").into());
    crate::notifications::tick(&sending).await.unwrap();
    let retry=sqlx::query("SELECT id,status,error,extract(epoch FROM next_attempt-now()) AS delay FROM notification_deliveries WHERE attempts=1").fetch_one(&state.db).await.unwrap();
    assert_eq!(retry.get::<String,_>("status"),"queued");assert_eq!(retry.get::<String,_>("error"),"telegram_rate_limited");
    let id:Uuid=retry.get("id");
    let delay:bool=sqlx::query_scalar("SELECT next_attempt>now()+interval '80 seconds' FROM notification_deliveries WHERE id=$1").bind(id).fetch_one(&state.db).await.unwrap();assert!(delay);
    sqlx::query("UPDATE notification_deliveries SET next_attempt=now()-interval '1 minute' WHERE id=$1").bind(id).execute(&state.db).await.unwrap();
    crate::notifications::tick(&sending).await.unwrap();
    let sent:String=sqlx::query_scalar("SELECT status FROM notification_deliveries WHERE id=$1").bind(id).fetch_one(&state.db).await.unwrap();assert_eq!(sent,"sent");
    let (_,settings,_)=request(app,"/api/notifications","GET",json!({}),Some(owner),false).await;
    let mut body=settings["data"].clone();body["enabled"]=json!(false);
    assert_eq!(request(app,"/api/notifications","PUT",body,Some(owner),true).await.0,StatusCode::OK);
    let pending:i64=sqlx::query_scalar("SELECT count(*) FROM notification_deliveries WHERE status='queued'").fetch_one(&state.db).await.unwrap();assert_eq!(pending,0);
    assert_eq!(request(app,"/api/notifications/test","POST",json!({}),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(request(app,"/api/notifications/test","POST",json!({}),Some(owner),true).await.0,StatusCode::TOO_MANY_REQUESTS);
    crate::notifications::tick(&sending).await.unwrap();
    let test_sent:bool=sqlx::query_scalar("SELECT status='sent' FROM notification_deliveries WHERE kind='test'").fetch_one(&state.db).await.unwrap();assert!(test_sent);
    server.abort();let _=server.await;
}
