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

impl std::fmt::Debug for InteropKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InteropKey").finish_non_exhaustive()
    }
}

impl InteropKey {
    /// Load an RSA private key from a PEM string.
    ///
    /// Detects format by header:
    /// - `BEGIN PRIVATE KEY`     → PKCS#8  (standard output of `openssl genpkey`)
    /// - `BEGIN RSA PRIVATE KEY` → PKCS#1  (output of legacy `openssl genrsa`)
    ///
    /// Returns `SeestarError::InteropKeyLoad` with a descriptive message on failure.
    pub fn from_pem(pem: &str) -> Result<Self, SeestarError> {
        let private_key = if pem.contains("BEGIN ENCRYPTED PRIVATE KEY") {
            // Encrypted PKCS#8 — password-protected keys are not supported.
            return Err(SeestarError::InteropKeyLoad(
                "encrypted PEM keys are not supported — \
                 use an unencrypted key (remove the passphrase with \
                 `openssl pkcs8 -in enc.pem -out plain.pem`)"
                    .to_string(),
            ));
        } else if pem.contains("BEGIN PRIVATE KEY") {
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
    let sig = sign_challenge(key, &challenge);

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

    // Step 3: sanity-check (non-fatal). Errors here do not fail authentication —
    // verify_client already succeeded, so the telescope accepted our key.
    let pi_result = async {
        send_raw(
            stream,
            serde_json::json!({
                "id": ID_PI_IS_VERIFIED,
                "method": "pi_is_verified",
            }),
        )
        .await?;
        read_response(stream).await
    }
    .await;

    match pi_result {
        Ok(resp3) => {
            let is_verified = resp3
                .get("result")
                .and_then(|r| r.get("is_verified"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if is_verified {
                debug!("pi_is_verified confirmed");
            } else {
                warn!("pi_is_verified returned non-verified (non-fatal), proceeding");
            }
        }
        Err(e) => {
            warn!("pi_is_verified check failed (non-fatal): {e}");
        }
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
        // BufReader borrows stream mutably and is dropped at the end of this block,
        // so it cannot outlive the stream reference.
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
fn sign_challenge(key: &InteropKey, challenge: &str) -> String {
    let sig: rsa::pkcs1v15::Signature = key.0.sign(challenge.as_bytes());
    BASE64.encode(sig.to_bytes())
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    use rand::rngs::OsRng;
    use rsa::RsaPrivateKey;
    use rsa::pkcs1v15::VerifyingKey;
    use rsa::pkcs8::EncodePrivateKey;
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use signature::Verifier;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    /// Generate a 512-bit RSA key for tests (small = fast; not for production use).
    fn make_test_private_key() -> RsaPrivateKey {
        RsaPrivateKey::new(&mut OsRng, 512).unwrap()
    }

    fn interop_key_from_private(private_key: &RsaPrivateKey) -> InteropKey {
        InteropKey(Arc::new(SigningKey::<Sha1>::new(private_key.clone())))
    }

    // ── InteropKey::from_pem ──────────────────────────────────────────────────

    #[test]
    fn from_pem_pkcs8_roundtrip() {
        let private_key = make_test_private_key();
        let pem = private_key
            .to_pkcs8_pem(rsa::pkcs8::LineEnding::LF)
            .unwrap();
        assert!(InteropKey::from_pem(pem.as_str()).is_ok());
    }

    #[test]
    fn from_pem_pkcs1_roundtrip() {
        let private_key = make_test_private_key();
        let pem = private_key
            .to_pkcs1_pem(rsa::pkcs1::LineEnding::LF)
            .unwrap();
        assert!(InteropKey::from_pem(pem.as_str()).is_ok());
    }

    #[test]
    fn from_pem_encrypted_pkcs8_rejected() {
        // Encrypted PKCS#8 header must produce a clear error, not a cryptic parse failure.
        let pem = "-----BEGIN ENCRYPTED PRIVATE KEY-----\ndGVzdA==\n-----END ENCRYPTED PRIVATE KEY-----\n";
        let err = InteropKey::from_pem(pem).unwrap_err();
        assert!(matches!(err, SeestarError::InteropKeyLoad(_)));
        assert!(err.to_string().contains("encrypted"), "error should mention encryption");
    }

    #[test]
    fn from_pem_unrecognized_header() {
        let pem = "-----BEGIN EC PRIVATE KEY-----\ndGVzdA==\n-----END EC PRIVATE KEY-----\n";
        assert!(matches!(
            InteropKey::from_pem(pem),
            Err(SeestarError::InteropKeyLoad(_))
        ));
    }

    #[test]
    fn from_pem_empty_input() {
        assert!(matches!(
            InteropKey::from_pem(""),
            Err(SeestarError::InteropKeyLoad(_))
        ));
    }

    #[test]
    fn from_pem_malformed_pkcs8_body() {
        let pem = "-----BEGIN PRIVATE KEY-----\nbm90YWtleQ==\n-----END PRIVATE KEY-----\n";
        assert!(matches!(
            InteropKey::from_pem(pem),
            Err(SeestarError::InteropKeyLoad(_))
        ));
    }

    #[test]
    fn from_pem_malformed_pkcs1_body() {
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nbm90YWtleQ==\n-----END RSA PRIVATE KEY-----\n";
        assert!(matches!(
            InteropKey::from_pem(pem),
            Err(SeestarError::InteropKeyLoad(_))
        ));
    }

    // ── extract_challenge ─────────────────────────────────────────────────────

    #[test]
    fn extract_challenge_ok() {
        let msg = serde_json::json!({"result": {"str": "abc123"}});
        assert_eq!(extract_challenge(&msg).unwrap(), "abc123");
    }

    #[test]
    fn extract_challenge_missing_str_field() {
        let msg = serde_json::json!({"result": {}});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn extract_challenge_empty_str() {
        let msg = serde_json::json!({"result": {"str": ""}});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn extract_challenge_str_is_null() {
        // str field present but JSON null — as_str() returns None.
        let msg = serde_json::json!({"result": {"str": null}});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn extract_challenge_result_is_null() {
        // result field present but JSON null — get("str") returns None.
        let msg = serde_json::json!({"result": null});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn extract_challenge_result_is_scalar() {
        // Telescope sends result as an integer instead of a dict.
        let msg = serde_json::json!({"result": 0});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn extract_challenge_missing_result() {
        let msg = serde_json::json!({"code": 0});
        assert!(matches!(
            extract_challenge(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    // ── check_verify_client ───────────────────────────────────────────────────

    #[test]
    fn check_code_zero_ok() {
        let msg = serde_json::json!({"id": 1002, "code": 0});
        assert!(check_verify_client(&msg).is_ok());
    }

    #[test]
    fn check_result_zero_ok() {
        // Some firmware variants return result instead of code.
        let msg = serde_json::json!({"id": 1002, "result": 0});
        assert!(check_verify_client(&msg).is_ok());
    }

    #[test]
    fn check_nonzero_code_err() {
        let msg = serde_json::json!({"id": 1002, "code": 1001});
        assert!(matches!(
            check_verify_client(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn check_negative_code_err() {
        let msg = serde_json::json!({"id": 1002, "code": -1});
        assert!(matches!(
            check_verify_client(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn check_null_code_falls_through_to_result_err() {
        // code field present but null — as_i64() returns None, falls to result,
        // which is also absent, so defaults to -1.
        let msg = serde_json::json!({"id": 1002, "code": null});
        assert!(matches!(
            check_verify_client(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn check_string_code_not_accepted() {
        // code as a string "0" rather than integer — as_i64() returns None,
        // falls through to result which is absent, defaults to -1.
        let msg = serde_json::json!({"id": 1002, "code": "0"});
        assert!(matches!(
            check_verify_client(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    #[test]
    fn check_missing_code_and_result_err() {
        let msg = serde_json::json!({"id": 1002, "method": "verify_client"});
        assert!(matches!(
            check_verify_client(&msg),
            Err(SeestarError::AuthFailed(_))
        ));
    }

    // ── sign_challenge ────────────────────────────────────────────────────────

    #[test]
    fn sign_challenge_produces_verifiable_signature() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);
        let challenge = "test-challenge-string-42";

        let sig_b64 = sign_challenge(&key, challenge);

        // Must decode as valid base64.
        let sig_bytes = BASE64.decode(&sig_b64).expect("base64 decode failed");

        // Signature must verify against the corresponding public key.
        let verifying_key = VerifyingKey::<Sha1>::new(private_key.to_public_key());
        let sig = rsa::pkcs1v15::Signature::try_from(sig_bytes.as_slice())
            .expect("signature decode failed");
        verifying_key
            .verify(challenge.as_bytes(), &sig)
            .expect("signature verification failed");
    }

    #[test]
    fn sign_challenge_empty_string() {
        // Signing an empty challenge must not panic and must produce a verifiable sig.
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let sig_b64 = sign_challenge(&key, "");
        let sig_bytes = BASE64.decode(&sig_b64).unwrap();
        let verifying_key = VerifyingKey::<Sha1>::new(private_key.to_public_key());
        let sig = rsa::pkcs1v15::Signature::try_from(sig_bytes.as_slice()).unwrap();
        verifying_key.verify(b"", &sig).expect("empty-challenge sig must verify");
    }

    #[test]
    fn sign_challenge_wrong_key_does_not_verify() {
        // A signature from key A must not verify under key B.
        let private_key_a = make_test_private_key();
        let private_key_b = make_test_private_key();
        let key_a = interop_key_from_private(&private_key_a);
        let challenge = "some-challenge";

        let sig_b64 = sign_challenge(&key_a, challenge);
        let sig_bytes = BASE64.decode(&sig_b64).unwrap();

        let verifying_key_b = VerifyingKey::<Sha1>::new(private_key_b.to_public_key());
        let sig = rsa::pkcs1v15::Signature::try_from(sig_bytes.as_slice()).unwrap();
        assert!(
            verifying_key_b.verify(challenge.as_bytes(), &sig).is_err(),
            "signature from key A must not verify under key B"
        );
    }

    #[test]
    fn sign_challenge_is_deterministic() {
        // RSA-PKCS1v15 is deterministic: same key + same input = same output.
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);
        let challenge = "deterministic-test";

        let sig1 = sign_challenge(&key, challenge);
        let sig2 = sign_challenge(&key, challenge);
        assert_eq!(sig1, sig2, "PKCS1v15 signing must be deterministic");
    }

    // ── authenticate() integration tests ─────────────────────────────────────
    //
    // Each test binds a local TCP listener that plays the telescope role,
    // then runs authenticate() on the client side and checks the outcome.

    /// Simulate a telescope that runs through the full happy-path handshake.
    /// Also verifies that the signature sent by the client is cryptographically valid.
    #[tokio::test]
    async fn authenticate_success() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);
        let challenge = "hello-seestar-42";

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let challenge_str = challenge.to_string();
        let public_key = private_key.to_public_key();
        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();

            // Step 1: receive get_verify_str, send challenge.
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["method"], "get_verify_str");

            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"str": challenge_str},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            // Step 2: receive verify_client, check signature, accept.
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["method"], "verify_client");
            assert_eq!(req["params"]["data"], challenge_str);

            let sig_b64 = req["params"]["sign"].as_str().unwrap();
            let sig_bytes = BASE64.decode(sig_b64).unwrap();
            let verifying_key = VerifyingKey::<Sha1>::new(public_key);
            let sig = rsa::pkcs1v15::Signature::try_from(sig_bytes.as_slice()).unwrap();
            verifying_key
                .verify(challenge_str.as_bytes(), &sig)
                .expect("client sent an invalid signature");

            let resp = serde_json::json!({"id": req["id"], "code": 0});
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            // Step 3: receive pi_is_verified, send confirmation.
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            assert_eq!(req["method"], "pi_is_verified");

            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"is_verified": true},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        authenticate(&mut stream, &key).await.expect("authenticate should succeed");
        telescope.await.unwrap();
    }

    /// Telescope rejects verify_client with a non-zero code.
    #[tokio::test]
    async fn authenticate_telescope_rejects_verify() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();

            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"str": "challenge"},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            // Reject with error code.
            let resp = serde_json::json!({"id": req["id"], "code": 1001});
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = authenticate(&mut stream, &key).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "expected AuthFailed, got {result:?}"
        );
        telescope.await.unwrap();
    }

    /// Telescope sends malformed JSON in response to get_verify_str.
    #[tokio::test]
    async fn authenticate_malformed_challenge_response() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            write.write_all(b"this is not json\r\n").await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = authenticate(&mut stream, &key).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "expected AuthFailed on malformed JSON, got {result:?}"
        );
        telescope.await.unwrap();
    }

    /// Telescope sends an empty challenge string — must be rejected before signing.
    #[tokio::test]
    async fn authenticate_empty_challenge_from_telescope() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"str": ""},  // empty challenge
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = authenticate(&mut stream, &key).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "expected AuthFailed on empty challenge, got {result:?}"
        );
        telescope.await.unwrap();
    }

    /// Telescope accepts verify_client then closes before pi_is_verified.
    /// Authentication must still succeed — pi_is_verified is non-fatal.
    #[tokio::test]
    async fn authenticate_pi_is_verified_nonfatal_on_close() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();

            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"str": "challenge"},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({"id": req["id"], "code": 0});
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            // Close before pi_is_verified — client should succeed anyway.
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        authenticate(&mut stream, &key)
            .await
            .expect("auth must succeed even when pi_is_verified connection is closed");
        telescope.await.unwrap();
    }

    /// Telescope reports is_verified: false — authentication still succeeds.
    #[tokio::test]
    async fn authenticate_pi_is_verified_false_is_nonfatal() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();

            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"str": "challenge"},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            let resp = serde_json::json!({"id": req["id"], "code": 0});
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();

            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            // Report not verified — should be ignored by client.
            let resp = serde_json::json!({
                "id": req["id"], "code": 0,
                "result": {"is_verified": false},
            });
            write.write_all(format!("{resp}\r\n").as_bytes()).await.unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        authenticate(&mut stream, &key)
            .await
            .expect("auth must succeed even when is_verified is false");
        telescope.await.unwrap();
    }

    /// Live test against a real Seestar scope. Skipped in normal `cargo test` runs.
    ///
    /// Authenticates, then sends `get_device_state` and asserts the telescope
    /// responds with `code: 0` — proving the scope accepted the key and is
    /// processing commands, not just that the local handshake logic ran cleanly.
    ///
    /// ```text
    /// SEESTAR_HOST=192.168.x.x SEESTAR_INTEROP_PEM=~/client.pem \
    ///   cargo test -p scopinator-seestar auth::tests::live -- --ignored --nocapture
    /// ```
    #[tokio::test]
    #[ignore]
    async fn live() {
        use tokio::io::AsyncBufReadExt;

        let host: std::net::Ipv4Addr = std::env::var("SEESTAR_HOST")
            .expect("set SEESTAR_HOST to the scope's IP address")
            .parse()
            .expect("SEESTAR_HOST must be a valid IPv4 address");
        let pem_path =
            std::env::var("SEESTAR_INTEROP_PEM").expect("set SEESTAR_INTEROP_PEM to the key path");
        let pem = std::fs::read_to_string(&pem_path)
            .unwrap_or_else(|e| panic!("failed to read {pem_path}: {e}"));
        let key = InteropKey::from_pem(&pem).expect("failed to parse interop PEM");

        let addr = std::net::SocketAddr::from((host, 4700u16));
        let mut stream = tokio::net::TcpStream::connect(addr)
            .await
            .expect("failed to connect to scope");

        authenticate(&mut stream, &key)
            .await
            .expect("authentication failed");

        // Send get_device_state to confirm the telescope is actually accepting
        // commands — a rejected key would return a non-zero code here.
        let cmd = serde_json::json!({"id": 100000, "method": "get_device_state", "params": ["verify"]});
        stream
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .expect("failed to send get_device_state");

        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        tokio::time::timeout(std::time::Duration::from_secs(10), reader.read_line(&mut line))
            .await
            .expect("timed out waiting for get_device_state response")
            .expect("read error");

        let resp: serde_json::Value =
            serde_json::from_str(line.trim()).expect("invalid JSON in get_device_state response");

        println!("get_device_state response: {resp}");

        let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        assert_eq!(
            code, 0,
            "get_device_state returned code={code} — scope may have rejected the auth key"
        );
    }

    /// Telescope closes the connection after receiving get_verify_str (EOF during challenge read).
    #[tokio::test]
    async fn authenticate_connection_closed_mid_handshake() {
        let private_key = make_test_private_key();
        let key = interop_key_from_private(&private_key);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let telescope = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, _write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            // Read get_verify_str so the client's write succeeds, then drop → clean EOF.
            reader.read_line(&mut line).await.unwrap();
            // stream dropped here, sending EOF to client
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = authenticate(&mut stream, &key).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "expected AuthFailed, got {result:?}"
        );
        telescope.await.unwrap();
    }
}
