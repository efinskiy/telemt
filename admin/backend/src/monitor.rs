use crate::state::{State,node_request,redact};
use serde_json::{Value,json};
use sqlx::Row;
use uuid::Uuid;

pub(crate) async fn collect(state:&State)->anyhow::Result<()> {
    let rows=sqlx::query("SELECT id,snapshot,collected_at FROM nodes WHERE enabled ORDER BY id LIMIT 20").fetch_all(&state.db).await?;
    let mut tasks=tokio::task::JoinSet::new();
    for row in rows {let state=state.clone();tasks.spawn(async move {
        let id:Uuid=row.get("id");let previous:Value=row.get("snapshot");let old_at=row.get::<Option<chrono::DateTime<chrono::Utc>>,_>("collected_at");
        let mut snapshot=json!({"management":"unreachable","ready":false});
        let mut reads=tokio::task::JoinSet::new();
        for (key,path) in [("health","/v1/health/ready"),("info","/v1/system/info"),("summary","/v1/stats/summary"),("traffic","/v1/fleet/traffic"),("shaping","/v1/runtime/shaping?limit=1"),("compatibility","/v1/fleet/compatibility"),("host","/v1/runtime/system-resources"),("dcs","/v1/stats/dcs"),("gates","/v1/runtime/gates")] {
            let state=state.clone(); reads.spawn(async move {(key,node_request(&state,id,reqwest::Method::GET,path,None,None).await)});
        }
        while let Some(read)=reads.join_next().await {
            let (key,result)=read?;
            match result {Ok((status,data))=>{snapshot[key]=if data["ok"]==true{data["data"].clone()}else{json!({"unavailable":true})};if key=="health"{snapshot["management"]=json!("reachable");snapshot["ready"]=json!(status==200&&data["data"]["ready"]==true);}},Err(_)=>{snapshot[key]=json!({"unavailable":true});}}
        }
        if let Some(object)=snapshot["shaping"].as_object_mut(){object.remove("ips");object.remove("users");}
        let now=chrono::Utc::now();let seconds=old_at.map(|at|now.signed_duration_since(at).num_milliseconds() as f64/1000.0).unwrap_or(0.0);
        let same_process=snapshot["traffic"]["counter_instance"].is_string() && snapshot["traffic"]["counter_instance"]==previous["traffic"]["counter_instance"]
            && snapshot["traffic"]["runtime_generation"]==previous["traffic"]["runtime_generation"]
            && snapshot["traffic"]["available"]==true && previous["traffic"]["available"]==true
            && snapshot["traffic"]["counter_source"]==previous["traffic"]["counter_source"];
        for (counter,rate) in [("octets_from_client","upload_bps"),("octets_to_client","download_bps")] {
            let current=snapshot["traffic"][counter].as_u64();let before=previous["traffic"][counter].as_u64();
            snapshot[rate]=match (current,before){(Some(current),Some(before)) if same_process&&seconds>0.0&&current>=before=>json!((current-before) as f64*8.0/seconds),_=>Value::Null};
        }
        overview_rates(&mut snapshot, &previous, seconds);
        snapshot=redact(snapshot);
        let mut tx=state.db.begin().await?;
        sqlx::query("UPDATE nodes SET snapshot=$2,collected_at=$3 WHERE id=$1").bind(id).bind(&snapshot).bind(now).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO node_history(node_id,snapshot) VALUES($1,$2)").bind(id).bind(&snapshot).execute(&mut *tx).await?;
        let old:Option<Value>=sqlx::query_scalar("SELECT snapshot FROM minute_history WHERE node_id=$1 AND minute=date_trunc('minute',now()) FOR UPDATE").bind(id).fetch_optional(&mut *tx).await?;
        let mut aggregate=snapshot.clone();aggregate["samples"]=json!(old.as_ref().and_then(|v|v["samples"].as_u64()).unwrap_or(0)+1);
        for rate in ["upload_bps","download_bps"] {
            let sum_key=format!("{rate}_sum");let count_key=format!("{rate}_samples");
            let count=old.as_ref().and_then(|v|v[&count_key].as_u64()).unwrap_or(0)+u64::from(snapshot[rate].is_number());
            let sum=old.as_ref().and_then(|v|v[&sum_key].as_f64()).unwrap_or(0.0)+snapshot[rate].as_f64().unwrap_or(0.0);
            aggregate[&sum_key]=json!(sum);aggregate[&count_key]=json!(count);aggregate[rate]=if count>0{json!(sum/count as f64)}else{Value::Null};
        }
        sqlx::query("INSERT INTO minute_history(node_id,minute,snapshot) VALUES($1,date_trunc('minute',now()),$2) ON CONFLICT(node_id,minute) DO UPDATE SET snapshot=$2").bind(id).bind(&aggregate).execute(&mut *tx).await?;tx.commit().await?;Ok::<_,anyhow::Error>(())
    });}
    while let Some(result)=tasks.join_next().await{result??;}Ok(())
}
pub(crate) async fn retention(state:&State)->anyhow::Result<()> {
    for query in ["DELETE FROM ip_rotations WHERE updated_at<now()-interval '180 days' AND state IN ('succeeded','failed')","DELETE FROM dns_record_cooldown WHERE last_switch<now()-interval '180 days'","DELETE FROM sessions WHERE expires_at<now()","DELETE FROM login_limits WHERE window_start<now()-interval '1 hour'","DELETE FROM node_history WHERE at<now()-interval '24 hours'","DELETE FROM minute_history WHERE minute<now()-interval '30 days'","DELETE FROM audit WHERE at<now()-interval '180 days'","DELETE FROM probe_jobs WHERE issued_at<now()-interval '24 hours'","DELETE FROM operations WHERE updated_at<now()-interval '180 days' AND state NOT IN ('running','unknown')","DELETE FROM dns_changes WHERE created_at<now()-interval '180 days' AND state NOT IN ('writing','unknown')"] {sqlx::query(query).execute(&state.db).await?;}Ok(())
}


