//! Bounded deployment contracts shared by Fleet and its managed-node supervisor.
pub mod certificate;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Maximum authenticated deployment response, including configuration and certificate chain.
pub const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;
/// Managed state is isolated from the host's configuration and Docker daemon.
pub const STATE_DIR: &str = "/var/lib/telemt-managed";
/// Local management listener; never exposed by the generated nginx configuration.
pub const API_ADDR: &str = "127.0.0.1:19091";

/// Immutable desired deployment. Secrets are delivered only over authenticated HTTPS.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Bundle {
    /// Monotonic deployment identity assigned by Fleet.
    pub revision: i64,
    /// Published profile version included in this deployment.
    pub profile_revision: i64,
    /// Certificate material version included in this deployment.
    pub certificate_revision: i64,
    /// Node identity that is authorized to apply this bundle.
    pub node_id: String,
    /// Shared public TLS and proxy domain.
    pub domain: String,
    /// Assigned local address used for the exact public endpoint tuple.
    pub public_ip: String,
    /// Complete validated telemt configuration.
    pub config: String,
    /// Complete nginx configuration with the bundle directory substitution marker.
    pub nginx: String,
    /// PEM certificate chain served to clients.
    pub fullchain: String,
    /// PEM private key, never included in public diagnostics.
    pub private_key: String,
    /// Individual authorization value for the loopback Control API.
    pub api_token: String,
}
impl Bundle {
    /// Digest binds acknowledgement to the exact persisted deployment, including its credentials.
    pub fn digest(&self) -> anyhow::Result<String> {
        Ok(hex::encode(Sha256::digest(serde_json::to_vec(self)?)))
    }
}

/// A single allowlisted local Control API call with a durable delivery identity.
#[derive(Clone, Serialize, Deserialize)]
pub struct Command {
    /// Durable identity used to correlate delivery and acknowledgment.
    /// Durable identity used to correlate delivery and acknowledgment.
    pub id: String,
    /// Allowlisted HTTP method for the local Control API.
    pub method: String,
    /// Relative allowlisted Control API path.
    pub path: String,
    /// Optional JSON request body.
    pub body: Option<Value>,
    /// Expected Control API configuration revision, when required.
    pub revision: Option<String>,
}
/// Replaying this receipt is safe; replaying its command after an uncertain mutation is not.
#[derive(Clone, Serialize, Deserialize)]
pub struct Receipt {
    /// Durable identity used to correlate delivery and acknowledgment.
    /// Durable identity used to correlate delivery and acknowledgment.
    pub id: String,
    /// HTTP result status; uncertain execution is reported explicitly.
    pub status: u16,
    /// Bounded JSON response or a structured uncertainty error.
    pub response: Value,
}

/// Defense in depth: the agent never follows controller-supplied URLs or arbitrary commands.
pub fn allowed_command(method: &str, path: &str) -> bool {
    if path.len() > 512 || !path.starts_with("/v1/") || path.contains(['#', '\\', '%']) || path.contains("..") { return false; }
    let raw = path.split('?').next().unwrap_or("");
    match method {
        "GET" => ["/v1/config", "/v1/users", "/v1/health", "/v1/health/ready", "/v1/system/info", "/v1/fleet/compatibility", "/v1/fleet/traffic", "/v1/limits/effective"].contains(&raw)
            || raw.starts_with("/v1/runtime/") || raw.starts_with("/v1/stats/") || raw.starts_with("/v1/users/"),
        "POST" => ["/v1/runtime/web/sessions/close", "/v1/runtime/web/debug/clear", "/v1/runtime/web/carrier-learning/reset", "/v1/runtime/web/lifecycle/pause", "/v1/runtime/web/lifecycle/drain", "/v1/runtime/web/lifecycle/resume"].contains(&raw)
            || (raw.starts_with("/v1/users/") && raw.ends_with("/reset-quota") && raw.split('/').count() == 5),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn commands_cannot_escape_loopback_or_mutate_profile(){
        for path in ["https://other/v1/users","/v1/../config","/v1/%2e%2e/config","/v1/config#x"]{assert!(!allowed_command("GET",path));}
        assert!(!allowed_command("PATCH","/v1/config"));assert!(!allowed_command("POST","/v1/users"));assert!(allowed_command("POST","/v1/users/alice/reset-quota"));
    }
}
