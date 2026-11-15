//! Connect-time verification negotiation for the Seestar control socket.
//!
//! Every control connection starts with `get_verify_str`, and the reply decides
//! how the rest of the session is framed (see [`Verification`]):
//!
//! - code 103 (method not found): a legacy device with no verification gate.
//! - a challenge and a configured [`InteropKey`]: sign it (RSA-PKCS1v15-SHA1)
//!   and send `verify_client`. On success the socket is verified and requests
//!   need no `verify` marker.
//! - a challenge and no key: probe one request carrying the `verify` marker.
//!   Firmware 6.70 and 7.06 serve marked requests on an unverified socket; if
//!   the probe goes unanswered the device needs a key and the connection fails
//!   with [`SeestarError::VerificationRequired`].
//!
//! The handshake exists on firmware 6.70 (2670) already, observed on an S50 on
//! 2026-11-14; it is not specific to 7.18+. An unverified socket silently
//! consumes unmarked requests, so nothing else may be sent before this
//! completes.
//!
//! # Interoperability Notice
//!
//! The [`InteropKey`] and the challenge/response handshake implemented here are used for
//! interoperability purposes under 17 U.S.C. § 1201(f) (the DMCA interoperability
//! exemption), enabling independent programs to interoperate with Seestar devices via
//! the challenge-response authentication the firmware requires.
//!
//! The legality of key use varies by jurisdiction. You are solely responsible for ensuring
//! compliance with the laws of your region.

use std::sync::Arc;
use std::time::Duration;

use base64::{Engine, engine::general_purpose::STANDARD as BASE64};
use rsa::RsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use sha1::Sha1;
use signature::{SignatureEncoding, Signer};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tracing::{debug, info, warn};

use crate::error::SeestarError;
use crate::protocol::json_rpc::MAX_LINE_BYTES;

/// How long to wait for each handshake reply when a key is configured.
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to wait for each probe reply when no key is configured. Shorter
/// than [`AUTH_TIMEOUT`] because here silence is an answer, not a failure.
pub(crate) const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Fixed JSON-RPC IDs for negotiation messages — well below INITIAL_COMMAND_ID (100_000).
const ID_GET_VERIFY: u64 = 1001;
const ID_VERIFY_CLIENT: u64 = 1002;
const ID_MARKER_PROBE: u64 = 1003;

/// `code` of the Alert sent to a connection refused for exceeding the client limit.
const CODE_CONNECTION_LIMIT: i64 = 429;

/// Device error code for an unknown method.
const CODE_METHOD_NOT_FOUND: i64 = 103;

/// How a control connection gets its requests past the device's verification gate.
///
/// Negotiated once per connection; a reconnect negotiates again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verification {
    /// The device has no verification handshake (`get_verify_str` is unknown
    /// or unanswered). Requests carry the `verify` marker according to the
    /// firmware-version rule inherited from pyscopinator.
    Legacy,
    /// The socket was verified with `verify_client`. Requests carry no marker.
    Handshake,
    /// The device has the handshake but no key is configured, and it serves
    /// requests that carry the `verify` marker. Every request carries it.
    Marker,
}

/// Outcome of [`negotiate`].
#[derive(Debug)]
pub struct Negotiated {
    /// The framing the rest of the session must use.
    pub mode: Verification,
    /// Messages that arrived during negotiation and were not replies to it
    /// (events, a greeting). The caller should route them as if they had
    /// arrived after negotiation.
    pub deferred: Vec<serde_json::Value>,
}

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

/// Negotiate verification on a freshly connected control socket.
///
/// `reader` must be the buffered reader that the session goes on to use: bytes
/// it has buffered past the last handshake reply belong to the session.
/// Nothing else may be written to the socket until this returns.
///
/// An error means the connection is unusable and the caller should drop it.
pub async fn negotiate<R, W>(
    reader: &mut R,
    writer: &mut W,
    key: Option<&InteropKey>,
) -> Result<Negotiated, SeestarError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut io = HandshakeIo {
        reader,
        writer,
        partial: Vec::new(),
        deferred: Vec::new(),
    };
    let mode = negotiate_mode(&mut io, key).await?;
    info!(?mode, "control connection verification negotiated");
    Ok(Negotiated {
        mode,
        deferred: io.deferred,
    })
}

