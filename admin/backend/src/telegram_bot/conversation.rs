//! Explicit callback routes and expiring enrollment steps; payloads from Telegram never select arbitrary API paths.
use super::*;
use axum::http::Method;
use transport::{button,clean,message};
fn id(value:&str)->Result<Uuid>{crate::state::parse_id(value)}
async fn session(state:&State,telegram:i64)->Result<Value> {
    sqlx::query_scalar("SELECT document FROM telegram_bot_sessions WHERE telegram_id=$1 AND expires_at>now()").bind(telegram).fetch_optional(&state.db).await?.ok_or(ApiError(StatusCode::CONFLICT,"telegram_session_expired"))
}
async fn save(state:&State,telegram:i64,document:Value)->Result<()> {
    sqlx::query("INSERT INTO telegram_bot_sessions(telegram_id,document) VALUES($1,$2) ON CONFLICT(telegram_id) DO UPDATE SET document=$2,expires_at=now()+interval '15 minutes'").bind(telegram).bind(document).execute(&state.db).await?;Ok(())
}
fn home()->Vec<Value>{vec![button("Главное меню","menu")]}

/// Accept only explicit menu commands and expiring enrollment steps.
pub(super) async fn handle(state:&State,token:&str,user:&Session,telegram:i64,revision:i64,input:&str,callback:bool)->Result<()> {
    let input=if !callback&&input.starts_with('/') {input.split_whitespace().next().unwrap_or("").split('@').next().unwrap_or("")}else{input};
    match input {
        "/start"|"/help"|"/cancel"|"menu"=>{
            sqlx::query("DELETE FROM telegram_bot_sessions WHERE telegram_id=$1").bind(telegram).execute(&state.db).await?;
            sqlx::query("UPDATE telegram_bot_actions SET status='cancelled' WHERE telegram_id=$1 AND status='pending'").bind(telegram).execute(&state.db).await?;
            let mut keys=vec![vec![button("Сводка Fleet","summary"),button("Серверы","nodes")],vec![button("Группы DNS","groups"),button("IP-пулы","pools")],vec![button("Мои операции","actions")]];
            if user.role=="owner"{keys.push(vec![button("Добавить сервер","add")]);}
            keys.push(vec![json!({"text":"Открыть Fleet","url":state.origin.as_ref()})]);
            transport::report(state,token,telegram,&format!("Telemt Fleet\nАдминистратор: {}\nРоль: {}\nВыберите раздел. Изменения потребуют отдельного подтверждения.",clean(&user.username,100),match user.role.as_str(){"owner"=>"Владелец","operator"=>"Оператор",_=>"Наблюдатель"}),keys,false).await
        }
        "/id"=>message(state,token,telegram,&format!("Ваш Telegram ID: {telegram}"),vec![home()]).await,
        "/status"|"summary"=>transport::report(state,token,telegram,&analytics::summary(state).await?,vec![vec![button("Обновить","summary"),button("Серверы","nodes")],home()],true).await,
        "/nodes"|"nodes"=>{
            let rows=sqlx::query("SELECT id,name FROM nodes ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            let mut keys:Vec<_>=rows.iter().map(|r|vec![button(&r.get::<String,_>("name"),&format!("node:{}",r.get::<Uuid,_>("id")))]).collect();keys.push(home());
            message(state,token,telegram,"Серверы · выберите ноду для аналитики",keys).await
        }
        "/groups"|"groups"|"pools"=>{
            let pool=input=="pools";let query=if pool{"SELECT id,name,status,automatic FROM ip_pools ORDER BY name LIMIT 20"}else{"SELECT id,name,status,automatic FROM failover_groups ORDER BY name LIMIT 20"};
            let rows=sqlx::query(query).fetch_all(&state.db).await?;let mut keys=Vec::new();let mut out:String=if pool{"IP-пулы\n".into()}else{"Группы DNS\n".into()};
            for r in rows {out.push_str(&format!("{} · автоматика {} · {}\n",clean(&r.get::<String,_>("name"),70),if r.get::<bool,_>("automatic"){"вкл"}else{"выкл"},clean(&r.get::<String,_>("status"),70)));keys.push(vec![button(&r.get::<String,_>("name"),&format!("{}:{}",if pool{"pool"}else{"group"},r.get::<Uuid,_>("id")))]);}
            keys.push(home());message(state,token,telegram,&out,keys).await
        }
        "/operations"|"actions"=>actions::history(state,token,user,telegram).await,
        "/add"|"add"=>profiles(state,token,user,telegram,0).await,
        _ if callback=>callback_route(state,token,user,telegram,revision,input).await,
        _=>input_step(state,token,user,telegram,revision,input).await,
    }
}
async fn callback_route(state:&State,token:&str,user:&Session,telegram:i64,revision:i64,input:&str)->Result<()> {
    let (action,value)=input.split_once(':').ok_or_else(bad)?;
    if action=="profiles"{return profiles(state,token,user,telegram,value.parse().ok().filter(|v|*v<=10000).ok_or_else(bad)?).await;}
    if action=="confirm"{return actions::confirm(state,token,telegram,revision,id(value)?).await;}
    if action=="file"{return actions::artifact(state,token,user,telegram,revision,id(value)?).await;}
    if action=="cancel" {
        sqlx::query("UPDATE telegram_bot_actions SET status='cancelled' WHERE id=$1 AND telegram_id=$2 AND administrator_id=$3 AND status='pending'").bind(id(value)?).bind(telegram).bind(user.id).execute(&state.db).await?;
        return message(state,token,telegram,"Действие отменено.",vec![home()]).await;
    }
    if ["node","countries","ips"].contains(&action) {
        let node=id(value)?;
        if action=="countries" {let _=transport::draft(state,token,telegram,"Получаю текущий снимок IP и географию клиентов…").await;}
        let out=match action{"node"=>analytics::node(state,node).await?,"ips"=>analytics::ips(state,node).await?,_=>analytics::countries(state,node).await?};
        let mut keys=vec![vec![button("Обновить",input)],vec![button("Статистика",&format!("node:{node}")),button("Страны",&format!("countries:{node}")),button("Здоровье IP",&format!("ips:{node}"))]];
        if user.role=="owner"{keys.push(vec![button("Файл подключения",&format!("enrollment:{node}"))]);}keys.push(home());
        return transport::report(state,token,telegram,&out,keys,true).await;
    }
    if action=="group" {
        let r=sqlx::query("SELECT g.name,g.document,g.active_node,g.automatic,n.name AS active_name FROM failover_groups g LEFT JOIN nodes n ON n.id=g.active_node WHERE g.id=$1").bind(id(value)?).fetch_one(&state.db).await?;
        let doc:Value=r.get("document");let mut keys=vec![home()];
        if user.role=="owner" {keys.insert(0,vec![button("Сменить активный сервер",&format!("switch:{value}")),button("Настроить автоматику",&format!("gauto:{value}"))]);}
        return message(state,token,telegram,&format!("Группа: {}\nДомен: {}\nАктивный сервер: {}\nАвтоматика: {}",clean(&r.get::<String,_>("name"),100),doc["domain"].as_str().unwrap_or("—"),r.get::<Option<String>,_>("active_name").unwrap_or_else(||"—".into()),if r.get::<bool,_>("automatic"){"вкл"}else{"выкл"}),keys).await;
    }
    if action=="pool" {
        let r=sqlx::query("SELECT p.*,n.name AS node_name FROM ip_pools p JOIN nodes n ON n.id=p.node_id WHERE p.id=$1").bind(id(value)?).fetch_one(&state.db).await?;
        let mut keys=vec![vec![button("Здоровье IP",&format!("ips:{}",r.get::<Uuid,_>("node_id")))]];
        if user.role=="owner" {
            let ips:Value=r.get("addresses");for (i,ip) in ips.as_array().ok_or_else(bad)?.iter().take(8).enumerate(){keys.push(vec![button(&format!("Переключить на {}",ip.as_str().unwrap_or("")),&format!("rotate:{value}:{i}"))]);}
            keys.push(vec![button("Настроить авторотацию",&format!("pauto:{value}"))]);
        }keys.push(home());return message(state,token,telegram,&format!("IP-пул: {}\nСервер: {}\nСтатус: {}\nАвтоматика: {}",clean(&r.get::<String,_>("name"),100),clean(&r.get::<String,_>("node_name"),100),r.get::<String,_>("status"),if r.get::<bool,_>("automatic"){"вкл"}else{"выкл"}),keys).await;
    }
    user.require("owner")?;
    match action {
        "switch"=>{
            let group=id(value)?;let doc:Value=sqlx::query_scalar("SELECT document FROM failover_groups WHERE id=$1").bind(group).fetch_one(&state.db).await?;
            let rows=sqlx::query("SELECT id,name FROM nodes ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
            save(state,telegram,json!({"phase":"switch","group":group})).await?;
            let mut keys=Vec::new();for r in rows {let node:Uuid=r.get("id");if doc["node_ids"].as_array().is_some_and(|ids|ids.contains(&json!(node))){keys.push(vec![button(&r.get::<String,_>("name"),&format!("target:{node}"))]);}}keys.push(home());
            message(state,token,telegram,"Выберите новый сервер. Принудительная смена обходит ожидание, проверки доступности и синхронизации. Предварительный просмотр DNS будет показан перед записью.",keys).await
        }
        "target"=>{
            let s=session(state,telegram).await?;if s["phase"]!="switch"{return Err(bad());}let group=s["group"].as_str().ok_or_else(bad)?;let node=id(value)?;
            let preview=crate::dns::route(state,user,&["groups",group,"force-preview"],&Method::POST,json!({"node_id":node})).await?.0["data"].clone();
            let name:String=sqlx::query_scalar("SELECT name FROM nodes WHERE id=$1").bind(node).fetch_one(&state.db).await?;
            let mut description=format!("Смена активного сервера\nДомен: {}\nНовый сервер: {}\n",preview["configuration"]["domain"].as_str().unwrap_or("—"),clean(&name,100));
            for family in ["A","AAAA"]{if preview["intent"][family].is_object(){description.push_str(&format!("{family}: {} → {}\n",preview["observed"][family]["content"].as_str().unwrap_or("—"),preview["intent"][family]["content"].as_str().unwrap_or("—")));}}
            description.push_str("Принудительно, независимо от доступности и синхронизации. Автоматика группы и её пулов будет выключена. TCP-сессии не мигрируют.");
            actions::prepare(state,token,user,telegram,revision,"switch",json!({"target":group,"body":{"node_id":node,"preview":preview}}),&description).await
        }
        "rotate"=>{
            let (pool,index)=value.rsplit_once(':').ok_or_else(bad)?;let pool_id=id(pool)?;let index=index.parse::<usize>().ok().filter(|i|*i<8).ok_or_else(bad)?;
            let ips:Value=sqlx::query_scalar("SELECT addresses FROM ip_pools WHERE id=$1").bind(pool_id).fetch_one(&state.db).await?;let ip=ips[index].as_str().ok_or_else(bad)?;
            let preview=crate::rotation::route(state,user,&["ip-pools",pool,"rotation-preview"],&Method::POST,json!({"ip":ip})).await?.0["data"].clone();
            actions::prepare(state,token,user,telegram,revision,"rotate",json!({"target":pool,"body":preview}),&format!("Ротация IP\n{} → {ip}\nПроверки готовности адреса и DNS остаются обязательными.",preview["old_ip"].as_str().unwrap_or("—"))).await
        }
        "gauto"|"pauto"=>{
            let target=id(value)?;let query=if action=="gauto"{"SELECT name,automatic FROM failover_groups WHERE id=$1"}else{"SELECT name,automatic FROM ip_pools WHERE id=$1"};let row=sqlx::query(query).bind(target).fetch_one(&state.db).await?;
            let enabled=!row.get::<bool,_>("automatic");
            actions::prepare(state,token,user,telegram,revision,if action=="gauto"{"group_auto"}else{"pool_auto"},json!({"target":target,"body":{"automatic":enabled}}),&format!("{} автоматику: {}",if enabled{"Включить"}else{"Выключить"},clean(&row.get::<String,_>("name"),100))).await
        }
        "profile"=>{
            let profile=id(value)?;let r=sqlx::query("SELECT name,published_revision FROM profiles WHERE id=$1 AND published_revision>0").bind(profile).fetch_one(&state.db).await?;
            save(state,telegram,json!({"phase":"name","profile_id":profile,"profile_name":r.get::<String,_>("name"),"profile_revision":r.get::<i64,_>("published_revision")})).await?;
            transport::prompt(state,token,telegram,"Введите название новой ноды (до 128 символов). Для отмены — /cancel.").await
        }
        "enrollment"=>{let node=id(value)?;actions::prepare(state,token,user,telegram,revision,"enrollment",json!({"target":node}),"Выпустить новый Compose для ещё не подключённой ноды? Предыдущий токен регистрации станет недействительным.").await}
        _=>Err(bad()),
    }
}
async fn input_step(state:&State,token:&str,user:&Session,telegram:i64,revision:i64,input:&str)->Result<()> {
    user.require("owner")?;let mut s=session(state,telegram).await?;
    match s["phase"].as_str() {
        Some("name")=>{
            let name=crate::state::bounded_name(&json!({"name":input}),"name")?.to_owned();s["name"]=json!(name);s["phase"]=json!("ip");save(state,telegram,s).await?;
            transport::prompt(state,token,telegram,"Введите IP, назначенный интерфейсу новой ноды (IPv4 или IPv6).").await
        }
        Some("ip")=>{
            let ip:std::net::IpAddr=input.parse().map_err(|_|bad())?;if ip.is_unspecified()||ip.is_multicast()||ip.is_loopback(){return Err(bad());}
            let body=json!({"name":s["name"],"ip":ip.to_string(),"profile_id":s["profile_id"],"expected_profile_revision":s["profile_revision"]});
            actions::prepare(state,token,user,telegram,revision,"create",json!({"body":body}),&format!("Добавить ноду\nНазвание: {}\nIP: {ip}\nПрофиль: {} · версия {}\nCompose с токеном регистрации будет отправлен в этот личный чат.",s["name"].as_str().unwrap_or(""),clean(s["profile_name"].as_str().unwrap_or(""),100),s["profile_revision"])).await?;
            sqlx::query("DELETE FROM telegram_bot_sessions WHERE telegram_id=$1").bind(telegram).execute(&state.db).await?;Ok(())
        }
        _=>Err(ApiError(StatusCode::CONFLICT,"telegram_session_expired")),
    }
}

async fn profiles(state:&State,token:&str,user:&Session,telegram:i64,offset:i64)->Result<()> {
    user.require("owner")?;
    let rows=sqlx::query("SELECT id,name,domain FROM profiles WHERE published_revision>0 ORDER BY name,id LIMIT 21 OFFSET $1").bind(offset).fetch_all(&state.db).await?;
    let mut keys:Vec<_>=rows.iter().take(20).map(|r|vec![button(&format!("{} · {}",r.get::<String,_>("name"),r.get::<String,_>("domain")),&format!("profile:{}",r.get::<Uuid,_>("id")))]).collect();
    let mut pages=Vec::new();if offset>0{pages.push(button("Назад",&format!("profiles:{}",(offset-20).max(0))));}if rows.len()>20{pages.push(button("Далее",&format!("profiles:{}",offset+20)));}if !pages.is_empty(){keys.push(pages);}keys.push(home());
    message(state,token,telegram,"Добавление сервера\nВыберите опубликованный общий профиль. Нужный IP должен быть назначен интерфейсу новой ноды.",keys).await
}
