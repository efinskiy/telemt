//! Durable private state is written before acknowledging enrollment or executing commands.
use anyhow::{ensure,Context};
use std::path::{Path,PathBuf};
use tokio::io::{AsyncReadExt,AsyncWriteExt};
use serde::{Serialize,de::DeserializeOwned};
use telemt_managed::STATE_DIR;

pub(crate) fn path(name:&str)->PathBuf{Path::new(STATE_DIR).join(name)}
pub(crate) async fn init()->anyhow::Result<()> {
    tokio::fs::create_dir_all(STATE_DIR).await?;
    #[cfg(unix)] {use std::os::unix::fs::PermissionsExt;tokio::fs::set_permissions(STATE_DIR,std::fs::Permissions::from_mode(0o700)).await?;}
    Ok(())
}
pub(crate) async fn read(path:&Path,limit:usize)->anyhow::Result<Vec<u8>> {
    let meta=tokio::fs::metadata(path).await?;ensure!(meta.is_file()&&meta.len()<=limit as u64,"invalid state size");
    let file=tokio::fs::File::open(path).await?;let mut bytes=Vec::new();file.take(limit as u64+1).read_to_end(&mut bytes).await?;ensure!(bytes.len()<=limit,"state exceeds limit");Ok(bytes)
}
pub(crate) async fn load<T:DeserializeOwned>(name:&str)->anyhow::Result<Option<T>> {
    let file=path(name);if !tokio::fs::try_exists(&file).await?{return Ok(None);}
    Ok(Some(serde_json::from_slice(&read(&file,telemt_managed::MAX_BUNDLE_BYTES).await?)?))
}
pub(crate) async fn save<T:Serialize>(name:&str,value:&T)->anyhow::Result<()> {write(&path(name),&serde_json::to_vec(value)?).await}
pub(crate) async fn write(path:&Path,bytes:&[u8])->anyhow::Result<()> {
    ensure!(bytes.len()<=telemt_managed::MAX_BUNDLE_BYTES,"state exceeds limit");
    let tmp=path.with_extension("tmp");let mut options=tokio::fs::OpenOptions::new();options.write(true).create(true).truncate(true);
    #[cfg(unix)] options.mode(0o600);
    let mut file=options.open(&tmp).await?;file.write_all(bytes).await?;file.sync_all().await?;drop(file);tokio::fs::rename(tmp,path).await?;
    let parent=path.parent().context("state parent missing")?.to_owned();
    tokio::task::spawn_blocking(move||std::fs::File::open(parent)?.sync_all()).await??;Ok(())
}