async fn negotiate_mode<R, W>(
    io: &mut HandshakeIo<'_, R, W>,
    key: Option<&InteropKey>,
) -> Result<Verification, SeestarError>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let wait = if key.is_some() {
        AUTH_TIMEOUT
    } else {
        PROBE_TIMEOUT
    };

    // Step 1: ask for a challenge. The device answers this on an unverified
    // socket; a legacy device answers "method not found".
    io.send(serde_json::json!({
        "id": ID_GET_VERIFY,
        "method": "get_verify_str",
    }))
    .await?;

    let Some(resp) = io.reply(ID_GET_VERIFY, wait).await? else {
        if key.is_some() {
            return Err(timed_out());
        }
        // Without a key there is nothing to lose by carrying on as before:
        // a relay or simulator that swallows the request is not a device
        // that demands verification.
        warn!("no reply to get_verify_str; treating the device as legacy");
        return Ok(Verification::Legacy);
    };

    if resp.get("code").and_then(|c| c.as_i64()) == Some(CODE_METHOD_NOT_FOUND) {
        debug!("get_verify_str is unknown to this device (legacy, no verification)");
        return Ok(Verification::Legacy);
    }

    let challenge = match (extract_challenge(&resp), key) {
        (Ok(challenge), _) => challenge,
        (Err(e), Some(_)) => return Err(e),
        (Err(e), None) => {
            warn!("unusable get_verify_str reply ({e}); treating the device as legacy");
            return Ok(Verification::Legacy);
        }
    };

    let Some(key) = key else {
        // Step 2 without a key: does the device serve marked requests?
        io.send(serde_json::json!({
            "id": ID_MARKER_PROBE,
            "method": "test_connection",
            "params": ["verify"],
        }))
        .await?;
        return match io.reply(ID_MARKER_PROBE, PROBE_TIMEOUT).await? {
            Some(_) => Ok(Verification::Marker),
            None => Err(SeestarError::VerificationRequired),
        };
    };

    // Step 2 with a key: sign the challenge. `data` must always be sent;
    // omitting it selects a factory path on the device.
    let sig = sign_challenge(key, &challenge);
    io.send(serde_json::json!({
        "id": ID_VERIFY_CLIENT,
        "method": "verify_client",
        "params": { "sign": sig, "data": challenge },
    }))
    .await?;

    let resp = io
        .reply(ID_VERIFY_CLIENT, AUTH_TIMEOUT)
        .await?
        .ok_or_else(timed_out)?;
    check_verify_client(&resp)?;
    Ok(Verification::Handshake)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn timed_out() -> SeestarError {
    SeestarError::AuthFailed("timed out waiting for handshake response".to_string())
}

/// Line-oriented request/reply IO for the negotiation phase.
struct HandshakeIo<'a, R, W> {
    reader: &'a mut R,
    writer: &'a mut W,
    /// Bytes of a line that has not been terminated yet. Kept across calls so
    /// a reply timeout does not discard the start of a line.
    partial: Vec<u8>,
    deferred: Vec<serde_json::Value>,
}

impl<R, W> HandshakeIo<'_, R, W>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
{
    /// Write a JSON value as a single `\r\n`-terminated line.
    async fn send(&mut self, msg: serde_json::Value) -> Result<(), SeestarError> {
        let mut line = msg.to_string();
        line.push_str("\r\n");
        self.writer
            .write_all(line.as_bytes())
            .await
            .map_err(SeestarError::Connection)?;
        self.writer.flush().await.map_err(SeestarError::Connection)
    }

