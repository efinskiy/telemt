use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::{Aead, Payload}};
use base64::{Engine, engine::general_purpose::STANDARD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub(crate) fn random() -> String { let mut bytes = [0u8;32]; rand::rng().fill_bytes(&mut bytes); hex::encode(bytes) }
pub(crate) fn hash(value: &str) -> String { hex::encode(Sha256::digest(value.as_bytes())) }
pub(crate) fn equal(a: &str, b: &str) -> bool { a.as_bytes().ct_eq(b.as_bytes()).into() }

pub(crate) struct Vault(Aes256Gcm);
impl Vault {
    pub(crate) fn new(bytes: &[u8]) -> anyhow::Result<Self> { Ok(Self(Aes256Gcm::new_from_slice(bytes).map_err(|_| anyhow::anyhow!("master key must have 32 bytes"))?)) }
    pub(crate) fn seal(&self, value: &str, purpose: &str) -> anyhow::Result<String> {
        let mut nonce = [0;12]; rand::rng().fill_bytes(&mut nonce);
        let cipher = self.0.encrypt(Nonce::from_slice(&nonce), Payload { msg: value.as_bytes(), aad: purpose.as_bytes() }).map_err(|_| anyhow::anyhow!("encryption failed"))?;
        let mut data = nonce.to_vec(); data.extend(cipher); Ok(STANDARD.encode(data))
    }
    pub(crate) fn open(&self, value: &str, purpose: &str) -> anyhow::Result<zeroize::Zeroizing<String>> {
        let bytes = STANDARD.decode(value)?;
        anyhow::ensure!(bytes.len() >= 28 && bytes.len() <= 2 * 1024 * 1024, "invalid encrypted record");
        let plain = self.0.decrypt(Nonce::from_slice(&bytes[..12]), Payload { msg: &bytes[12..], aad: purpose.as_bytes() }).map_err(|_| anyhow::anyhow!("authentication failed"))?;
        Ok(zeroize::Zeroizing::new(String::from_utf8(plain)?))
    }
}

pub(crate) fn totp(secret_hex: &str, step: u64) -> anyhow::Result<String> {
    let bytes = hex::decode(secret_hex)?;
    let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid TOTP key"))?;
    mac.update(&step.to_be_bytes()); let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 15) as usize;
    let number = u32::from_be_bytes(digest[offset..offset+4].try_into()?) & 0x7fff_ffff;
    Ok(format!("{:06}", number % 1_000_000))
}

pub(crate) fn base32(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut out = String::new(); let mut accumulator = 0u32; let mut bits = 0;
    for byte in bytes { accumulator = (accumulator << 8) | u32::from(*byte); bits += 8;
        while bits >= 5 { bits -= 5; out.push(ALPHABET[((accumulator >> bits) & 31) as usize] as char); }
    }
    if bits > 0 { out.push(ALPHABET[((accumulator << (5-bits)) & 31) as usize] as char); } out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn vault_binds_ciphertext_to_resource() { let vault = Vault::new(&[1;32]).unwrap(); let cipher = vault.seal("secret", "node-a").unwrap(); assert_eq!(*vault.open(&cipher,"node-a").unwrap(), "secret"); assert!(vault.open(&cipher,"node-b").is_err()); assert!(vault.open("AA==","node-a").is_err()); }
    #[test] fn totp_matches_rfc6238_vector() { assert_eq!(totp(&hex::encode(b"12345678901234567890"),59/30).unwrap(),"287082"); assert_eq!(base32(b"foo"),"MZXW6"); }
}
