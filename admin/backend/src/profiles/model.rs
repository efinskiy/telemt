//! Profile validation and node-specific rendering; public endpoint tuples remain exact.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::error::{Result, bad};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SharedUser {
    pub username: String,
    pub secret: String,
    #[serde(default = "enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub quota_bytes: u64,
    #[serde(default)]
    pub up_bps: u64,
    #[serde(default)]
    pub down_bps: u64,
}
fn enabled() -> bool { true }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProfileDocument {
    pub native_port: u16,
    #[serde(default)]
    pub base_path: String,
    pub users: Vec<SharedUser>,
    #[serde(default = "empty")]
    pub settings: Value,
}
fn empty() -> Value { json!({}) }

pub(crate) fn validate(value: Value) -> Result<ProfileDocument> {
    let doc: ProfileDocument = serde_json::from_value(value).map_err(|_|bad())?;
    if doc.native_port == 0 || [443,18080,19091].contains(&doc.native_port) || doc.users.len() > 1000 { return Err(bad()); }
    if doc.base_path.len() > 128 || doc.base_path.split('/').any(|s| s == "." || s == "..") || doc.base_path.starts_with('/') || doc.base_path.ends_with('/') || !doc.base_path.bytes().all(|b|b.is_ascii_alphanumeric() || b"_-/".contains(&b)) { return Err(bad()); }
    let mut names=std::collections::HashSet::new();
    for user in &doc.users {
        if user.username=="fleet_probe" || user.up_bps>100_000_000_000 || user.down_bps>100_000_000_000 || user.username.is_empty() || user.username.len()>64 || !user.username.bytes().all(|b|b.is_ascii_alphanumeric()||b"_.-".contains(&b)) || !names.insert(&user.username) || !crate::state::valid_secret(&user.secret) { return Err(bad()); }
    }
    if !doc.base_path.is_empty() && doc.base_path.split('/').any(|part|part.is_empty()||!part.as_bytes()[0].is_ascii_alphanumeric()){return Err(bad());}
    let settings=doc.settings.as_object().ok_or_else(bad)?;
    if settings.keys().any(|key|!["general","timeouts","shaping","upstreams","dc_overrides","web","access"].contains(&key.as_str())) { return Err(bad()); }
    if let Some(web)=settings.get("web") {
        if !web.is_object() || web.get("vhosts").is_some() || web.get("enabled").is_some() {return Err(bad());}
        // HTTPS is the mandatory common fallback and can be independently verified by external agents.
        if web.get("carrier").is_some_and(|v|v!="https") || web.get("carrier_method").is_some_and(|v|v!="post") {return Err(bad());}
    }
    Ok(doc)
}

pub(crate) fn domain(value:&str) -> Result<String> {
    let domain=value.to_ascii_lowercase();
    if domain.rsplit('.').next().is_some_and(|s|s.bytes().all(|b|b.is_ascii_digit())) || domain.len()>253 || !domain.contains('.') || domain.parse::<std::net::IpAddr>().is_ok() || domain.split('.').any(|part|part.is_empty()||part.len()>63||part.starts_with('-')||part.ends_with('-')||!part.bytes().all(|c|c.is_ascii_alphanumeric()||c==b'-')) {return Err(bad());}Ok(domain)
}