// Counter deltas are valid only within one process or one visible OS boot.
fn overview_rates(current: &mut Value, previous: &Value, seconds: f64) {
    let delta = |now: &Value, before: &Value| now.as_u64()?.checked_sub(before.as_u64()?);
    let same_process = current["traffic"]["counter_instance"].is_string()
        && current["traffic"]["counter_instance"] == previous["traffic"]["counter_instance"];
    current["handshake_timeouts_per_minute"] = if same_process && seconds > 0.0 {
        delta(&current["summary"]["handshake_timeouts_total"], &previous["summary"]["handshake_timeouts_total"])
            .map_or(Value::Null, |value| json!(value as f64 * 60.0 / seconds))
    } else { Value::Null };
    let host = &current["host"]; let old = &previous["host"];
    if host["boot_id"].is_string() && host["boot_id"] == old["boot_id"] && seconds > 0.0 {
        let total = delta(&host["cpu"]["total"], &old["cpu"]["total"]);
        let percent = |key: &str| total.filter(|total| *total > 0).and_then(|total| {
            let value = delta(&host["cpu"][key], &old["cpu"][key])?;
            (value <= total).then_some(value as f64 * 100.0 / total as f64)
        });
        let idle = percent("idle"); let iowait = percent("iowait"); let softirq = percent("softirq");
        let process = if same_process { total.filter(|v| *v > 0).and_then(|total| Some(delta(&host["process_cpu_ticks"], &old["process_cpu_ticks"])? as f64 * host["cpu"]["cores"].as_f64()? / total as f64)) } else { None };
        let retrans = delta(&host["tcp"]["RetransSegs"], &old["tcp"]["RetransSegs"]);
        let outgoing = delta(&host["tcp"]["OutSegs"], &old["tcp"]["OutSegs"]);
        let retries = outgoing.filter(|v| same_process && *v > 0).and_then(|count| Some(retrans? as f64 * 100.0 / count as f64));
        current["host"]["cpu_idle_percent"] = json!(idle);
        current["host"]["cpu_busy_percent"] = json!(idle.zip(iowait).and_then(|(a,b)| (a+b<=100.0).then_some(100.0-a-b)));
        current["host"]["softirq_percent"] = json!(softirq);
        current["host"]["process_cpu_cores"] = json!(process);
        current["host"]["tcp_retransmit_percent"] = json!(retries);
    }
}

#[cfg(test)]
mod overview_tests {
    use super::*;
    #[test]
    fn resource_deltas_require_the_correct_counter_identity() {
        let previous = json!({"traffic":{"counter_instance":"old"},"summary":{"handshake_timeouts_total":10},"host":{"boot_id":"boot","cpu":{"total":1000,"idle":200,"iowait":20,"softirq":10,"cores":4},"process_cpu_ticks":20,"tcp":{"RetransSegs":10,"OutSegs":100}}});
        let sample = json!({"traffic":{"counter_instance":"old"},"summary":{"handshake_timeouts_total":11},"host":{"boot_id":"boot","cpu":{"total":1100,"idle":240,"iowait":30,"softirq":15,"cores":4},"process_cpu_ticks":30,"tcp":{"RetransSegs":12,"OutSegs":200}}});
        let mut current = sample.clone(); overview_rates(&mut current, &previous, 5.0);
        assert_eq!(current["host"]["cpu_busy_percent"], 50.0);
        assert_eq!(current["host"]["process_cpu_cores"], 0.4);
        assert_eq!(current["host"]["tcp_retransmit_percent"], 2.0);
        assert_eq!(current["handshake_timeouts_per_minute"], 12.0);
        let mut restarted = sample.clone(); restarted["traffic"]["counter_instance"] = json!("new");
        overview_rates(&mut restarted, &previous, 5.0);
        assert_eq!(restarted["host"]["cpu_busy_percent"], 50.0);
        assert!(restarted["host"]["process_cpu_cores"].is_null());
        assert!(restarted["host"]["tcp_retransmit_percent"].is_null());
        assert!(restarted["handshake_timeouts_per_minute"].is_null());
        let mut rebooted = sample; rebooted["host"]["boot_id"] = json!("other");
        overview_rates(&mut rebooted, &previous, 5.0);
        assert!(rebooted["host"]["cpu_busy_percent"].is_null());
    }
}
