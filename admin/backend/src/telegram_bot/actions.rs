//! One-use, sender-bound confirmations delegate mutations to the same handlers as the web cabinet.
use super::*;
use axum::http::Method;
use transport::{button,clean,message};

/// Persist a sender-bound preview before showing its one-use confirmation.
pub(super) async fn prepare(state:&State,token:&str,user:&Session,telegram:i64,revision:i64,kind:&str,payload:Value,description:&str)->Result<()> {
    user.require("owner")?;let id=Uuid::new_v4();
    let mut tx=state.db.begin().await?;
    sqlx::query("UPDATE telegram_bot_actions SET status='cancelled' WHERE telegram_id=$1 AND status='pending'").bind(telegram).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO telegram_bot_actions(id,telegram_id,administrator_id,revision,kind,payload) VALUES($1,$2,$3,$4,$5,$6)").bind(id).bind(telegram).bind(user.id).bind(revision).bind(kind).bind(payload).execute(&mut *tx).await?;tx.commit().await?;
    let mut confirm=button("Подтвердить",&format!("confirm:{id}"));confirm["style"]=json!(if kind=="switch"{"danger"}else{"success"});
    transport::report(state,token,telegram,&format!("Подтверждение действия\n{description}\n\nПодтверждение действительно 5 минут. При изменении DNS потребуется новый просмотр."),vec![vec![confirm,button("Отмена",&format!("cancel:{id}"))]],false).await
}

