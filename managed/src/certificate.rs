//! Validate imported material without executing shell commands or disclosing private keys.
use anyhow::{Context, ensure};
use rustls::sign::CertifiedKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use x509_parser::prelude::*;

/// Public certificate facts safe to expose in monitoring and audit.
#[derive(Clone, Serialize, Deserialize)]
pub struct CertificateInfo {
    /// SHA-256 of the leaf DER certificate for deployment readback.
    pub fingerprint: String,
    /// Inclusive validity start as Unix seconds.
    pub not_before: i64,
    /// Validity end as Unix seconds.
    pub not_after: i64,
}
/// Require a matching key, a currently valid leaf, and exact or single-label wildcard SAN coverage.
pub fn validate(fullchain: &str, private_key: &str, domain: &str) -> anyhow::Result<CertificateInfo> {
    ensure!(fullchain.len() <= 128 * 1024 && private_key.len() <= 32 * 1024, "certificate material exceeds size limit");
    let chain = rustls_pemfile::certs(&mut fullchain.as_bytes()).collect::<Result<Vec<_>, _>>()?;
    ensure!(!chain.is_empty() && chain.len() <= 10, "invalid certificate chain length");
    let key = rustls_pemfile::private_key(&mut private_key.as_bytes())?.context("private key missing")?;
    let certified = CertifiedKey::from_der(chain, key, &rustls::crypto::ring::default_provider())?;
    certified.keys_match()?;
    use rustls::client::danger::ServerCertVerifier;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    // Private test PKI is opt-in via an operator-controlled file, never a bundle field.
    if let Ok(path) = std::env::var("MANAGED_CA_FILE") {
        use std::io::Read;
        let file = std::fs::File::open(path)?;
        let mut pem = Vec::new(); file.take(128 * 1024 + 1).read_to_end(&mut pem)?;
        ensure!(pem.len() <= 128 * 1024, "CA file exceeds limit");
        for cert in rustls_pemfile::certs(&mut pem.as_slice()) { roots.add(cert?)?; }
    }
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(std::sync::Arc::new(roots), std::sync::Arc::new(rustls::crypto::ring::default_provider())).build()?;
    verifier.verify_server_cert(&certified.cert[0], &certified.cert[1..], &rustls::pki_types::ServerName::try_from(domain.to_owned())?, &[], rustls::pki_types::UnixTime::now())?;
    let (_, leaf) = X509Certificate::from_der(certified.cert[0].as_ref())?;
    ensure!(leaf.validity().is_valid(), "certificate is not currently valid");
    let san = leaf.subject_alternative_name()?.context("certificate SAN missing")?;
    let covered = san.value.general_names.iter().any(|name| match name {
        GeneralName::DNSName(name) => name.eq_ignore_ascii_case(domain) || name.strip_prefix("*.").is_some_and(|suffix| domain.split_once('.').is_some_and(|(_, tail)| tail.eq_ignore_ascii_case(suffix))),
        _ => false,
    });
    ensure!(covered, "certificate does not cover the profile domain");
    Ok(CertificateInfo { fingerprint: hex::encode(Sha256::digest(certified.cert[0].as_ref())), not_before: leaf.validity().not_before.timestamp(), not_after: leaf.validity().not_after.timestamp() })
}
