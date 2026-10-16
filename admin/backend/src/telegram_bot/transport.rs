//! Fixed-origin Telegram requests never expose URL credentials or raw provider error bodies.
use super::*;
use std::time::Duration;

fn url(state:&State,token:&str,method:&str)->String {
    let origin="https://api.telegram.org";
    #[cfg(test)]
    let origin=state.telegram_origin.as_deref().unwrap_or(origin);
    let _=state;
    format!("{origin}/bot{token}/{method}")
}
async fn response(state:&State,request:reqwest::RequestBuilder)->Result<Value> {
    let work=async {
        let mut response=request.send().await.map_err(|_|ApiError(StatusCode::BAD_GATEWAY,"telegram_transport_error"))?;
        let status=response.status();let mut bytes=Vec::new();
        while let Some(chunk)=response.chunk().await.map_err(|_|ApiError(StatusCode::BAD_GATEWAY,"telegram_transport_error"))? {
            if bytes.len()+chunk.len()>128*1024{return Err(ApiError(StatusCode::BAD_GATEWAY,"telegram_response_too_large"));}bytes.extend_from_slice(&chunk);
        }
        let body:Value=serde_json::from_slice(&bytes).map_err(|_|ApiError(StatusCode::BAD_GATEWAY,"telegram_api_error"))?;
        if status.is_success()&&body["ok"]==true{return Ok(body["result"].clone());}
        let code=body["error_code"].as_u64().unwrap_or(status.as_u16().into());
        if code==429 {
            let delay=body["parameters"]["retry_after"].as_i64().unwrap_or(60).clamp(1,86400);
            sqlx::query("UPDATE telegram_bot_settings SET next_poll_at=greatest(next_poll_at,now()+make_interval(secs=>$1)) WHERE singleton").bind(delay as f64).execute(&state.db).await?;
        }
        Err(ApiError(StatusCode::BAD_GATEWAY,match code {401=>"telegram_invalid_token",403=>"telegram_forbidden",409=>"telegram_poll_conflict",429=>"telegram_rate_limited",_=>"telegram_api_error"}))
    };
    tokio::time::timeout(Duration::from_secs(5),work).await.map_err(|_|ApiError(StatusCode::GATEWAY_TIMEOUT,"telegram_transport_error"))?
}
/// Bound responses and redact provider failures before crossing the transport boundary.
pub(super) async fn call(state:&State,token:&str,method:&str,body:Value)->Result<Value> {
    response(state,state.client.post(url(state,token,method)).json(&body)).await
}
/// Build callback controls without interpreting administrator-provided text.
pub(super) fn button(label:&str,data:&str)->Value {json!({"text":clean(label,70),"callback_data":data})}
/// Bound displayed labels and remove control characters.
pub(super) fn clean(value:&str,limit:usize)->String {value.chars().filter(|c|!c.is_control()).take(limit).collect()}
/// Send a bounded plain message with protected content.
pub(super) async fn message(state:&State,token:&str,chat:i64,text:&str,keyboard:Vec<Vec<Value>>)->Result<()> {
    let mut length=0;let text:String=text.chars().take_while(|c|{length+=c.len_utf16();length<=3900}).collect();
    call(state,token,"sendMessage",json!({"chat_id":chat,"text":text,"protect_content":true,"link_preview_options":{"is_disabled":true},"reply_markup":{"inline_keyboard":keyboard}})).await?;Ok(())
}
/// Deliver the saved enrollment artifact as a downloadable Compose document.
pub(super) async fn document(state:&State,token:&str,chat:i64,compose:&str)->Result<()> {
    if compose.len()>256*1024{return Err(bad());}
    // All form headers are static; the unpredictable boundary cannot be supplied through the document.
    let boundary=format!("telemt{}",Uuid::new_v4().simple());
    if compose.contains(&boundary){return Err(bad());}
    let body=format!("--{boundary}\r\nContent-Disposition: form-data; name=\"chat_id\"\r\n\r\n{chat}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"protect_content\"\r\n\r\nfalse\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"document\"; filename=\"docker-compose.yml\"\r\nContent-Type: application/yaml\r\n\r\n{compose}\r\n--{boundary}--\r\n");
    response(state,state.client.post(url(state,token,"sendDocument")).header("content-type",format!("multipart/form-data; boundary={boundary}")).body(body)).await?;Ok(())
}
/// Translate known failures without exposing raw provider errors.
pub(super) fn explain(code:&str)->&'static str {
    match code {
        "forbidden"=>"Для этого действия нужны права владельца Fleet.",
        "telegram_confirmation_expired"=>"Подтверждение истекло или уже использовано. Откройте действие заново.",
        "telegram_session_expired"=>"Шаг мастера истёк. Начните действие заново через /start.",
        "telegram_profile_changed"|"dns_preview_changed"|"rotation_preview_changed"=>"Настройки изменились после просмотра. Получите новый предварительный просмотр.",
        "external_dns_conflict"=>"DNS изменён вне Fleet. Разрешите конфликт в кабинете.",
        "dns_outcome_unknown"|"telegram_action_interrupted"=>"Результат операции требует сверки в Fleet. Не повторяйте изменение вслепую.",
        "dns_target_already_active"=>"Выбранный сервер и его адреса уже активны.",
        "managed_image_not_configured"=>"В Fleet не настроен MANAGED_NODE_IMAGE. Укажите образ управляемой ноды.",
        "reserve_ip_not_prepared_or_without_quorum"=>"Адрес ещё не подготовлен или его доступность не подтверждена проверками.",
        "dns_cooldown"|"rotation_cooldown"=>"Слишком мало времени прошло после предыдущего переключения.",
        "resource_exists"=>"Сервер с таким именем или запись уже существует.",
        "telegram_artifact_unavailable"=>"Compose недоступен. Проверьте сервер в Fleet; для ещё не зарегистрированной ноды обновите файл регистрации.",
        "invalid_request"=>"Некорректный ввод. Проверьте имя, IP и выбранное действие.",
        _=>"Операция не завершена. Подробности доступны в Fleet по указанному коду.",
    }
}