/// Claim the reviewed action before invoking the existing administrative route.
pub(super) async fn confirm(state:&State,token:&str,telegram:i64,revision:i64,id:Uuid)->Result<()> {
    let user=identity(state,telegram).await?.ok_or(ApiError(StatusCode::FORBIDDEN,"forbidden"))?;user.require("owner")?;
    let mut tx=state.db.begin().await?;
    let row=sqlx::query("UPDATE telegram_bot_actions SET status='running' WHERE id=$1 AND telegram_id=$2 AND administrator_id=$3 AND revision=$4 AND status='pending' AND expires_at>now() RETURNING kind,payload").bind(id).bind(telegram).bind(user.id).bind(revision).fetch_optional(&mut *tx).await?.ok_or(ApiError(StatusCode::CONFLICT,"telegram_confirmation_expired"))?;
    let kind:String=row.get("kind");let payload:Value=row.get("payload");
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'telegram.confirm',$2,$3)").bind(user.id).bind(id.to_string()).bind(json!({"telegram_id":telegram,"kind":kind})).execute(&mut *tx).await?;tx.commit().await?;
    let _=transport::draft(state,token,telegram,"Выполняю подтверждённую операцию…").await;
    let outcome=execute(state,&user,&kind,&payload).await;
    let (status,result,cipher)=match outcome {
        Ok(mut value)=>{
            let compose=value.as_object_mut().and_then(|v|v.remove("compose")).and_then(|v|v.as_str().map(str::to_owned));
            let cipher=compose.map(|value|state.vault.seal(&value,&format!("telegram-compose:{id}"))).transpose()?;
            ("done",value,cipher)
        }
        Err(error)=>(if error.1=="dns_outcome_unknown"||error.0.is_server_error(){"unknown"}else{"failed"},json!({"code":error.1}),None),
    };
    let mut tx=state.db.begin().await?;
    sqlx::query("UPDATE telegram_bot_actions SET status=$2,result=$3,artifact_cipher=$4 WHERE id=$1").bind(id).bind(status).bind(&result).bind(cipher.as_deref()).execute(&mut *tx).await?;
    sqlx::query("INSERT INTO audit(actor,action,target,outcome) VALUES($1,'telegram.result',$2,$3)").bind(user.id).bind(id.to_string()).bind(json!({"telegram_id":telegram,"kind":kind,"status":status,"result":result})).execute(&mut *tx).await?;tx.commit().await?;
    if let Some(code)=result["code"].as_str(){return message(state,token,telegram,&format!("{}\nКод: {code}\nКоманда: {id}",if status=="unknown"{"Результат операции требует сверки в Fleet. Не повторяйте изменение вслепую."}else{transport::explain(code)}),vec![vec![button("Мои операции","actions")]]).await;}
    if cipher.is_some() {
        message(state,token,telegram,"Нода зарегистрирована во Fleet. Compose содержит одноразовый токен подключения на 24 часа. Сохраните docker-compose.yml на новой ноде и выполните docker compose up -d. Передавайте файл только администратору этой ноды.",vec![vec![button("Получить Compose повторно",&format!("file:{id}")),json!({"text":"Скопировать команду","copy_text":{"text":"docker compose up -d"}})]]).await?;
        return artifact(state,token,&user,telegram,revision,id).await;
    }
    let detail=match kind.as_str(){"rotate"=>"Ротация поставлена в очередь. Завершение смотрите в «Мои операции»; уведомление придёт после подтверждения DNS.","switch"=>"Активный сервер изменён, запись DNS подтверждена. Автоматика группы и её IP-пулов выключена. Текущие TCP-сессии не переносятся.",_=>"Настройка применена."};
    message(state,token,telegram,detail,vec![vec![button("Мои операции","actions"),button("Главное меню","menu")]]).await
}
async fn execute(state:&State,user:&Session,kind:&str,payload:&Value)->Result<Value> {
    let target=payload["target"].as_str().unwrap_or("");
    let response=match kind {
        "switch"=>crate::dns::route(state,user,&["groups",target,"force-switch"],&Method::POST,payload["body"].clone()).await?,
        "rotate"=>crate::rotation::route(state,user,&["ip-pools",target,"rotate"],&Method::POST,payload["body"].clone()).await?,
        "group_auto"=>crate::dns::route(state,user,&["groups",target],&Method::PATCH,payload["body"].clone()).await?,
        "pool_auto"=>crate::rotation::route(state,user,&["ip-pools",target],&Method::PATCH,payload["body"].clone()).await?,
        "create"=>crate::managed::route(state,user,&["managed-nodes"],&Method::POST,payload["body"].clone()).await?,
        "enrollment"=>crate::managed::route(state,user,&["nodes",target,"enrollment"],&Method::POST,json!({})).await?,
        _=>return Err(bad()),
    };Ok(response.0["data"].clone())
}
/// Recover the encrypted deployment document without renewing its enrollment token.
pub(super) async fn artifact(state:&State,token:&str,user:&Session,telegram:i64,revision:i64,id:Uuid)->Result<()> {
    user.require("owner")?;
    let cipher:Option<String>=sqlx::query_scalar("SELECT artifact_cipher FROM telegram_bot_actions WHERE id=$1 AND telegram_id=$2 AND administrator_id=$3 AND revision=$4 AND status='done' AND created_at>now()-interval '24 hours'").bind(id).bind(telegram).bind(user.id).bind(revision).fetch_optional(&state.db).await?.flatten();
    let cipher=cipher.ok_or(ApiError(StatusCode::CONFLICT,"telegram_artifact_unavailable"))?;
    let compose=state.vault.open(&cipher,&format!("telegram-compose:{id}"))?;
    transport::document(state,token,telegram,&compose).await
}
/// Show durable command outcomes and current asynchronous rotation progress.
pub(super) async fn history(state:&State,token:&str,user:&Session,telegram:i64)->Result<()> {
    let rows=sqlx::query("SELECT id,kind,status,result,artifact_cipher IS NOT NULL AS artifact FROM telegram_bot_actions WHERE telegram_id=$1 AND administrator_id=$2 ORDER BY created_at DESC LIMIT 10").bind(telegram).bind(user.id).fetch_all(&state.db).await?;
    let mut out=String::from("Мои операции\n");let mut buttons=Vec::new();
    for row in rows {
        let id:Uuid=row.get("id");let kind:String=row.get("kind");let status:String=row.get("status");let result:Value=row.get("result");
        let title=match kind.as_str(){"switch"=>"Смена сервера","rotate"=>"Ротация IP","create"=>"Добавление ноды","enrollment"=>"Файл подключения",_=>"Автоматика"};
        let state_label=match status.as_str(){"pending"=>"ожидает подтверждения","running"=>"выполняется","done"=>"принято","failed"=>"ошибка","cancelled"=>"отменено",_=>"результат требует сверки"};
        out.push_str(&format!("\n{title}: {state_label}\n{id}\n"));
        if kind=="rotate"&&let Some(rotation)=result["id"].as_str().and_then(|v|Uuid::parse_str(v).ok()) {
            let progress:Option<String>=sqlx::query_scalar("SELECT state FROM ip_rotations WHERE id=$1").bind(rotation).fetch_optional(&state.db).await?;
            out.push_str(match progress.as_deref(){Some("succeeded")=>"DNS переключён\n",Some("queued")=>"В очереди\n",Some("dns_pending")=>"Подтверждается DNS\n",Some("failed")=>"Ротация не выполнена\n",_=>"Проверьте результат в Fleet\n"});
        }
        if let Some(code)=result["code"].as_str(){out.push_str(&format!("Код: {}\n",clean(code,100)));}
        if row.get::<bool,_>("artifact")&&user.role=="owner"{buttons.push(vec![button(&format!("Compose · {}",&id.to_string()[..8]),&format!("file:{id}"))]);}
    }
    buttons.push(vec![button("Обновить","actions"),button("Главное меню","menu")]);message(state,token,telegram,&out,buttons).await
}
