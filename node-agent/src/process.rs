//! Bounded child lifecycle and validated activation with rollback to the last committed bundle.
use crate::{storage,local};
use anyhow::{ensure,Context};
use telemt_managed::Bundle;
use tokio::process::{Child,Command};
use std::{path::PathBuf,process::Stdio,time::Duration};

pub(crate) struct Services { pub(crate) bundle:Option<Bundle>,telemt:Option<Child>,nginx:Option<Child> }
impl Services {
    pub(crate) fn new()->Self{Self{bundle:None,telemt:None,nginx:None}}
    pub(crate) async fn activate(&mut self,bundle:Bundle,client:&reqwest::Client)->anyhow::Result<()> {
        ensure!(bundle.node_id==std::env::var("NODE_ID")?,"bundle belongs to another node");
        ensure!(bundle.revision>0,"invalid revision");
        telemt_managed::certificate::validate(&bundle.fullchain,&bundle.private_key,&bundle.domain)?;
        let directory=directory(&bundle);
        let validation=async {prepare(&bundle).await?;
        check("/app/telemt",&["check-config",directory.join("telemt.toml").to_str().context("path")?]).await.context("telemt_validation_failed")?;
        check("/usr/sbin/nginx",&["-t","-c",directory.join("nginx.conf").to_str().context("path")?]).await.context("nginx_validation_failed")?;Ok::<(),anyhow::Error>(())}.await;
        if let Err(error)=validation {
            if self.bundle.as_ref().is_none_or(|active|active.revision!=bundle.revision){let _=tokio::fs::remove_dir_all(&directory).await;}
            return Err(error);
        }
        let previous=self.bundle.clone();
        // Persist the previous committed state before touching either live process.
        storage::save("pending.json",&bundle).await?;
        let result=async {self.start(&bundle,client).await?;storage::save("active.json",&bundle).await}.await;
        if let Err(error)=result {
            self.stop().await;
            if let Some(old)=previous {self.start(&old,client).await.context("rollback failed")?;self.bundle=Some(old);}
            if self.bundle.as_ref().is_none_or(|active|active.revision!=bundle.revision){let _=tokio::fs::remove_dir_all(&directory).await;}
            return Err(error);
        }
        self.bundle=Some(bundle);
        let _=tokio::fs::remove_file(storage::path("pending.json")).await;
        // Retention failure must not turn an already committed healthy deployment into a failure.
        if self.prune().await.is_err(){eprintln!("bundle retention failed");}Ok(())
    }
    async fn start(&mut self,bundle:&Bundle,client:&reqwest::Client)->anyhow::Result<()> {
        let directory=directory(bundle);let config=storage::path("telemt.toml");
        let certificate_only=self.telemt.is_some()&&self.nginx.is_some()&&self.bundle.as_ref().is_some_and(|old|old.config==bundle.config&&old.nginx==bundle.nginx)
            &&storage::read(&config,telemt_managed::MAX_BUNDLE_BYTES).await.is_ok_and(|bytes|bytes==bundle.config.as_bytes());
        if !certificate_only {
            stop(&mut self.telemt,"TERM").await;
            storage::write(&config,bundle.config.as_bytes()).await?;
        }
        // nginx's master keeps one fixed configuration pathname across HUP reloads.
        storage::write(&storage::path("nginx.conf"),bundle.nginx.replace("@BUNDLE_DIR@",directory.to_str().context("path")?).as_bytes()).await?;
        if let Some(nginx)=&mut self.nginx {signal(nginx,"HUP").await?;}else{self.nginx=Some(Command::new("/usr/sbin/nginx").args(["-c",storage::path("nginx.conf").to_str().context("path")?,"-g","daemon off;"]).stdin(Stdio::null()).kill_on_drop(true).spawn()?);}
        if self.telemt.is_none(){self.telemt=Some(Command::new("/app/telemt").arg(&config).current_dir(telemt_managed::STATE_DIR).stdin(Stdio::null()).kill_on_drop(true).spawn()?);}
        tokio::time::timeout(Duration::from_secs(40),async {
            loop {ensure!(self.alive()?,"service exited during activation");if local::ready(client,bundle).await.is_ok()&&local::public_tls(bundle).await.is_ok(){return Ok::<(),anyhow::Error>(());}tokio::time::sleep(Duration::from_secs(1)).await;}
        }).await??;Ok(())
    }
    pub(crate) fn alive(&mut self)->anyhow::Result<bool>{
        for child in [&mut self.telemt,&mut self.nginx] {if child.as_mut().is_none_or(|c|c.try_wait().ok().flatten().is_some()){return Ok(false);}}
        Ok(true)
    }
    pub(crate) async fn drift(&self)->anyhow::Result<bool>{
        let Some(bundle)=&self.bundle else{return Ok(false);};let directory=directory(bundle);
        if storage::read(&storage::path("telemt.toml"),telemt_managed::MAX_BUNDLE_BYTES).await?!=bundle.config.as_bytes(){return Ok(true);}
        for (name,expected) in [("telemt.toml",bundle.config.as_str()),("fullchain.pem",bundle.fullchain.as_str()),("privkey.pem",bundle.private_key.as_str())] {
            if storage::read(&directory.join(name),telemt_managed::MAX_BUNDLE_BYTES).await?!=expected.as_bytes(){return Ok(true);}
        }
        let nginx=bundle.nginx.replace("@BUNDLE_DIR@",directory.to_str().context("path")?);
        Ok(storage::read(&storage::path("nginx.conf"),telemt_managed::MAX_BUNDLE_BYTES).await?!=nginx.as_bytes())
    }
    pub(crate) async fn stop(&mut self){stop(&mut self.nginx,"QUIT").await;stop(&mut self.telemt,"TERM").await;}
    async fn prune(&self)->anyhow::Result<()> {
        let mut entries=tokio::fs::read_dir(storage::path("bundles")).await?;let mut dirs=Vec::new();
        while let Some(entry)=entries.next_entry().await?{ensure!(dirs.len()<1024,"bundle directory limit");if let Some(name)=entry.file_name().to_str() && let Ok(id)=name.parse::<i64>(){dirs.push((id,entry.path()));}}
        dirs.sort_by_key(|(id,_)|std::cmp::Reverse(*id));for (_,path) in dirs.into_iter().skip(3){tokio::fs::remove_dir_all(path).await?;}Ok(())
    }
}
fn directory(bundle:&Bundle)->PathBuf{storage::path("bundles").join(bundle.revision.to_string())}
async fn prepare(bundle:&Bundle)->anyhow::Result<PathBuf>{
    let dir=directory(bundle);tokio::fs::create_dir_all(&dir).await?;
    for (name,bytes) in [("telemt.toml",bundle.config.as_bytes()),("fullchain.pem",bundle.fullchain.as_bytes()),("privkey.pem",bundle.private_key.as_bytes())]{storage::write(&dir.join(name),bytes).await?;}
    storage::write(&dir.join("nginx.conf"),bundle.nginx.replace("@BUNDLE_DIR@",dir.to_str().context("path")?).as_bytes()).await?;Ok(dir)
}
async fn check(program:&str,args:&[&str])->anyhow::Result<()> {
    // Validation output can include configuration contents; only return a stable failure category.
    let mut child=Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).kill_on_drop(true).spawn()?;
    ensure!(tokio::time::timeout(Duration::from_secs(15),child.wait()).await??.success(),"bundle validation failed");Ok(())
}
async fn signal(child:&mut Child,name:&str)->anyhow::Result<()> {
    if let Some(id)=child.id(){ensure!(Command::new("/bin/kill").args([format!("-{name}"),id.to_string()]).status().await?.success(),"child signal failed");}Ok(())
}
async fn stop(child:&mut Option<Child>,name:&str){
    if let Some(mut process)=child.take(){let _=signal(&mut process,name).await;if tokio::time::timeout(Duration::from_secs(20),process.wait()).await.is_err(){let _=process.kill().await;}let _=process.wait().await;}
}
