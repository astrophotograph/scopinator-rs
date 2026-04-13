//! Authentication handshake for Seestar firmware 7.18+ (version_int >= 2718).
//!
//! Protocol: get_verify_str → sign(RSA-PKCS1v15-SHA1) → verify_client → pi_is_verified
//!
//! The handshake runs on the raw [`TcpStream`] before it is split into reader/writer
//! halves, blocking all user commands until authentication completes.
//!
//! # Interoperability Notice
//!
//! The [`InteropKey`] and the challenge/response handshake implemented here are used for
//! interoperability purposes under 17 U.S.C. § 1201(f) (the DMCA interoperability
//! exemption), enabling independent programs to interoperate with Seestar devices via
//! the challenge-response authentication required by firmware 7.18+.
//!
//! The legality of key use varies by jurisdiction. You are solely responsible for ensuring
//! compliance with the laws of your region.

use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rsa::RsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use sha1::Sha1;
use signature::{Signer, SignatureEncoding};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tracing::{debug, info, warn};

use crate::error::SeestarError;

/// Timeout for each individual read during the auth handshake.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// Fixed JSON-RPC IDs for auth messages — well below INITIAL_COMMAND_ID (100_000).
const ID_GET_VERIFY: u64 = 1001;
const ID_VERIFY_CLIENT: u64 = 1002;
const ID_PI_IS_VERIFIED: u64 = 1003;

/// A loaded RSA signing key, ready to produce challenge signatures.
///
/// Constructed from a PEM file via [`InteropKey::from_pem`]. Cheap to clone
/// (the inner key is `Arc`-wrapped).
#[derive(Clone)]
pub struct InteropKey(Arc<SigningKey<Sha1>>);

impl InteropKey {
    /// Load an RSA private key from a PEM string.
    ///
    /// Detects format by header:
    /// - `BEGIN PRIVATE KEY`     → PKCS#8  (standard output of `openssl genpkey`)
    /// - `BEGIN RSA PRIVATE KEY` → PKCS#1  (output of legacy `openssl genrsa`)
    ///
    /// Returns `SeestarError::InteropKeyLoad` with a descriptive message on failure.
    pub fn from_pem(pem: &str) -> Result<Self, SeestarError> {
        let private_key = if pem.contains("BEGIN PRIVATE KEY") {
            // PKCS#8 unencrypted
            use rsa::pkcs8::DecodePrivateKey;
            RsaPrivateKey::from_pkcs8_pem(pem)
                .map_err(|e| SeestarError::InteropKeyLoad(format!("PKCS#8 parse failed: {e}")))?
        } else if pem.contains("BEGIN RSA PRIVATE KEY") {
            // PKCS#1 (traditional RSA key)
            use rsa::pkcs1::DecodeRsaPrivateKey;
            RsaPrivateKey::from_pkcs1_pem(pem)
                .map_err(|e| SeestarError::InteropKeyLoad(format!("PKCS#1 parse failed: {e}")))?
        } else {
            return Err(SeestarError::InteropKeyLoad(
                "unrecognized PEM header — expected 'BEGIN PRIVATE KEY' (PKCS#8) \
                 or 'BEGIN RSA PRIVATE KEY' (PKCS#1)"
                    .to_string(),
            ));
        };

        let signing_key = SigningKey::<Sha1>::new(private_key);
        Ok(Self(Arc::new(signing_key)))
    }
}