// Bot API 10.3 block entities avoid HTML interpolation and provide native compact tables.
fn rich(text:&str,keyboard:Vec<Vec<Value>>,table:bool)->Value {
    let mut lines=text.lines();let title=lines.next().unwrap_or("Telemt Fleet");
    let mut blocks=vec![json!({"type":"heading","size":2,"text":title})];
    let mut cells=Vec::new();
    for line in lines.take(160) {
        if table&&!line.starts_with("Пояснение: ")&&let Some((key,value))=line.split_once(": ") {
            cells.push(json!([{"text":key,"align":"left","valign":"top"},{"text":value,"align":"left","valign":"top"}]));continue;
        }
        // Flush at section boundaries so an IP or node heading stays attached to its own measurements.
        if !cells.is_empty(){blocks.push(json!({"type":"table","is_compact":true,"is_striped":true,"cells":std::mem::take(&mut cells)}));}
        if let Some(note)=line.strip_prefix("Пояснение: ") {
            blocks.push(json!({"type":"details","summary":"Пояснение к данным","blocks":[{"type":"paragraph","text":note}]}));
        }else if !line.is_empty(){blocks.push(json!({"type":"paragraph","text":line}));}
    }
    if !cells.is_empty(){blocks.push(json!({"type":"table","is_compact":true,"is_striped":true,"cells":cells}));}
    blocks.push(json!({"type":"footer","text":["Сформировано: ",{"type":"date_time","text":chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string(),"unix_time":chrono::Utc::now().timestamp(),"date_time_format":"dT"}]}));
    for row in keyboard.into_iter().take(25) {blocks.push(json!({"type":"buttons","buttons":row,"align":"left"}));}
    json!({"blocks":blocks,"skip_entity_detection":true})
}
/// Send structured report blocks through the current rich-message API.
pub(super) async fn report(state:&State,token:&str,chat:i64,text:&str,keyboard:Vec<Vec<Value>>,table:bool)->Result<()> {
    if text.len()>32*1024{return Err(bad());}
    call(state,token,"sendRichMessage",json!({"chat_id":chat,"rich_message":rich(text,keyboard,table),"protect_content":true})).await?;Ok(())
}
/// Show short-lived progress while a bounded operation is running.
pub(super) async fn draft(state:&State,token:&str,chat:i64,text:&str)->Result<()> {
    call(state,token,"sendRichMessageDraft",json!({"chat_id":chat,"draft_id":1,"rich_message":{"blocks":[{"type":"paragraph","text":text}]},"can_stop":false})).await?;Ok(())
}
/// Request an explicit reply while retaining the cancel control.
pub(super) async fn prompt(state:&State,token:&str,chat:i64,text:&str)->Result<()> {
    call(state,token,"sendMessage",json!({"chat_id":chat,"text":text,"protect_content":true,"reply_markup":{"inline_keyboard":[[button("Отмена","menu")]],"force_reply":true}})).await?;Ok(())
}
