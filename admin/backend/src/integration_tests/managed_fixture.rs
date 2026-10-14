//! The fixture executes durable commands through the real authenticated managed polling route.
use super::*;

pub(super) async fn seed(state:&State,nodes:&[Uuid],secret:&str)->Uuid {
    let cert=Uuid::new_v4();let profile=Uuid::new_v4();
    let material:Value=serde_json::from_slice(&std::fs::read(std::env::var("TELEMT_TEST_CERT_MATERIAL").unwrap()).unwrap()).unwrap();
    let info=telemt_managed::certificate::validate(material["fullchain"].as_str().unwrap(),material["private_key"].as_str().unwrap(),"example.org").unwrap();
    let cipher=state.vault.seal(&material.to_string(),&format!("certificate:{cert}:1")).unwrap();
    sqlx::query("INSERT INTO certificates(id,name,domain,revision,material_cipher,info) VALUES($1,'fixture','example.org',1,$2,$3)").bind(cert).bind(cipher).bind(serde_json::to_value(&info).unwrap()).execute(&state.db).await.unwrap();
    let doc=json!({"native_port":8443,"users":[{"username":"alice","secret":secret}]});
    let draft=state.vault.seal(&doc.to_string(),&format!("profile:{profile}:draft")).unwrap();
    let probe=state.vault.seal(secret,&format!("profile-probe:{profile}")).unwrap();
    sqlx::query("INSERT INTO profiles(id,name,domain,certificate_id,draft_cipher,probe_cipher,published_revision) VALUES($1,'fixture','example.org',$2,$3,$4,1)").bind(profile).bind(cert).bind(draft).bind(probe).execute(&state.db).await.unwrap();
    let cipher=state.vault.seal(&doc.to_string(),&format!("profile:{profile}:1")).unwrap();
    sqlx::query("INSERT INTO profile_versions(profile_id,revision,document_cipher,actor) VALUES($1,1,$2,(SELECT id FROM administrators WHERE username='owner'))").bind(profile).bind(cipher).execute(&state.db).await.unwrap();
    for (index,id) in nodes.iter().enumerate(){
        let token=state.vault.seal("Bearer test-node-token",&format!("node:{id}")).unwrap();let probe=state.vault.seal(secret,&format!("probe:{id}")).unwrap();
        sqlx::query("INSERT INTO nodes(id,name,token_cipher,probe_cipher,document) VALUES($1,$2,$3,$4,$5)").bind(id).bind(if index==0{"primary"}else{"reserve"}).bind(token).bind(probe).bind(json!({"managed":true,"profile_id":profile,"ipv4":format!("192.0.2.{}",index+1),"ipv6":format!("2001:db8::{}",index+1),"port":8443,"sni":"example.org","probe_dc":2,"web_base_path":""})).execute(&state.db).await.unwrap();
        sqlx::query("INSERT INTO managed_nodes(node_id,profile_id,credential_hash,desired_revision,applied_revision,status,last_seen,report) VALUES($1,$2,$3,1,1,'applied',now(),$4)").bind(id).bind(profile).bind(crypto::hash(&"c".repeat(64))).bind(json!({"certificate_fingerprint":info.fingerprint})).execute(&state.db).await.unwrap();
    }profile
}

pub(super) fn start(state:State,app:Router,port:u16,nodes:Vec<Uuid>)->tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut receipts:HashMap<Uuid,Vec<Value>>=HashMap::new();
        loop {
            for id in &nodes {
                let response=app.clone().oneshot(Request::builder().method("POST").uri("/node-agent/v1/poll").header("authorization",format!("Bearer {}","c".repeat(64))).header("content-type","application/json").body(Body::from(json!({"node_id":id,"revision":i64::MAX,"receipts":receipts.remove(id).unwrap_or_default()}).to_string())).unwrap()).await.unwrap();
                if response.status()==StatusCode::UNAUTHORIZED {
                    assert!(!sqlx::query_scalar::<_,bool>("SELECT EXISTS(SELECT 1 FROM managed_nodes WHERE node_id=$1 AND NOT revoked)").bind(id).fetch_one(&state.db).await.unwrap());continue;
                }
                assert_eq!(response.status(),StatusCode::OK);
                let response:Value=serde_json::from_slice(&to_bytes(response.into_body(),1024*1024).await.unwrap()).unwrap();
                for command in response["data"]["commands"].as_array().unwrap(){
                    let command:telemt_managed::Command=serde_json::from_value(command.clone()).unwrap();
                    let mut request=state.client.request(reqwest::Method::from_bytes(command.method.as_bytes()).unwrap(),format!("http://127.0.0.1:{port}{}",command.path)).header("authorization","Bearer test-node-token");
                    if let Some(revision)=command.revision{request=request.header("if-match",revision);}
                    if let Some(body)=command.body{request=request.json(&body);}
                    let response=request.send().await.unwrap();let status=response.status().as_u16();let response:Value=response.json().await.unwrap();
                    receipts.entry(*id).or_default().push(json!({"id":command.id,"status":status,"response":response}));
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
}

// A child test process isolates test-only trust roots from concurrently running tests.
pub(super) async fn isolated_pki()->bool {
    if std::env::var_os("TELEMT_TEST_CERT_MATERIAL").is_some(){return false;}
    let dir=std::env::temp_dir().join(format!("telemt-test-pki-{}",Uuid::new_v4()));std::fs::create_dir(&dir).unwrap();
    let mut params=rcgen::CertificateParams::new(vec!["test-root.invalid".into()]).unwrap();params.is_ca=rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let root_key=rcgen::KeyPair::generate().unwrap();let root=params.self_signed(&root_key).unwrap();
    let issuer=rcgen::Issuer::from_params(&params,&root_key);
    let key=rcgen::KeyPair::generate().unwrap();let leaf=rcgen::CertificateParams::new(vec!["example.org".into()]).unwrap().signed_by(&key,&issuer).unwrap();
    let ca=dir.join("ca.pem");let material=dir.join("material.json");
    std::fs::write(&ca,root.pem()).unwrap();std::fs::write(&material,json!({"fullchain":format!("{}{}",leaf.pem(),root.pem()),"private_key":key.serialize_pem()}).to_string()).unwrap();
    let output=tokio::task::spawn_blocking(move||std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact","integration_tests::postgres_router_auth_operations_probes_and_dns","--ignored","--nocapture"]).env("MANAGED_CA_FILE",ca).env("TELEMT_TEST_CERT_MATERIAL",material).output().unwrap()).await.unwrap();
    std::fs::remove_dir_all(dir).unwrap();assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));true
}
