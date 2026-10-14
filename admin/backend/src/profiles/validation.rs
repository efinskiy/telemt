//! Run the same complete loader shipped in managed-node images before publishing.
use super::model::{ProfileDocument,render};
use anyhow::ensure;
use std::process::Stdio;

pub(super) async fn validate(doc:&ProfileDocument,domain:&str)->anyhow::Result<()> {
    let (config,_)=render(doc,domain,"192.0.2.1","Bearer validation-only",&"a".repeat(32),&serde_json::json!({})).map_err(|e|anyhow::anyhow!(e.1))?;
    let path=std::env::temp_dir().join(format!("telemt-profile-{}.toml",uuid::Uuid::new_v4()));
    let mut options=tokio::fs::OpenOptions::new();options.write(true).create_new(true);
    #[cfg(unix)] options.mode(0o600);
    let result=async {
        use tokio::io::AsyncWriteExt;
        let mut file=options.open(&path).await?;file.write_all(config.as_bytes()).await?;file.sync_all().await?;drop(file);
        let binary=std::env::var("TELEMT_VALIDATOR").unwrap_or_else(|_|"/app/telemt".into());
        let mut child=tokio::process::Command::new(binary).arg("check-config").arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn()?;
        ensure!(tokio::time::timeout(std::time::Duration::from_secs(8),child.wait()).await??.success(),"profile validation failed");Ok::<(),anyhow::Error>(())
    }.await;
    let _=tokio::fs::remove_file(path).await;result
}