/// Perform the firmware 7.18+ challenge/response handshake on a freshly connected stream.
///
/// Must be called **before** the stream is split into reader/writer halves.
/// Returns `Ok(())` on success, or an error that the caller should treat as a
/// connection failure (triggering a reconnect).
pub async fn authenticate(stream: &mut TcpStream, key: &InteropKey) -> Result<(), SeestarError> {
    info!("starting authentication handshake");

    // Step 1: request a challenge string.
    send_raw(
        stream,
        serde_json::json!({
            "id": ID_GET_VERIFY,
            "method": "get_verify_str",
        }),
    )
    .await?;

    let resp1 = read_response(stream).await?;
    let challenge = extract_challenge(&resp1)?;
    debug!(challenge, "received auth challenge");

    // Step 2: sign and verify.
    let sig = sign_challenge(key, &challenge)?;

    send_raw(
        stream,
        serde_json::json!({
            "id": ID_VERIFY_CLIENT,
            "method": "verify_client",
            "params": { "sign": sig, "data": challenge },
        }),
    )
    .await?;

    let resp2 = read_response(stream).await?;
    check_verify_client(&resp2)?;

    // Step 3: sanity-check (non-fatal).
    send_raw(
        stream,
        serde_json::json!({
            "id": ID_PI_IS_VERIFIED,
            "method": "pi_is_verified",
        }),
    )
    .await?;

    let resp3 = read_response(stream).await?;
    let is_verified = resp3
        .get("result")
        .and_then(|r| r.get("is_verified"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !is_verified {
        warn!("pi_is_verified returned non-verified (non-fatal), proceeding");
    } else {
        debug!("pi_is_verified confirmed");
    }

    info!("authentication successful");
    Ok(())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Write a JSON value as a single `\r\n`-terminated line.
async fn send_raw(stream: &mut TcpStream, msg: serde_json::Value) -> Result<(), SeestarError> {
    let mut line = msg.to_string();
    line.push_str("\r\n");
    stream
        .write_all(line.as_bytes())
        .await
        .map_err(SeestarError::Connection)
}

/// Read one newline-terminated JSON line with a timeout.
async fn read_response(stream: &mut TcpStream) -> Result<serde_json::Value, SeestarError> {
    // We need a BufReader for line-oriented reading. Re-wrapping the mutable
    // reference is safe here because we do not split the stream.
    use tokio::io::AsyncBufReadExt;
    let mut buf = String::new();

    let result = tokio::time::timeout(AUTH_TIMEOUT, async {
        // SAFETY: We borrow stream mutably; BufReader does not outlive this block.
        let mut reader = BufReader::new(&mut *stream);
        reader.read_line(&mut buf).await
    })
    .await;

    match result {
        Ok(Ok(0)) => Err(SeestarError::AuthFailed(
            "connection closed during handshake".to_string(),
        )),
        Ok(Ok(_)) => {
            let trimmed = buf.trim();
            serde_json::from_str(trimmed).map_err(|e| {
                SeestarError::AuthFailed(format!("invalid JSON in handshake response: {e}"))
            })
        }
        Ok(Err(e)) => Err(SeestarError::Connection(e)),
        Err(_) => Err(SeestarError::AuthFailed(
            "timed out waiting for handshake response".to_string(),
        )),
    }
}

/// Extract the challenge string from a `get_verify_str` response.
fn extract_challenge(msg: &serde_json::Value) -> Result<String, SeestarError> {
    let challenge = msg
        .get("result")
        .and_then(|r| r.get("str"))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            SeestarError::AuthFailed(format!(
                "get_verify_str response missing result.str: {msg}"
            ))
        })?;
    Ok(challenge.to_string())
}

/// Sign the challenge string with RSA-PKCS1v15-SHA1, return base64.
fn sign_challenge(key: &InteropKey, challenge: &str) -> Result<String, SeestarError> {
    let sig: rsa::pkcs1v15::Signature = key
        .0
        .sign(challenge.as_bytes());
    Ok(BASE64.encode(sig.to_bytes()))
}

/// Check that `verify_client` was accepted (code == 0).
fn check_verify_client(msg: &serde_json::Value) -> Result<(), SeestarError> {
    let code = msg
        .get("code")
        .and_then(|v| v.as_i64())
        .or_else(|| msg.get("result").and_then(|v| v.as_i64()))
        .unwrap_or(-1);

    if code == 0 {
        Ok(())
    } else {
        Err(SeestarError::AuthFailed(format!(
            "verify_client rejected by telescope (code={code})"
        )))
    }
}
