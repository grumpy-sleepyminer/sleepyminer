use serde::{Deserialize, Serialize};

use crate::algo::Algorithm;

/// JSON-RPC request wrapper
#[derive(Debug, Serialize)]
pub struct JsonRpcRequest<T: Serialize> {
    pub id: u64,
    pub jsonrpc: &'static str,
    pub method: &'static str,
    pub params: T,
}

impl<T: Serialize> JsonRpcRequest<T> {
    pub fn new(id: u64, method: &'static str, params: T) -> Self {
        Self {
            id,
            jsonrpc: "2.0",
            method,
            params,
        }
    }
}

/// Login parameters
#[derive(Debug, Serialize)]
pub struct LoginParams {
    pub login: String,
    pub pass: String,
    pub agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rigid: Option<String>,
}

/// Share submission parameters
#[derive(Debug, Serialize)]
pub struct SubmitParams {
    pub id: String,
    pub job_id: String,
    pub nonce: String,
    pub result: String,
}

/// Keepalive parameters
#[derive(Debug, Serialize)]
pub struct KeepaliveParams {
    pub id: String,
}

/// Generic JSON-RPC response.
///
/// `id` is `i64` because NiceHash uses `-1` for unsolicited notifications
/// (e.g. mining.set_difficulty pushed by the pool with no matching request).
/// The id is opaque to the protocol — we only use it to correlate request
/// responses, so a signed integer is fine.
#[derive(Debug, Deserialize)]
pub struct JsonRpcResponse {
    pub id: Option<i64>,
    pub jsonrpc: Option<String>,
    pub method: Option<String>,
    pub result: Option<serde_json::Value>,
    pub params: Option<serde_json::Value>,
    pub error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub struct RpcError {
    pub code: Option<i64>,
    pub message: Option<String>,
}

/// Login response result
#[derive(Debug, Deserialize)]
pub struct LoginResult {
    pub id: String,
    pub job: JobParams,
    pub status: Option<String>,
}

/// Job notification from pool
#[derive(Debug, Clone, Deserialize)]
pub struct JobParams {
    pub blob: String,
    pub job_id: String,
    pub target: String,
    #[serde(default)]
    pub height: Option<u64>,
    #[serde(default)]
    pub seed_hash: Option<String>,
    #[serde(default)]
    pub algo: Option<String>,
    // NiceHash extra nonce fields
    #[serde(default)]
    pub extra_nonce: Option<String>,
}

/// Submit response result
#[derive(Debug, Deserialize)]
pub struct SubmitResult {
    pub status: Option<String>,
}

/// Parsed mining job ready for workers
#[derive(Debug, Clone)]
pub struct MiningJob {
    pub blob: Vec<u8>,
    pub job_id: String,
    pub target: u64,
    pub seed_hash: [u8; 32],
    pub height: u64,
    pub nicehash: bool,
    pub extra_nonce: Option<Vec<u8>>,
    pub nonce_offset: usize,
    /// Algorithm to use for hashing this job (inherited from the pool's config).
    pub algo: Algorithm,
}

impl MiningJob {
    pub fn from_params(
        params: &JobParams,
        nicehash: bool,
        algo: Algorithm,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let blob = hex::decode(&params.blob)?;

        // Nonce offset is algorithm-specific.
        let nonce_offset = algo.nonce_offset(blob.len());

        // Parse target - can be 4 or 8 bytes
        let target_bytes = hex::decode(&params.target)?;
        let target = match target_bytes.len() {
            4 => {
                let t = u32::from_le_bytes(target_bytes.try_into().unwrap());
                if t == 0 {
                    u64::MAX
                } else {
                    u64::MAX / (u32::MAX as u64 / t as u64)
                }
            }
            8 => u64::from_le_bytes(target_bytes.try_into().unwrap()),
            _ => return Err(format!("Invalid target length: {}", target_bytes.len()).into()),
        };

        let seed_hash = if let Some(ref sh) = params.seed_hash {
            let bytes = hex::decode(sh)?;
            if bytes.len() != 32 {
                return Err("Invalid seed_hash length".into());
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        } else {
            [0u8; 32]
        };

        let extra_nonce = params
            .extra_nonce
            .as_ref()
            .map(|n| hex::decode(n))
            .transpose()?;

        Ok(MiningJob {
            blob,
            job_id: params.job_id.clone(),
            target,
            seed_hash,
            height: params.height.unwrap_or(0),
            nicehash,
            extra_nonce,
            nonce_offset,
            algo,
        })
    }
}

pub const AGENT_STRING: &str = concat!("sleepyminer/", env!("CARGO_PKG_VERSION"));
