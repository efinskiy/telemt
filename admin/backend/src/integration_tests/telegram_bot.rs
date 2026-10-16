//! One local end-to-end bot scenario covers access, enrollment confirmation and replay fencing.
use super::*;
#[derive(Clone,Default)]
struct Telegram {
    updates:Arc<std::sync::Mutex<Vec<Value>>>,
    sent:Arc<std::sync::Mutex<Vec<(String,Value)>>>,
    files:Arc<std::sync::Mutex<Vec<String>>>,
    edit_error:Arc<std::sync::Mutex<Option<Value>>>,
    message_id:Arc<std::sync::Mutex<i64>>,
}
async fn telegram(ExtractState(mock):ExtractState<Telegram>,req:axum::extract::Request)->Json<Value> {
    let method=req.uri().path().rsplit('/').next().unwrap().to_owned();
    let bytes=to_bytes(req.into_body(),512*1024).await.unwrap();
    if method=="sendDocument"{mock.files.lock().unwrap().push(String::from_utf8(bytes.to_vec()).unwrap());return Json(json!({"ok":true,"result":{"message_id":1}}));}
    let body:Value=serde_json::from_slice(&bytes).unwrap();mock.sent.lock().unwrap().push((method.clone(),body.clone()));
    if method=="editMessageText"&&let Some(error)=mock.edit_error.lock().unwrap().take(){return Json(error);}
    let result=match method.as_str(){
        "getMe"=>json!({"id":999,"is_bot":true,"username":"fixture_bot"}),
        "getWebhookInfo"=>json!({"url":""}),
        "getUpdates"=>{let mut updates=mock.updates.lock().unwrap();let offset=body["offset"].as_i64().unwrap();updates.retain(|u|offset<0||u["update_id"].as_i64().unwrap()>=offset);json!(updates.iter().take(1).cloned().collect::<Vec<_>>())},
        "sendRichMessage"|"sendMessage"=>{if method=="sendRichMessage"{assert!(body["rich_message"]["blocks"].is_array());}assert_eq!(body["protect_content"],true);let mut id=mock.message_id.lock().unwrap();*id+=1;json!({"message_id":*id})},
        "editMessageText"=>{assert_eq!(body["message_id"],*mock.message_id.lock().unwrap());assert!(body["text"].is_string()||body["rich_message"]["blocks"].is_array());assert!(body["reply_markup"].get("force_reply").is_none());json!({"message_id":body["message_id"]})},
        _=>json!(true),
    };Json(json!({"ok":true,"result":result}))
}
async fn send(state:&State,mock:&Telegram,seq:&mut i64,sender:i64,text:&str,callback:bool) {
    *seq+=1;let from=json!({"id":sender,"is_bot":false});let chat=json!({"id":sender,"type":"private"});
    let mut update=json!({"update_id":*seq});
    if callback{update["callback_query"]=json!({"id":seq.to_string(),"from":from,"message":{"chat":chat,"message_id":*mock.message_id.lock().unwrap()},"data":text});}
    else{update["message"]=json!({"chat":chat,"from":from,"date":chrono::Utc::now().timestamp(),"text":text});}
    mock.updates.lock().unwrap().push(update);crate::telegram_bot::tick(state).await.unwrap();
    let error:Option<String>=sqlx::query_scalar("SELECT last_error FROM telegram_bot_settings").fetch_one(&state.db).await.unwrap();assert_eq!(error,None);
}
async fn pending(state:&State)->Uuid {sqlx::query_scalar("SELECT id FROM telegram_bot_actions WHERE status='pending'").fetch_one(&state.db).await.unwrap()}
async fn wizard(state:&State,mock:&Telegram,seq:&mut i64,profile:Uuid) {
    send(state,mock,seq,101,&format!("profile:{profile}"),true).await;
    send(state,mock,seq,101,"bot-enrollment",false).await;
    send(state,mock,seq,101,"192.0.2.50",false).await;
}
pub(super) async fn run(state:&State,app:&Router,owner:&(String,String),viewer:&(String,String),profile:Uuid) {
    let mock=Telegram::default();let service=Router::new().route("/{*path}",post(telegram)).with_state(mock.clone());
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let addr=listener.local_addr().unwrap();let server=tokio::spawn(async move{axum::serve(listener,service).await.unwrap();});
    let mut state=state.clone();state.telegram_origin=Some(format!("http://{addr}").into());
    let admin:Uuid=sqlx::query_scalar("SELECT id FROM administrators WHERE username='owner'").fetch_one(&state.db).await.unwrap();
    let (_,initial,_)=request(app,"/api/telegram-bot","GET",json!({}),Some(owner),false).await;
    let body=json!({"revision":initial["data"]["revision"],"enabled":true,"bindings":[{"telegram_id":"101","administrator_id":admin}]});
    assert_eq!(request(app,"/api/telegram-bot","GET",json!({}),Some(viewer),false).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,"/api/telegram-bot","PUT",body.clone(),Some(owner),false).await.0,StatusCode::FORBIDDEN);
    assert_eq!(request(app,"/api/telegram-bot","PUT",body.clone(),Some(owner),true).await.0,StatusCode::OK);
    assert_eq!(request(app,"/api/telegram-bot","PUT",body,Some(owner),true).await.0,StatusCode::CONFLICT);
    crate::telegram_bot::tick(&state).await.unwrap();
    let mut seq=0;send(&state,&mock,&mut seq,101,"/status",false).await;
    assert!(mock.sent.lock().unwrap().iter().any(|(method,body)|method=="sendRichMessage"&&body["rich_message"]["blocks"].as_array().unwrap().iter().any(|b|b["type"]=="table"&&b["is_compact"]==true)));
    send(&state,&mock,&mut seq,202,"/add",false).await;
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM telegram_bot_sessions").fetch_one(&state.db).await.unwrap(),0);
    wizard(&state,&mock,&mut seq,profile).await;let cancelled=pending(&state).await;
    send(&state,&mock,&mut seq,101,"/cancel",false).await;
    send(&state,&mock,&mut seq,101,&format!("confirm:{cancelled}"),true).await;
    assert_eq!(sqlx::query_scalar::<_,String>("SELECT status FROM telegram_bot_actions WHERE id=$1").bind(cancelled).fetch_one(&state.db).await.unwrap(),"cancelled");
    wizard(&state,&mock,&mut seq,profile).await;let action=pending(&state).await;
    send(&state,&mock,&mut seq,202,&format!("confirm:{action}"),true).await;
    assert_eq!(pending(&state).await,action);
    sqlx::query("UPDATE administrators SET role='viewer' WHERE id=$1").bind(admin).execute(&state.db).await.unwrap();
    send(&state,&mock,&mut seq,101,&format!("confirm:{action}"),true).await;assert_eq!(pending(&state).await,action);
    sqlx::query("UPDATE administrators SET role='owner' WHERE id=$1").bind(admin).execute(&state.db).await.unwrap();
    send(&state,&mock,&mut seq,101,&format!("confirm:{action}"),true).await;
    let row=sqlx::query("SELECT status,result,artifact_cipher FROM telegram_bot_actions WHERE id=$1").bind(action).fetch_one(&state.db).await.unwrap();
    assert_eq!(row.get::<String,_>("status"),"done","{}",row.get::<Value,_>("result"));
    assert!(row.get::<Value,_>("result").get("compose").is_none());assert!(!row.get::<String,_>("artifact_cipher").contains("ENROLLMENT_TOKEN"));
    assert!(mock.files.lock().unwrap()[0].contains("ENROLLMENT_TOKEN:"));
    send(&state,&mock,&mut seq,101,&format!("confirm:{action}"),true).await;
    assert_eq!(mock.files.lock().unwrap().len(),1);
    assert_eq!(sqlx::query_scalar::<_,i64>("SELECT count(*) FROM nodes WHERE name='bot-enrollment'").fetch_one(&state.db).await.unwrap(),1);
    send(&state,&mock,&mut seq,101,&format!("file:{action}"),true).await;assert_eq!(mock.files.lock().unwrap().len(),2);
    // Buttons, free-text wizard input, confirmations and errors all reuse the original menu.
    assert_eq!(*mock.message_id.lock().unwrap(),1);
    {let sent=mock.sent.lock().unwrap();assert!(sent.iter().any(|(method,body)|method=="editMessageText"&&body["text"].is_string()));assert!(sent.iter().any(|(method,body)|method=="editMessageText"&&body["rich_message"].is_object()));}
    *mock.edit_error.lock().unwrap()=Some(json!({"ok":false,"error_code":400,"description":"Bad Request: message is not modified: specified new message content and reply markup are exactly the same"}));
    send(&state,&mock,&mut seq,101,"/nodes",false).await;assert_eq!(*mock.message_id.lock().unwrap(),1);
    *mock.edit_error.lock().unwrap()=Some(json!({"ok":false,"error_code":400,"description":"Bad Request: message to edit not found"}));
    send(&state,&mock,&mut seq,101,"/nodes",false).await;assert_eq!(*mock.message_id.lock().unwrap(),2);
    send(&state,&mock,&mut seq,101,"/status",false).await;assert_eq!(*mock.message_id.lock().unwrap(),2);
    assert_eq!(sqlx::query_scalar::<_,Option<i64>>("SELECT menu_message_id FROM telegram_bot_access WHERE telegram_id=101").fetch_one(&state.db).await.unwrap(),Some(2));
    // An ambiguous provider failure must not create an error message or retry the write.
    let edits=mock.sent.lock().unwrap().iter().filter(|(method,_)|method=="editMessageText").count();
    *mock.edit_error.lock().unwrap()=Some(json!({"ok":false,"error_code":500,"description":"Internal Server Error"}));
    seq+=1;mock.updates.lock().unwrap().push(json!({"update_id":seq,"message":{"chat":{"id":101,"type":"private"},"from":{"id":101,"is_bot":false},"date":chrono::Utc::now().timestamp(),"text":"/nodes"}}));
    crate::telegram_bot::tick(&state).await.unwrap();
    assert_eq!(sqlx::query_scalar::<_,Option<String>>("SELECT last_error FROM telegram_bot_settings").fetch_one(&state.db).await.unwrap().as_deref(),Some("telegram_api_error"));
    assert_eq!(*mock.message_id.lock().unwrap(),2);
    assert_eq!(mock.sent.lock().unwrap().iter().filter(|(method,_)|method=="editMessageText").count(),edits+1);
    // A reviewed profile revision must not silently change before enrollment.
    let invalid=json!({"name":"stale-bot-profile","ip":"192.0.2.51","profile_id":profile,"expected_profile_revision":0});
    let (status,error,_)=request(app,"/api/managed-nodes","POST",invalid,Some(owner),true).await;assert_eq!(status,StatusCode::CONFLICT);assert_eq!(error["error"]["code"],"telegram_profile_changed");
    let node:Uuid=sqlx::query_scalar("SELECT id FROM nodes WHERE name='bot-enrollment'").fetch_one(&state.db).await.unwrap();
    assert_eq!(request(app,&format!("/api/nodes/{node}"),"DELETE",json!({}),Some(owner),true).await.0,StatusCode::OK);
    let (_,current,_)=request(app,"/api/telegram-bot","GET",json!({}),Some(owner),false).await;
    assert_eq!(request(app,"/api/telegram-bot","PUT",json!({"revision":current["data"]["revision"],"enabled":false,"bindings":[]}),Some(owner),true).await.0,StatusCode::OK);
    server.abort();let _=server.await;
}