    /// Read lines until the reply with the given `id` arrives.
    ///
    /// Anything else (events, a greeting, replies to other ids) is kept in
    /// `deferred`. Returns `Ok(None)` if `wait` elapses first.
    async fn reply(
        &mut self,
        id: u64,
        wait: Duration,
    ) -> Result<Option<serde_json::Value>, SeestarError> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            // `read_until` keeps partially read bytes in `self.partial` when
            // the timeout cancels it, so no data is lost.
            let read =
                tokio::time::timeout_at(deadline, self.reader.read_until(b'\n', &mut self.partial))
                    .await;
            match read {
                Err(_) => return Ok(None),
                Ok(Err(e)) => return Err(SeestarError::Connection(e)),
                Ok(Ok(_)) if !self.partial.ends_with(b"\n") => {
                    // EOF, possibly in the middle of a line.
                    return Err(SeestarError::AuthFailed(
                        "connection closed during handshake".to_string(),
                    ));
                }
                Ok(Ok(_)) => {}
            }

            let line = std::mem::take(&mut self.partial);
            if line.len() > MAX_LINE_BYTES {
                return Err(SeestarError::LineTooLong {
                    size: line.len(),
                    limit: MAX_LINE_BYTES,
                });
            }
            let text = String::from_utf8_lossy(&line);
            let trimmed = text.trim();
            if trimmed.is_empty() {
                continue;
            }
            let msg: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
                SeestarError::AuthFailed(format!("invalid JSON in handshake response: {e}"))
            })?;
            if msg.get("id").and_then(|v| v.as_u64()) == Some(id) {
                return Ok(Some(msg));
            }
            if is_connection_limit_alert(&msg) {
                return Err(SeestarError::ConnectionLimit);
            }
            self.deferred.push(msg);
        }
    }
}

/// True for the `Alert` the device sends to a connection it is about to close
/// because the client limit is reached:
/// `{"Event":"Alert","code":429,"Msg":"Exceed the maximum number of connections",...}`.
fn is_connection_limit_alert(msg: &serde_json::Value) -> bool {
    msg.get("Event").and_then(|e| e.as_str()) == Some("Alert")
        && msg.get("code").and_then(|c| c.as_i64()) == Some(CODE_CONNECTION_LIMIT)
}

/// Extract the challenge string from a `get_verify_str` response.
fn extract_challenge(msg: &serde_json::Value) -> Result<String, SeestarError> {
    let challenge = msg
        .get("result")
        .and_then(|r| r.get("str"))
        .and_then(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            SeestarError::AuthFailed(format!("get_verify_str response missing result.str: {msg}"))
        })?;
    Ok(challenge.to_string())
}

/// Sign the challenge string with RSA-PKCS1v15-SHA1, return base64.
fn sign_challenge(key: &InteropKey, challenge: &str) -> String {
    let sig: rsa::pkcs1v15::Signature = key.0.sign(challenge.as_bytes());
    BASE64.encode(sig.to_bytes())
}

