//! Bounded, read-only OS measurements collected outside the relay data path.
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

// Large proc files must not make a monitoring request an unbounded allocation.
const MAX_PROC_BYTES: u64 = 1024 * 1024;

async fn read(path: &str) -> Option<String> {
    tokio::time::timeout(std::time::Duration::from_millis(500), async {
        let file = tokio::fs::File::open(path).await.ok()?;
        let mut bytes = Vec::new();
        file.take(MAX_PROC_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .ok()?;
        if bytes.len() as u64 > MAX_PROC_BYTES {
            return None;
        }
        String::from_utf8(bytes).ok()
    })
    .await
    .ok()
    .flatten()
}

fn field(source: Option<&str>, key: &str) -> Option<u64> {
    source?.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name != key {
            return None;
        }
        value.split_whitespace().next()?.parse().ok()
    })
}

fn kib(source: Option<&str>, key: &str) -> Option<u64> {
    field(source, key)?.checked_mul(1024)
}

fn cpu(source: Option<&str>) -> Value {
    let Some(source) = source else {
        return Value::Null;
    };
    let Some(line) = source.lines().find(|line| line.starts_with("cpu ")) else {
        return Value::Null;
    };
    let values: Option<Vec<u64>> = line
        .split_whitespace()
        .skip(1)
        .take(8)
        .map(|value| value.parse().ok())
        .collect();
    let Some(values) = values.filter(|values| values.len() == 8) else {
        return Value::Null;
    };
    let Some(total) = values
        .iter()
        .try_fold(0u64, |sum, value| sum.checked_add(*value))
    else {
        return Value::Null;
    };
    // Guest ticks are already included in user/nice; adding them again distorts percentages.
    json!({"total":total,"idle":values[3],"iowait":values[4],"softirq":values[6],
        "cores":source.lines().filter(|line| line.strip_prefix("cpu").is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()))).count()})
}

fn tcp(source: Option<&str>) -> Value {
    let Some(source) = source else {
        return Value::Null;
    };
    let mut lines = source.lines().filter(|line| line.starts_with("Tcp:"));
    let (Some(keys), Some(values)) = (lines.next(), lines.next()) else {
        return Value::Null;
    };
    let mut result = json!({});
    for (key, value) in keys
        .split_whitespace()
        .zip(values.split_whitespace())
        .skip(1)
    {
        if ["RetransSegs", "OutSegs", "CurrEstab"].contains(&key) {
            result[key] = value
                .parse::<u64>()
                .map_or(Value::Null, |value| json!(value));
        }
    }
    result
}

/// Samples the visible Linux namespaces; missing sources remain null, never fabricated zeroes.
pub(super) async fn collect() -> Value {
    if !cfg!(target_os = "linux") {
        return json!({"available":false,"reason":"unsupported_os"});
    }
    let (stat, mem, load, uptime, model, process, process_stat, sockets, snmp, ct, ct_max, boot) = tokio::join!(
        read("/proc/stat"),
        read("/proc/meminfo"),
        read("/proc/loadavg"),
        read("/proc/uptime"),
        read("/proc/cpuinfo"),
        read("/proc/self/status"),
        read("/proc/self/stat"),
        read("/proc/net/sockstat"),
        read("/proc/net/snmp"),
        read("/proc/sys/net/netfilter/nf_conntrack_count"),
        read("/proc/sys/net/netfilter/nf_conntrack_max"),
        read("/proc/sys/kernel/random/boot_id")
    );
    let cpu = cpu(stat.as_deref());
    let model = model.as_deref().and_then(|source| {
        source.lines().find_map(|line| {
            let (key, value) = line.split_once(':')?;
            ["model name", "Hardware", "Processor"]
                .contains(&key.trim())
                .then(|| value.trim().chars().take(256).collect::<String>())
        })
    });
    let process_ticks = process_stat.as_deref().and_then(|value| {
        // Process names may include spaces and parentheses; fields start after the final ')'.
        let fields: Vec<_> = value
            .rsplit_once(')')?
            .1
            .split_whitespace()
            .take(15)
            .collect();
        fields
            .get(11)?
            .parse::<u64>()
            .ok()?
            .checked_add(fields.get(12)?.parse::<u64>().ok()?)
    });
    let sockets = sockets
        .as_deref()
        .and_then(|value| value.lines().find(|line| line.starts_with("sockets:")))
        .and_then(|line| line.split_whitespace().nth(2)?.parse::<u64>().ok());
    let number = |source: Option<&str>| {
        source
            .and_then(|value| value.split_whitespace().next()?.parse::<f64>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
    };
    let memory = mem.as_deref();
    let disk = disk().await;
    json!({"available":!cpu.is_null() || kib(memory,"MemTotal").is_some(),"sampled_at":chrono::Utc::now().to_rfc3339(),
        "scope":"visible_proc_and_network_namespace","boot_id":boot.as_deref().map(str::trim),
        "cpu":cpu,"cpu_model":model,"load1":number(load.as_deref()),"uptime_seconds":number(uptime.as_deref()),
        "memory_total_bytes":kib(memory,"MemTotal"),"memory_available_bytes":kib(memory,"MemAvailable"),
        "swap_total_bytes":kib(memory,"SwapTotal"),"swap_free_bytes":kib(memory,"SwapFree"),
        "process_rss_bytes":kib(process.as_deref(),"VmRSS"),"process_cpu_ticks":process_ticks,
        "sockets":sockets,"tcp":tcp(snmp.as_deref()),
        "conntrack_count":ct.as_deref().and_then(|v|v.trim().parse::<u64>().ok()),
        "conntrack_max":ct_max.as_deref().and_then(|v|v.trim().parse::<u64>().ok()),"disk":disk})
}

#[cfg(target_os = "linux")]
async fn disk() -> Value {
    // Filesystem queries may block; no mount enumeration or relay-thread filesystem calls.
    static SLOT: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let Ok(permit) = SLOT.try_acquire() else {
        return Value::Null;
    };
    tokio::time::timeout(
        std::time::Duration::from_millis(500),
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let Ok(stat) = nix::sys::statvfs::statvfs("/") else {
                return Value::Null;
            };
            json!({"path":"/","total_bytes":stat.blocks().checked_mul(stat.fragment_size()),
            "available_bytes":stat.blocks_available().checked_mul(stat.fragment_size())})
        }),
    )
    .await
    .ok()
    .and_then(Result::ok)
    .unwrap_or(Value::Null)
}

#[cfg(not(target_os = "linux"))]
async fn disk() -> Value {
    Value::Null
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_proc_parsers_keep_missing_values_distinct_from_zero() {
        let parsed = cpu(Some(
            "cpu  10 2 3 20 4 5 6 7 100 100\ncpu0 10 2 3 20 4 5 6 7",
        ));
        assert_eq!(parsed["total"], 57);
        assert_eq!(parsed["cores"], 1);
        assert!(cpu(Some("cpu 1 broken")).is_null());
        assert_eq!(
            kib(Some("MemTotal: 2048 kB\nSwapTotal: 0 kB"), "MemTotal"),
            Some(2097152)
        );
        assert_eq!(kib(Some("SwapTotal: 0 kB"), "SwapTotal"), Some(0));
        assert_eq!(kib(None, "SwapTotal"), None);
        let parsed = tcp(Some("Tcp: OutSegs RetransSegs CurrEstab\nTcp: 100 5 2"));
        assert_eq!(parsed["RetransSegs"], 5);
        assert!(tcp(Some("Tcp: OutSegs")).is_null());
    }
}