pub(super) fn render(doc:&ProfileDocument,domain:&str,ip:&str,token:&str,probe:&str,overrides:&Value)->Result<(String,String)> {
    let address:std::net::IpAddr=ip.parse().map_err(|_|bad())?;
    let mut value=doc.settings.clone();
    if !value["general"].is_object(){value["general"]=json!({});}
    value["general"]["modes"]=json!({"classic":false,"secure":true,"tls":true});
    value["general"]["links"]=json!({"show":[],"public_host":domain,"public_port":doc.native_port});
    value["server"]=json!({"port":doc.native_port,"api":{"enabled":true,"listen":telemt_managed::API_ADDR,"whitelist":["127.0.0.1/32"],"auth_header":token,"read_only":false},"listeners":[{"ip":ip,"port":doc.native_port,"transport":"mtproxy","proxy_protocol":false,"announce":domain},{"ip":"127.0.0.1","port":18080,"transport":"web","proxy_protocol":false,"reuse_allow":false,"web_client_ip_source":"x_forwarded_for","web_trusted_proxy_cidrs":["127.0.0.1/32"]}]});
    let mut users=json!({"fleet_probe":probe});let mut enabled=json!({});let mut quotas=json!({});let mut rates=json!({});let mut profiles=vec![json!({"user":"fleet_probe","secret_mode":"dd"})];
    for user in &doc.users {
        if user.username=="fleet_probe"{return Err(bad());}
        users[&user.username]=json!(user.secret);enabled[&user.username]=json!(user.enabled);
        if user.quota_bytes>0{quotas[&user.username]=json!(user.quota_bytes);}
        if user.up_bps>0 || user.down_bps>0 {rates[&user.username]=json!({"up_bps":user.up_bps,"down_bps":user.down_bps});}
        profiles.push(json!({"user":user.username,"secret_mode":"dd"}));
    }
    if !value["access"].is_object(){value["access"]=json!({});}
    for (key,data) in [("users",users),("user_enabled",enabled),("user_data_quota",quotas),("user_rate_limits",rates)]{value["access"][key]=data;}
    value["censorship"]=json!({"tls_domain":domain,"mask":true,"mask_dynamic":false,"mask_host":ip,"mask_port":443});
    if !value["web"].is_object(){value["web"]=json!({});}
    if !value["web"]["limits"].is_object(){value["web"]["limits"]=json!({});}
    value["web"]["limits"]["max_profiles"]=json!(doc.users.len()+1);
    value["web"]["enabled"]=json!(true);value["web"]["carrier"]=json!("https");
    value["web"]["vhosts"]=json!([{"host":domain,"base_path":doc.base_path,"public_addr":std::net::SocketAddr::new(address,443).to_string(),"decoy":{"mode":"static_directory","directory":"/app/public","index":"index.html"},"profiles":profiles}]);
    for key in ["upload_bps","download_bps"] {if let Some(capacity)=overrides.get(key).and_then(Value::as_u64) {if !value["shaping"].is_object(){value["shaping"]=json!({});}value["shaping"][if key=="upload_bps"{"up_bps"}else{"down_bps"}]=json!(capacity);}}
    let toml=toml::to_string_pretty(&value).map_err(|_|bad())?;
    let bind=std::net::SocketAddr::new(address,443);
    let nginx=format!(r#"worker_processes auto;
error_log stderr warn;
pid /var/lib/telemt-managed/nginx.pid;
events {{ worker_connections 16384; }}
http {{
    access_log off;
    client_body_temp_path /tmp/client_body;
    proxy_temp_path /tmp/proxy;
    fastcgi_temp_path /tmp/fastcgi;
    uwsgi_temp_path /tmp/uwsgi;
    scgi_temp_path /tmp/scgi;
    map $http_upgrade $connection_upgrade {{ default upgrade; '' ''; }}
    server {{
        listen {bind} ssl;
        http2 on;
        server_name {domain};
        ssl_certificate @BUNDLE_DIR@/fullchain.pem;
        ssl_certificate_key @BUNDLE_DIR@/privkey.pem;
        client_max_body_size 16m;
        location / {{
            proxy_pass http://127.0.0.1:18080;
            proxy_http_version 1.1;
            proxy_set_header Host $host;
            proxy_set_header X-Forwarded-For $remote_addr;
            proxy_set_header Upgrade $http_upgrade;
            proxy_set_header Connection $connection_upgrade;
            proxy_buffering off;
            proxy_request_buffering off;
            proxy_next_upstream off;
            proxy_connect_timeout 5s;
            proxy_read_timeout 300s;
            proxy_send_timeout 300s;
        }}
    }}
}}
"#);
    Ok((toml,nginx))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document()->Value{json!({"native_port":8443,"users":[{"username":"alice","secret":"a".repeat(32)}]})}
    #[test] fn render_preserves_keys_and_exact_web_tuple(){
        let doc=validate(document()).unwrap();let (config,_)=render(&doc,"node.example","2001:db8::1","Bearer private","bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",&json!({"upload_bps":200000000,"download_bps":500000000})).unwrap();
        let cfg:toml::Value=toml::from_str(&config).unwrap();assert_eq!(cfg["web"]["vhosts"][0]["public_addr"].as_str(),Some("[2001:db8::1]:443"));assert_eq!(cfg["access"]["users"]["alice"].as_str(),Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"));assert!(cfg["access"]["user_rate_limits"].as_table().unwrap().is_empty());assert_eq!(cfg["shaping"]["up_bps"].as_integer(),Some(200000000));
    }
    #[test] fn profile_boundary_rejects_reserved_identity_and_bad_paths(){
        for path in ["a//b","_a","a/-b","../x"] {let mut doc=document();doc["base_path"]=json!(path);assert!(validate(doc).is_err());}
        let mut doc=document();doc["users"][0]["username"]=json!("fleet_probe");assert!(validate(doc).is_err());assert!(domain("node.123").is_err());
    }
}