/// Check that `verify_client` was accepted (code == 0).
///
/// Only the code is checked: firmware 9.31 and earlier reply `result: 0`,
/// firmware 9.75 replies `result: {"key_envelope": ...}`.
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
    use rsa::pkcs1::EncodeRsaPrivateKey;
    use rsa::pkcs1v15::VerifyingKey;
    use rsa::pkcs8::EncodePrivateKey;
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

    /// Run [`negotiate`] on a whole stream, as the control loop does.
    async fn negotiate_on(
        stream: &mut tokio::net::TcpStream,
        key: Option<&InteropKey>,
    ) -> Result<Negotiated, SeestarError> {
        let (read, mut write) = stream.split();
        let mut reader = BufReader::new(read);
        negotiate(&mut reader, &mut write, key).await
    }

    /// Negotiate with a key and require the handshake to have been used.
    async fn authenticate(
        stream: &mut tokio::net::TcpStream,
        key: &InteropKey,
    ) -> Result<(), SeestarError> {
        let negotiated = negotiate_on(stream, Some(key)).await?;
        assert_eq!(negotiated.mode, Verification::Handshake);
        Ok(())
    }

    /// A scripted telescope: for each step, read one request, assert its
    /// method, and write the given lines (`$id` is replaced by the request id).
    /// Returns the requests it received.
    async fn scripted(
        steps: Vec<(&'static str, Vec<String>)>,
    ) -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<Vec<serde_json::Value>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (read, mut write) = tokio::io::split(stream);
            let mut reader = BufReader::new(read);
            let mut seen = Vec::new();
            for (method, replies) in steps {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
                assert_eq!(req["method"], method);
                for reply in replies {
                    let out = reply.replace("$id", &req["id"].to_string());
                    write
                        .write_all(format!("{out}\r\n").as_bytes())
                        .await
                        .unwrap();
                }
                seen.push(req);
            }
            // Hold the socket open until the client is done with it.
            let mut rest = String::new();
            let _ = reader.read_line(&mut rest).await;
            seen
        });
        (addr, handle)
    }

    const CHALLENGE_REPLY: &str = r#"{"id":$id,"code":0,"result":{"str":"challenge"}}"#;

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
        assert!(
            err.to_string().contains("encrypted"),
            "error should mention encryption"
        );
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
        verifying_key
            .verify(b"", &sig)
            .expect("empty-challenge sig must verify");
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
            write
                .write_all(format!("{resp}\r\n").as_bytes())
                .await
                .unwrap();

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
            write
                .write_all(format!("{resp}\r\n").as_bytes())
                .await
                .unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        authenticate(&mut stream, &key)
            .await
            .expect("authenticate should succeed");
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
            write
                .write_all(format!("{resp}\r\n").as_bytes())
                .await
                .unwrap();

            line.clear();
            reader.read_line(&mut line).await.unwrap();
            let req: serde_json::Value = serde_json::from_str(line.trim()).unwrap();
            // Reject with error code.
            let resp = serde_json::json!({"id": req["id"], "code": 1001});
            write
                .write_all(format!("{resp}\r\n").as_bytes())
                .await
                .unwrap();
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
            write
                .write_all(format!("{resp}\r\n").as_bytes())
                .await
                .unwrap();
        });

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let result = authenticate(&mut stream, &key).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "expected AuthFailed on empty challenge, got {result:?}"
        );
        telescope.await.unwrap();
    }

    /// Firmware 9.75 answers `verify_client` with a key envelope, not `0`.
    #[tokio::test]
    async fn handshake_accepts_key_envelope_result() {
        let key = interop_key_from_private(&make_test_private_key());
        let (addr, telescope) = scripted(vec![
            ("get_verify_str", vec![CHALLENGE_REPLY.into()]),
            (
                "verify_client",
                vec![r#"{"id":$id,"code":0,"result":{"key_envelope":{"v":1}}}"#.into()],
            ),
        ])
        .await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let negotiated = negotiate_on(&mut stream, Some(&key)).await.unwrap();
        assert_eq!(negotiated.mode, Verification::Handshake);
        drop(stream);

        // The handshake is two requests; `pi_is_verified` is not part of it.
        let seen = telescope.await.unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1]["params"]["data"], "challenge");
    }

    /// A bad signature is rejected with 207 (observed on firmware 6.70).
    #[tokio::test]
    async fn handshake_rejected_with_207() {
        let key = interop_key_from_private(&make_test_private_key());
        let (addr, _telescope) = scripted(vec![
            ("get_verify_str", vec![CHALLENGE_REPLY.into()]),
            (
                "verify_client",
                vec![r#"{"id":$id,"code":207,"error":"fail to operate"}"#.into()],
            ),
        ])
        .await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let err = negotiate_on(&mut stream, Some(&key)).await.unwrap_err();
        assert!(
            matches!(&err, SeestarError::AuthFailed(m) if m.contains("207")),
            "got {err:?}"
        );
    }

    /// `get_verify_str` unknown (103) means a legacy device, with or without a key.
    #[tokio::test]
    async fn method_not_found_is_legacy() {
        let key = interop_key_from_private(&make_test_private_key());
        for key in [None, Some(&key)] {
            let (addr, telescope) = scripted(vec![(
                "get_verify_str",
                vec![r#"{"id":$id,"code":103,"error":"method not found"}"#.into()],
            )])
            .await;

            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let negotiated = negotiate_on(&mut stream, key).await.unwrap();
            assert_eq!(negotiated.mode, Verification::Legacy);
            drop(stream);
            assert_eq!(telescope.await.unwrap().len(), 1, "nothing sent after 103");
        }
    }

    /// No key, but the device serves a marked request: marker mode.
    #[tokio::test]
    async fn no_key_and_marked_probe_answered_is_marker_mode() {
        let (addr, telescope) = scripted(vec![
            ("get_verify_str", vec![CHALLENGE_REPLY.into()]),
            (
                "test_connection",
                vec![r#"{"id":$id,"code":0,"result":"server connected!"}"#.into()],
            ),
        ])
        .await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let negotiated = negotiate_on(&mut stream, None).await.unwrap();
        assert_eq!(negotiated.mode, Verification::Marker);
        drop(stream);

        let seen = telescope.await.unwrap();
        assert_eq!(seen[1]["params"], serde_json::json!(["verify"]));
    }

    /// No key and the marked probe is silently consumed: the device needs a
    /// key. This must fail within the probe timeout, not a command timeout.
    #[tokio::test(start_paused = true)]
    async fn no_key_and_silent_probe_is_verification_required() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let telescope = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(&mut server);
            let mut reader = BufReader::new(read);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            write
                .write_all(b"{\"id\":1001,\"code\":0,\"result\":{\"str\":\"c\"}}\r\n")
                .await
                .unwrap();
            // Read the probe and never answer it.
            line.clear();
            reader.read_line(&mut line).await.unwrap();
            assert!(line.contains("test_connection"));
            std::future::pending::<()>().await;
        });

        let started = tokio::time::Instant::now();
        let (read, mut write) = tokio::io::split(&mut client);
        let mut reader = BufReader::new(read);
        let err = negotiate(&mut reader, &mut write, None).await.unwrap_err();
        assert!(
            matches!(err, SeestarError::VerificationRequired),
            "got {err:?}"
        );
        assert_eq!(started.elapsed(), PROBE_TIMEOUT);
        telescope.abort();
    }

    /// Over the client limit the device sends Alert 429 and closes. That is
    /// reported as such, not as a failed handshake.
    #[tokio::test]
    async fn connection_limit_alert_is_reported() {
        let key = interop_key_from_private(&make_test_private_key());
        for key in [None, Some(&key)] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let alert = r#"{"Event":"Alert","Timestamp":"1.0","code":429,"Type":"error","Msg":"Exceed the maximum number of connections"}"#;
                stream
                    .write_all(format!("{alert}\r\n").as_bytes())
                    .await
                    .unwrap();
                // Dropping the stream closes the connection.
            });
            let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let err = negotiate_on(&mut stream, key).await.unwrap_err();
            assert!(matches!(err, SeestarError::ConnectionLimit), "got {err:?}");
        }
    }

    // ── Robustness against a misbehaving peer ─────────────────────────────────
    // (Forge task "Add auth robustness fake listener tests".) The peer here
    // is an in-memory duplex stream under paused time, so timeouts are exact
    // and nothing can hang the test run.

    /// Run `negotiate` against a peer that reads one request and writes
    /// `reply`, then goes silent with the socket open.
    async fn negotiate_against(
        key: Option<&InteropKey>,
        replies: &'static [&'static str],
    ) -> (Result<Negotiated, SeestarError>, Duration) {
        let (mut client, mut server) = tokio::io::duplex(8192);
        let peer = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(&mut server);
            let mut reader = BufReader::new(read);
            for reply in replies {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                write.write_all(reply.as_bytes()).await.unwrap();
            }
            std::future::pending::<()>().await;
        });
        let started = tokio::time::Instant::now();
        let (read, mut write) = tokio::io::split(&mut client);
        let mut reader = BufReader::new(read);
        let result = negotiate(&mut reader, &mut write, key).await;
        peer.abort();
        (result, started.elapsed())
    }

    const CHALLENGE_LINE: &str = "{\"id\":1001,\"code\":0,\"result\":{\"str\":\"c\"}}\r\n";

    /// With a key, a peer that accepts and then says nothing fails after
    /// exactly AUTH_TIMEOUT: bounded, and never treated as verified.
    #[tokio::test(start_paused = true)]
    async fn silent_peer_fails_after_auth_timeout() {
        let key = interop_key_from_private(&make_test_private_key());
        let (result, elapsed) = negotiate_against(Some(&key), &[]).await;
        assert!(
            matches!(result, Err(SeestarError::AuthFailed(_))),
            "{result:?}"
        );
        assert_eq!(elapsed, AUTH_TIMEOUT);
    }

    /// Silence after the challenge (the peer never answers `verify_client`)
    /// is also a bounded failure.
    #[tokio::test(start_paused = true)]
    async fn silence_after_challenge_fails_after_auth_timeout() {
        let key = interop_key_from_private(&make_test_private_key());
        let (result, elapsed) = negotiate_against(Some(&key), &[CHALLENGE_LINE]).await;
        assert!(
            matches!(&result, Err(SeestarError::AuthFailed(m)) if m.contains("timed out")),
            "{result:?}"
        );
        assert_eq!(elapsed, AUTH_TIMEOUT);
    }

    /// A reply carrying the wrong id is not taken for the answer, however
    /// plausible its body: a `verify_client` success sent in reply to
    /// `get_verify_str` (wrong order), and a challenge with a foreign id.
    #[tokio::test(start_paused = true)]
    async fn replies_with_the_wrong_id_are_not_accepted() {
        let key = interop_key_from_private(&make_test_private_key());
        for reply in [
            "{\"id\":1002,\"code\":0,\"result\":0}\r\n",
            "{\"id\":7,\"code\":0,\"result\":{\"str\":\"c\"}}\r\n",
            "{\"code\":0,\"result\":{\"str\":\"c\"}}\r\n",
            "{\"id\":\"1001\",\"code\":0,\"result\":{\"str\":\"c\"}}\r\n",
        ] {
            let replies: &'static [&'static str] = Box::leak(Box::new([reply]));
            let (result, elapsed) = negotiate_against(Some(&key), replies).await;
            assert!(
                matches!(result, Err(SeestarError::AuthFailed(_))),
                "{reply}: {result:?}"
            );
            assert_eq!(elapsed, AUTH_TIMEOUT, "{reply}");
        }
    }

    /// Truncated or non-JSON lines end the handshake at once with an error.
    #[tokio::test(start_paused = true)]
    async fn malformed_lines_fail_immediately() {
        let key = interop_key_from_private(&make_test_private_key());
        for reply in [
            "{\"id\":1001,\"code\":0,\"result\":{\"str\":\r\n",
            "\u{0}\u{1}\u{2}\r\n",
            "[1001]]\r\n",
        ] {
            let replies: &'static [&'static str] = Box::leak(Box::new([reply]));
            let (result, elapsed) = negotiate_against(Some(&key), replies).await;
            assert!(
                matches!(&result, Err(SeestarError::AuthFailed(m)) if m.contains("invalid JSON")),
                "{reply:?}: {result:?}"
            );
            assert_eq!(elapsed, Duration::ZERO, "{reply:?}");
        }
    }

    /// A challenge that is not a usable string is refused before anything is
    /// signed or sent.
    #[tokio::test(start_paused = true)]
    async fn unusable_challenges_are_refused_with_a_key() {
        let key = interop_key_from_private(&make_test_private_key());
        for reply in [
            "{\"id\":1001,\"code\":0,\"result\":{\"str\":42}}\r\n",
            "{\"id\":1001,\"code\":0,\"result\":\"c\"}\r\n",
            "{\"id\":1001,\"code\":200,\"error\":\"x\"}\r\n",
        ] {
            let replies: &'static [&'static str] = Box::leak(Box::new([reply]));
            let (result, _) = negotiate_against(Some(&key), replies).await;
            assert!(
                matches!(&result, Err(SeestarError::AuthFailed(m)) if m.contains("result.str")),
                "{reply}: {result:?}"
            );
        }
    }

    /// The peer closing mid-line (a reply cut off without its terminator) is
    /// a closed connection, not a parsed reply.
    #[tokio::test]
    async fn reply_cut_off_before_its_terminator_is_a_closed_connection() {
        let key = interop_key_from_private(&make_test_private_key());
        let (addr, _telescope) = {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 256];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buf).await;
                stream
                    .write_all(b"{\"id\":1001,\"code\":0,\"result\":{\"str\":\"c\"}}")
                    .await
                    .unwrap();
            });
            (addr, handle)
        };
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let err = negotiate_on(&mut stream, Some(&key)).await.unwrap_err();
        assert!(
            matches!(&err, SeestarError::AuthFailed(m) if m.contains("closed")),
            "got {err:?}"
        );
    }

    /// No key and no reply to `get_verify_str` at all: carry on as legacy.
    #[tokio::test(start_paused = true)]
    async fn no_key_and_silent_device_is_legacy() {
        let (mut client, _server) = tokio::io::duplex(4096);
        let (read, mut write) = tokio::io::split(&mut client);
        let mut reader = BufReader::new(read);
        let negotiated = negotiate(&mut reader, &mut write, None).await.unwrap();
        assert_eq!(negotiated.mode, Verification::Legacy);
    }

    /// Events and a greeting that arrive between handshake replies are not
    /// mistaken for replies and are handed back to the caller. A message that
    /// follows the last reply in the same packet stays in the caller's reader.
    #[tokio::test]
    async fn interleaved_events_are_deferred_and_trailing_bytes_kept() {
        let key = interop_key_from_private(&make_test_private_key());
        let (addr, _telescope) = scripted(vec![
            (
                "get_verify_str",
                vec![
                    r#"{"Event":"Version","name":"greeting"}"#.into(),
                    r#"{"Event":"PiStatus","temp":30.0}"#.into(),
                    CHALLENGE_REPLY.into(),
                ],
            ),
            (
                "verify_client",
                vec![
                    concat!(
                        r#"{"Event":"PiStatus","temp":31.0}"#,
                        "\r\n",
                        r#"{"id":$id,"code":0,"result":0}"#,
                        "\r\n",
                        r#"{"Event":"PiStatus","temp":32.0}"#
                    )
                    .into(),
                ],
            ),
        ])
        .await;

        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let (read, mut write) = stream.split();
        let mut reader = BufReader::new(read);
        let negotiated = negotiate(&mut reader, &mut write, Some(&key))
            .await
            .unwrap();
        assert_eq!(negotiated.mode, Verification::Handshake);
        let names: Vec<_> = negotiated
            .deferred
            .iter()
            .map(|m| (m["Event"].as_str().unwrap(), m["temp"].as_f64()))
            .collect();
        assert_eq!(
            names,
            [
                ("Version", None),
                ("PiStatus", Some(30.0)),
                ("PiStatus", Some(31.0))
            ]
        );

        // The event after the final reply is still readable by the session.
        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("32.0"), "got {line:?}");
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

        // Send an unmarked get_device_state: an unverified socket silently
        // consumes it, so a reply proves the handshake verified this socket.
        let cmd = serde_json::json!({"id": 100000, "method": "get_device_state"});
        stream
            .write_all(format!("{cmd}\r\n").as_bytes())
            .await
            .expect("failed to send get_device_state");

        let mut reader = BufReader::new(&mut stream);
        let mut line = String::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(10),
            reader.read_line(&mut line),
        )
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
