//! Analytics preserve freshness, counter units and uncertainty instead of treating missing data as zero.
use super::*;
use crate::state::fresh;
use transport::clean;

fn number(value:&Value)->String {value.as_f64().filter(|n|n.is_finite()).map(|n|format!("{n:.1}")).unwrap_or_else(||"нет данных".into())}
fn count(value:&Value)->String {value.as_u64().map(|n|n.to_string()).unwrap_or_else(||"нет данных".into())}
fn bytes(value:&Value)->String {value.as_f64().map(|n|format!("{:.2} ГиБ",n/1_073_741_824.0)).unwrap_or_else(||"нет данных".into())}
fn rate(value:&Value)->String {value.as_f64().map(|n|format!("{:.2} Мбит/с",n/1_000_000.0)).unwrap_or_else(||"нет данных".into())}
/// Summarize current per-node counts without treating unavailable samples as zero.
pub(super) async fn summary(state:&State)->Result<String> {
    let rows=sqlx::query("SELECT name,enabled,snapshot,collected_at FROM nodes ORDER BY name LIMIT 20").fetch_all(&state.db).await?;
    let mut text=format!("Fleet · серверов: {}\n",rows.len());let mut total=0u64;let mut covered=0;
    for row in &rows {
        let snapshot:Value=row.get("snapshot");let live=row.get::<bool,_>("enabled")&&fresh(row.get("collected_at"),20)&&snapshot["management"]=="reachable";
        let traffic=&snapshot["traffic"];let ips=if live{traffic["active_ips"].as_u64()}else{None};
        if let Some(n)=ips {total+=n;covered+=1;}
        text.push_str(&format!("\n{}: {}\nIP: {}{} · соединений: {}\n↑ {} · ↓ {}\n",clean(&row.get::<String,_>("name"),70),if live{if snapshot["ready"]==true{"готов"}else{"не готов"}}else{"нет свежих данных"},if traffic["active_ips_incomplete"]==true{"≈ "}else{""},ips.map(|n|n.to_string()).unwrap_or_else(||"нет данных".into()),if live{count(&traffic["current_connections"])}else{"нет данных".into()},if live{rate(&snapshot["upload_bps"])}else{"—".into()},if live{rate(&snapshot["download_bps"])}else{"—".into()}));
    }
    text.push_str(&format!("\nВсего IP по нодам: {total} (измерение есть на {covered}/{}).\nПояснение: Один IP на двух нодах учитывается дважды. Неполные снимки отмечены ≈.",rows.len()));Ok(text)
}
/// Report resource and upstream measurements only when their snapshots are fresh.
pub(super) async fn node(state:&State,id:Uuid)->Result<String> {
    let row=crate::state::node(state,id).await?;let s:Value=row.get("snapshot");let doc:Value=row.get("document");
    let age=row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("collected_at").map(|at|chrono::Utc::now().signed_duration_since(at).num_seconds().max(0));
    let live=row.get::<bool,_>("enabled")&&fresh(row.get("collected_at"),20)&&s["management"]=="reachable";
    let mut out=format!("{}\nIPv4: {} · IPv6: {}\nМониторинг: {}\n",clean(&row.get::<String,_>("name"),100),doc["ipv4"].as_str().unwrap_or("—"),doc["ipv6"].as_str().unwrap_or("—"),if live{"свежие данные".into()}else{format!("НЕТ СВЕЖИХ ДАННЫХ (возраст: {} с)",age.map(|v|v.to_string()).unwrap_or_else(||"неизвестен".into()))});
    if !live {out.push_str("Текущая нагрузка и число клиентов неизвестны. Проверки IP доступны отдельной кнопкой.");return Ok(out);}
    let t=&s["traffic"];let h=&s["host"];
    out.push_str(&format!("\nIP онлайн: {}{} · соединений: {}\nАккаунтов: {}\n↑ {} · ↓ {}\nТрафик ↑ {} · ↓ {}\n",if t["active_ips_incomplete"]==true{"≈ "}else{""},count(&t["active_ips"]),count(&t["current_connections"]),count(&t["active_users"]),rate(&s["upload_bps"]),rate(&s["download_bps"]),bytes(&t["octets_from_client"]),bytes(&t["octets_to_client"])));
    let total=t["octets_from_client"].as_u64().zip(t["octets_to_client"].as_u64()).and_then(|(a,b)|a.checked_add(b));out.push_str(&format!("Всего: {} (текущие счётчики, не календарный период)\n",bytes(&json!(total))));
    if h["available"]==true {
        let used=h["memory_total_bytes"].as_u64().zip(h["memory_available_bytes"].as_u64()).and_then(|(a,b)|a.checked_sub(b));
        out.push_str(&format!("\nCPU: {}% · ядер {} · LA1 {}\nRAM: {} / {}\nTelemt RSS: {}\nПовторные TCP-сегменты: {}%\n",number(&h["cpu_busy_percent"]),count(&h["cpu"]["cores"]),number(&h["load1"]),bytes(&json!(used)),bytes(&h["memory_total_bytes"]),bytes(&h["process_rss_bytes"]),number(&h["tcp_retransmit_percent"])));
    }else{out.push_str("\nМетрики ОС недоступны.\n");}
    out.push_str(&format!("\nМаршрут: {}\n",match s["gates"]["route_mode"].as_str(){Some("middle")=>"Middle Proxy",Some("direct")=>"Direct",_=>"нет данных"}));
    let dc_fresh=s["dcs"]["generated_at_epoch_secs"].as_i64().is_some_and(|at|chrono::Utc::now().timestamp().abs_diff(at)<=30);
    if dc_fresh {if let Some(dcs)=s["dcs"]["dcs"].as_array() {for dc in dcs.iter().take(16) {out.push_str(&format!("DC {}: {}% · RTT {} мс · сессий {}\n",number(&dc["dc"]),number(&dc["available_pct"]),number(&dc["rtt_ms"]),count(&dc["load"])));}}}else{out.push_str("Нет свежей статистики Telegram DC. Это не подтверждает отказ публичного прокси.\n");}
    out.push_str("Пояснение: Ресурсы ОС относятся к пространству имён ноды.");Ok(out)
}
fn health(value:crate::probes::Health)->&'static str {match value {crate::probes::Health::Healthy=>"доступен",crate::probes::Health::Recovered=>"восстановлен",crate::probes::Health::Failed=>"недоступен",crate::probes::Health::Unknown=>"неизвестно"}}
/// Use the same per-transport evidence policy as automatic failover.
pub(super) async fn ips(state:&State,id:Uuid)->Result<String> {
    let row=crate::state::node(state,id).await?;let doc:Value=row.get("document");
    let pool:Option<Value>=sqlx::query_scalar("SELECT addresses FROM ip_pools WHERE node_id=$1 LIMIT 1").bind(id).fetch_optional(&state.db).await?;
    let mut addresses=std::collections::BTreeSet::new();
    for key in ["ipv4","ipv6"] {if let Some(ip)=doc[key].as_str().filter(|s|!s.is_empty()){addresses.insert(ip.to_owned());}}
    if let Some(list)=pool.as_ref().and_then(Value::as_array){for ip in list.iter().take(8).filter_map(Value::as_str){addresses.insert(ip.to_owned());}}
    let mut out=format!("Здоровье IP · {}\n",clean(&row.get::<String,_>("name"),100));
    for ip in addresses {
        let evidence=crate::probes::evidence(state,id,&ip).await?;
        let last:Option<chrono::DateTime<chrono::Utc>>=sqlx::query_scalar("SELECT max(j.completed_at) FROM probe_jobs j JOIN agents a ON a.id=j.agent_id WHERE j.node_id=$1 AND j.address=$2 AND a.enabled AND a.country='RU'").bind(id).bind(&ip).fetch_one(&state.db).await?;
        out.push_str(&format!("\n{ip}{}\nMTProxy: {} · WEB: {}\nИтог: {} · проверка {}\n",if doc["ipv4"]==ip||doc["ipv6"]==ip{" (текущий адрес ноды)"}else{""},health(evidence[1]),health(evidence[2]),health(evidence[0]),last.map(|at|format!("{} с назад",chrono::Utc::now().signed_duration_since(at).num_seconds().max(0))).unwrap_or_else(||"отсутствует".into())));
    }
    out.push_str("\nПояснение: Неизвестно — нет свежего достаточного свидетельства; причина сбоя сети не определяется.");Ok(out)
}
/// Calculate country shares from the bounded, local GeoIP connection snapshot.
pub(super) async fn countries(state:&State,id:Uuid)->Result<String> {
    let row=crate::state::node(state,id).await?;
    let map=crate::connections_map::read(state,id).await?;
    if map["available"]!=true{return Ok("Страны клиентов: живой снимок недоступен. Проверьте доступность ноды и версию telemt.".into());}
    Ok(country_report(&row.get::<String,_>("name"),&map))
}
fn country_report(name:&str,map:&Value)->String {
    let total=map["total_ips"].as_u64().unwrap_or(0);let mut countries=std::collections::BTreeMap::<String,u64>::new();let mut listed=0u64;
    if let Some(ips)=map["ips"].as_array(){for ip in ips.iter().take(10000){let country=ip["location"]["country"].as_str().filter(|s|!s.is_empty()).unwrap_or("Не определена");*countries.entry(clean(country,60)).or_default()+=1;listed+=1;}}
    let mut rows:Vec<_>=countries.into_iter().collect();rows.sort_by(|a,b|b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out=format!("Страны клиентов · {}\nIP онлайн: {total}\n",clean(name,100));
    if total==0{out.push_str("Нет подключённых IP.");return out;}
    let mut top=0;
    for (country,n) in rows.iter().take(15){out.push_str(&format!("{country}: {:.1}% ({n} IP)\n",*n as f64*100.0/total as f64));top+=n;}
    if listed>top{out.push_str(&format!("Остальные страны: {:.1}% ({} IP)\n",(listed-top)as f64*100.0/total as f64,listed-top));}
    if total>listed{out.push_str(&format!("Вне доступной части снимка: {:.1}% ({} IP)\n",(total-listed)as f64*100.0/total as f64,total-listed));}
    out.push_str("Пояснение: Доли от всех IP этой ноды, не от TCP-соединений. GeoIP — приблизительная география.");
    if map["database"]["status"]!="ready"{out.push_str("\nБаза GeoIP недоступна или неполна; неизвестные страны не угадываются.");}
    if map["cleanup_pending"].as_u64().unwrap_or(0)>0||map["truncated"]==true{out.push_str("\nСнимок неполный или ожидается очистка соединений.");}out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn country_percentages_include_unknown_and_truncated_ips() {
        let report=country_report("node",&json!({"total_ips":4,"truncated":true,"database":{"status":"ready"},"ips":[{"location":{"country":"Россия"}},{"location":null}]}));
        assert!(report.contains("Россия: 25.0% (1 IP)"));assert!(report.contains("Не определена: 25.0% (1 IP)"));assert!(report.contains("снимка: 50.0% (2 IP)"));
        assert!(!country_report("node",&json!({"total_ips":0})).contains("NaN"));
    }
}
